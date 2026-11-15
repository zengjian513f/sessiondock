//! Owns an opaque child process and durable byte streams, independently of clients.
#[cfg(any(unix, windows))]
mod channel;
#[cfg(any(unix, windows))]
mod commands;
#[cfg(any(unix, windows))]
mod receipts;
#[cfg(windows)]
mod windows;
#[cfg(any(unix, windows))]
mod host {
    use sdkhost_client::platform::OpenOptionsExt;
    use sdkhost_client::{
        Exit, LaunchSpec, ReadResult, Receipt, Request, Response, Status, Stream, os, private_dir,
    };
    use std::{io, path::PathBuf, sync::Arc};
    use tokio::{
        io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
        sync::{Mutex, Notify},
    };
    struct State {
        dir: PathBuf,
        status: Status,
        input: Arc<Mutex<Option<tokio::process::ChildStdin>>>,
        receipts: crate::receipts::Receipts,
        signals:
            tokio::sync::mpsc::UnboundedSender<(i32, tokio::sync::oneshot::Sender<io::Result<()>>)>,
    }
    type Shared = Arc<Mutex<State>>;
    /// Signal the child's process group and every process in its session.
    /// The unreaped child keeps both IDs reserved, so neither can be reused.
    #[cfg(target_os = "linux")]
    fn signal_session(leader: i32, signal: i32) -> io::Result<()> {
        // Scan before signalling: once the child dies the host may exit, and the
        // caller's acknowledgement must not wait behind a /proc walk.
        let mut members = Vec::new();
        for entry in std::fs::read_dir("/proc")?.flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|n| n.parse::<i32>().ok())
            else {
                continue;
            };
            let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
                continue;
            };
            // Fields after the parenthesised command: state ppid pgrp session.
            let session = stat
                .rsplit_once(')')
                .and_then(|(_, rest)| rest.split_whitespace().nth(3))
                .and_then(|value| value.parse::<i32>().ok());
            if session == Some(leader) && pid != leader {
                members.push(pid);
            }
        }
        let mut result = if unsafe { libc::kill(-leader, signal) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        };
        for pid in members {
            if unsafe { libc::kill(pid, signal) } == 0 {
                result = Ok(());
            }
        }
        result
    }
    #[cfg(target_os = "macos")]
    fn signal_session(leader: i32, signal: i32) -> io::Result<()> {
        // libproc lists current processes; getsid includes workers which changed groups.
        unsafe extern "C" {
            fn proc_listallpids(buffer: *mut libc::c_void, size: libc::c_int) -> libc::c_int;
        }
        let count = unsafe { proc_listallpids(std::ptr::null_mut(), 0) };
        if count <= 0 {
            return Err(io::Error::last_os_error());
        }
        let mut pids = vec![0i32; count as usize + 64];
        let count = unsafe { proc_listallpids(pids.as_mut_ptr().cast(), (pids.len() * 4) as i32) };
        if count < 0 {
            return Err(io::Error::last_os_error());
        }
        let members: Vec<_> = pids
            .into_iter()
            .take(count as usize)
            .filter(|pid| *pid > 0 && unsafe { libc::getsid(*pid) } == leader)
            .collect();
        let mut result = if unsafe { libc::kill(-leader, signal) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        };
        for pid in members {
            if unsafe { libc::kill(pid, signal) } == 0 {
                result = Ok(());
            }
        }
        result
    }
    #[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
    fn signal_session(leader: i32, signal: i32) -> io::Result<()> {
        if unsafe { libc::kill(-leader, signal) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
    fn persist(state: &State) -> io::Result<()> {
        let temp = state.dir.join("status.tmp");
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temp)?;
        use std::io::Write;
        f.write_all(&serde_json::to_vec(&state.status)?)?;
        f.sync_data()?;
        sdkhost_client::platform::replace(&temp, &state.dir.join("status.json"))
    }
    fn receipt(state: &mut State, id: &str, status: &str) -> io::Result<Receipt> {
        state.receipts.save(id, status)
    }

    async fn drain<R: tokio::io::AsyncRead + Unpin>(
        mut reader: R,
        path: PathBuf,
        state: Shared,
        notify: Arc<Notify>,
        stdout: bool,
    ) -> io::Result<()> {
        let mut output = sdkhost_client::segments::Writer::new(path.clone())?;
        let mut timing = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(path.with_extension("time"))?;
        let mut bytes = [0; 65536];
        let mut end = 0u64;
        loop {
            let n = reader.read(&mut bytes).await?;
            let received = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(io::Error::other)?
                .as_millis() as u64;
            if n == 0 {
                break;
            }
            let chunk = bytes[..n].to_vec();
            output = tokio::task::spawn_blocking(move || {
                output.write(&chunk)?;
                Ok::<_, io::Error>(output)
            })
            .await
            .map_err(io::Error::other)??;
            // Opaque byte ranges, not parsed SDK messages. Save the receipt
            // instant before making these bytes visible to attached readers.
            end += n as u64;
            use std::io::Write;
            timing.write_all(&end.to_le_bytes())?;
            timing.write_all(&received.to_le_bytes())?;
            {
                let mut s = state.lock().await;
                if stdout {
                    s.status.stdout_len += n as u64;
                } else {
                    s.status.stderr_len += n as u64;
                }
            }
            notify.notify_waiters();
        }
        timing.sync_data()?;
        tokio::task::spawn_blocking(move || output.finish())
            .await
            .map_err(io::Error::other)?
    }
    async fn handle(
        request: Request,
        state: &Shared,
        notify: &Arc<Notify>,
    ) -> io::Result<serde_json::Value> {
        match request {
            Request::Status => Ok(serde_json::to_value(&state.lock().await.status)?),
            Request::Receipt { id } => {
                Ok(serde_json::to_value(state.lock().await.receipts.get(&id)?)?)
            }
            Request::Write { id, bytes } => {
                let input = state.lock().await.input.clone();
                let mut input = input.lock().await;
                {
                    let mut s = state.lock().await;
                    if let Some(r) = s.receipts.get(&id)? {
                        return Ok(serde_json::to_value(r)?);
                    }
                    if input.is_none() {
                        return Err(io::Error::new(
                            io::ErrorKind::BrokenPipe,
                            "child stdin closed",
                        ));
                    }
                    // Persist uncertainty before touching the pipe. A retry must never
                    // write the bytes again, including after a partial pipe write.
                    receipt(&mut s, &id, "unknown")?;
                }
                let result = input.as_mut().unwrap().write_all(&bytes).await;
                let mut s = state.lock().await;
                if result.is_err() {
                    return Ok(serde_json::to_value(s.receipts.get(&id)?)?);
                }
                Ok(serde_json::to_value(receipt(&mut s, &id, "written")?)?)
            }
            Request::CloseStdin { id } => {
                let input = state.lock().await.input.clone();
                let mut input = input.lock().await;
                let mut s = state.lock().await;
                if let Some(r) = s.receipts.get(&id)? {
                    return Ok(serde_json::to_value(r)?);
                }
                receipt(&mut s, &id, "unknown")?;
                input.take();
                s.status.stdin_closed = true;
                persist(&s)?;
                Ok(serde_json::to_value(receipt(&mut s, &id, "written")?)?)
            }
            Request::Signal { signal } => {
                let (reply, receiver) = tokio::sync::oneshot::channel();
                state
                    .lock()
                    .await
                    .signals
                    .send((signal, reply))
                    .map_err(|_| io::Error::other("child already exited"))?;
                receiver
                    .await
                    .map_err(|_| io::Error::other("child already exited"))??;
                Ok(serde_json::Value::Null)
            }
            Request::Read {
                stream,
                offset,
                compact,
            } => {
                let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(250);
                let (len, exited) = loop {
                    let notified = notify.notified();
                    let (len, eof) = {
                        let s = state.lock().await;
                        match stream {
                            Stream::Stdout => (s.status.stdout_len, s.status.stdout_eof),
                            Stream::Stderr => (s.status.stderr_len, s.status.stderr_eof),
                        }
                    };
                    if offset < len || eof {
                        break (len, eof);
                    }
                    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                    if remaining.is_zero()
                        || tokio::time::timeout(remaining, notified).await.is_err()
                    {
                        break (len, eof);
                    }
                };
                let buf = if offset < len {
                    let path = state.lock().await.dir.join(match stream {
                        Stream::Stdout => "stdout",
                        Stream::Stderr => "stderr",
                    });
                    let cap = (len - offset).min(if compact { 524288 } else { 65536 }) as usize;
                    tokio::task::spawn_blocking(move || {
                        sdkhost_client::segments::read(&path, offset, cap)
                    })
                    .await
                    .map_err(io::Error::other)??
                } else {
                    Vec::new()
                };
                let n = buf.len();
                if n == 0
                    && let Some(error) = &state.lock().await.status.output_error
                {
                    return Err(io::Error::other(error.clone()));
                }
                if compact {
                    use base64::Engine;
                    Ok(
                        serde_json::json!({"bytes":base64::engine::general_purpose::STANDARD.encode(&buf),
                        "next_offset":offset+n as u64,"eof":exited && n==0}),
                    )
                } else {
                    Ok(serde_json::to_value(ReadResult {
                        bytes: buf,
                        next_offset: offset + n as u64,
                        eof: exited && n == 0,
                    })?)
                }
            }
            Request::Remove => {
                let s = state.lock().await;
                if s.status.exit.is_none() || !s.status.stdout_eof || !s.status.stderr_eof {
                    return Err(io::Error::other("cannot remove a running host"));
                }
                Ok(serde_json::Value::Null)
            }
        }
    }
    pub async fn run() -> io::Result<()> {
        let args: Vec<_> = std::env::args_os().collect();
        if args.len() != 3 || args[1] != "--dir" {
            return Err(io::Error::other("usage: sdkhost --dir PRIVATE_DIRECTORY"));
        }
        let dir = PathBuf::from(&args[2]);
        private_dir(&dir)?;
        // A permanent start marker forbids accidental relaunch after host failure.
        let mut marker = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(dir.join("host.pid"))?;
        use std::io::Write;
        writeln!(&mut marker, "{}", std::process::id())?;
        marker.sync_data()?;
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut std::io::stdin(), &mut bytes)?;
        let spec: LaunchSpec = serde_json::from_slice(&bytes)?;
        drop(bytes);
        let mut command = tokio::process::Command::new(os(spec.program));
        command.args(spec.args.into_iter().map(os));
        command.env_clear();
        for (k, v) in spec.env {
            command.env(os(k), os(v));
        }
        if let Some(cwd) = spec.cwd {
            command.current_dir(os(cwd));
        }
        command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        // A new session also makes a new process group. Kiro moves its worker
        // into another group; the session still holds it for explicit stop.
        #[cfg(unix)]
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        #[cfg(unix)]
        let mut sigchld = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::child())?;
        #[cfg(unix)]
        let mut child = command.spawn()?;
        #[cfg(windows)]
        let (job, mut child) = crate::windows::Job::spawn(&mut command)?;
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let (signals, mut signal_requests) = tokio::sync::mpsc::unbounded_channel::<(
            i32,
            tokio::sync::oneshot::Sender<io::Result<()>>,
        )>();
        if let Some(milliseconds) = spec.lifetime_ms {
            let signals = signals.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(milliseconds)).await;
                let (reply, _) = tokio::sync::oneshot::channel();
                let _ = signals.send((9, reply));
            });
        }
        let notify = Arc::new(Notify::new());
        let state = Arc::new(Mutex::new(State {
            dir: dir.clone(),
            status: Status {
                pid: child.id().unwrap(),
                exit: None,
                stdin_closed: false,
                stdout_len: 0,
                stderr_len: 0,
                stdout_eof: false,
                stderr_eof: false,
                output_error: None,
            },
            input: Arc::new(Mutex::new(child.stdin.take())),
            receipts: crate::receipts::Receipts::new(&dir)?,
            signals,
        }));
        persist(&*state.lock().await)?;
        for name in ["stdout", "stderr"] {
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(dir.join(name))?;
        }
        for (reader, stdout) in [
            (
                Box::pin(stdout) as std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>>,
                true,
            ),
            (
                Box::pin(stderr) as std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>>,
                false,
            ),
        ] {
            let state = state.clone();
            let notify = notify.clone();
            let path = dir.join(if stdout { "stdout" } else { "stderr" });
            tokio::spawn(async move {
                let result = drain(reader, path, state.clone(), notify.clone(), stdout).await;
                let mut s = state.lock().await;
                if stdout {
                    s.status.stdout_eof = true;
                } else {
                    s.status.stderr_eof = true;
                }
                if let Err(error) = result {
                    s.status.output_error = Some(format!("sdkhost output persistence: {error}"));
                }
                if let Err(error) = persist(&s) {
                    eprintln!("sdkhost status persistence: {error}");
                }
                drop(s);
                notify.notify_waiters();
            });
        }
        let waiting = state.clone();
        let wait_notify = notify.clone();
        tokio::spawn(async move {
            // Keep the exited child unreaped until inherited output pipes close.
            // Its zombie reserves the process-group identity, so explicit stop can
            // still terminate descendants without risking a reused PGID.
            #[cfg(unix)]
            let child_pid = child.id().unwrap();
            #[cfg(unix)]
            loop {
                let notified = wait_notify.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                let (rc, pid, status, code) = {
                    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
                    let rc = unsafe {
                        libc::waitid(
                            libc::P_PID,
                            child_pid,
                            &mut info,
                            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                        )
                    };
                    #[cfg(target_os = "linux")]
                    let (pid, status) = unsafe { (info.si_pid(), info.si_status()) };
                    #[cfg(not(target_os = "linux"))]
                    let (pid, status) = (info.si_pid, info.si_status);
                    (rc, pid, status, info.si_code)
                };
                let exited = {
                    let mut s = waiting.lock().await;
                    if rc == 0 && pid != 0 && s.status.exit.is_none() {
                        let normal = code == libc::CLD_EXITED;
                        s.status.exit = Some(Exit {
                            code: normal.then_some(status),
                            signal: (!normal).then_some(status),
                        });
                        if let Err(error) = persist(&s) {
                            eprintln!("sdkhost exit persistence: {error}");
                        }
                    }
                    if s.status.exit.is_some() && s.status.stdout_eof && s.status.stderr_eof {
                        break;
                    }
                    s.status.exit.is_some()
                };
                tokio::select! {
                    Some((signal, reply)) = signal_requests.recv() => {
                        let _ = reply.send(signal_session(child_pid as i32, signal));
                    }
                    _ = sigchld.recv(), if !exited => {}
                    _ = &mut notified, if exited => {}
                }
            }
            #[cfg(windows)]
            loop {
                let notified = wait_notify.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                let exited = {
                    let s = waiting.lock().await;
                    if s.status.exit.is_some() && s.status.stdout_eof && s.status.stderr_eof {
                        break;
                    }
                    s.status.exit.is_some()
                };
                // Child::wait may reap the leader: the Job handle reserves the tree's identity.
                tokio::select! {
                    exit = child.wait(), if !exited => {
                        if let Ok(exit) = exit {
                            let mut s = waiting.lock().await;
                            s.status.exit = Some(Exit { code: exit.code(), signal: None });
                            if let Err(e) = persist(&s) { eprintln!("sdkhost exit persistence: {e}"); }
                        }
                    }
                    Some((signal, reply)) = signal_requests.recv() => { let _ = reply.send(job.signal(signal)); }
                    _ = &mut notified, if exited => {}
                }
            }
            drop(signal_requests);
            let _ = child.wait().await;
            let input = waiting.lock().await.input.clone();
            input.lock().await.take();
            {
                let mut s = waiting.lock().await;
                s.status.stdin_closed = true;
                if let Err(error) = persist(&s) {
                    eprintln!("sdkhost status persistence: {error}");
                }
            }
            loop {
                let notified = wait_notify.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                let s = waiting.lock().await;
                if s.status.stdout_eof && s.status.stderr_eof {
                    break;
                }
                drop(s);
                notified.await;
            }
            std::process::exit(0);
        });
        let mut listener = sdkhost_client::platform::Listener::bind(&dir)?;
        loop {
            let socket = listener.accept().await?;
            let shared = state.clone();
            let notify = notify.clone();
            tokio::spawn(async move {
                let mut socket = BufReader::new(socket);
                let mut line = Vec::new();
                if socket.read_until(b'\n', &mut line).await.is_err() {
                    return;
                }
                let request = serde_json::from_slice::<Request>(&line);
                let remove = matches!(request, Ok(Request::Remove));
                let response = match request {
                    Ok(r) => match handle(r, &shared, &notify).await {
                        Ok(value) => Response {
                            value: Some(value),
                            error: None,
                        },
                        Err(e) => Response {
                            value: None,
                            error: Some(e.to_string()),
                        },
                    },
                    Err(e) => Response {
                        value: None,
                        error: Some(e.to_string()),
                    },
                };
                let success = response.error.is_none();
                if let Ok(mut bytes) = serde_json::to_vec(&response) {
                    bytes.push(b'\n');
                    let _ = socket.get_mut().write_all(&bytes).await;
                }
                if remove && success {
                    let s = shared.lock().await;
                    let _ = std::fs::remove_dir_all(&s.dir);
                    std::process::exit(0);
                }
            });
        }
    }
}
#[cfg(any(unix, windows))]
fn main() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(4)
        .thread_keep_alive(std::time::Duration::from_millis(250))
        .build()
        .unwrap_or_else(|e| {
            eprintln!("sdkhost: {e}");
            std::process::exit(1);
        });
    let args: Vec<_> = std::env::args_os().collect();
    let result = runtime.block_on(async move {
        if args.get(1).is_some_and(|a| a == "--websocket") {
            match args.get(2) {
                Some(address) => channel::run(address).await,
                None => Err(std::io::Error::other("missing WebSocket address")),
            }
        } else if args.get(1).is_some_and(|a| a == "--commands") {
            commands::run(&args[2..]).await
        } else {
            host::run().await
        }
    });
    if let Err(e) = result {
        eprintln!("sdkhost: {e}");
        std::process::exit(1);
    }
}
#[cfg(not(any(unix, windows)))]
fn main() {
    eprintln!("sdkhost requires Unix");
    std::process::exit(1);
}
