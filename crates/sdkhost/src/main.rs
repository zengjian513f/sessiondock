//! Owns an opaque child process and durable byte streams, independently of clients.
#[cfg(unix)]
mod unix {
    use sdkhost_client::{
        Exit, LaunchSpec, ReadResult, Receipt, Request, Response, Status, Stream, os, private_dir,
    };
    #[cfg(not(target_os = "linux"))]
    use std::os::unix::process::ExitStatusExt;
    use std::{collections::HashMap, io, os::unix::fs::OpenOptionsExt, path::PathBuf, sync::Arc};
    use tokio::{
        io::{AsyncBufReadExt, AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufReader},
        sync::{Mutex, Notify},
    };
    struct State {
        dir: PathBuf,
        status: Status,
        input: Arc<Mutex<Option<tokio::process::ChildStdin>>>,
        receipts: HashMap<String, Receipt>,
        signals:
            tokio::sync::mpsc::UnboundedSender<(i32, tokio::sync::oneshot::Sender<io::Result<()>>)>,
    }
    type Shared = Arc<Mutex<State>>;
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
        std::fs::rename(temp, state.dir.join("status.json"))
    }
    fn receipt(state: &mut State, id: &str, status: &str) -> io::Result<Receipt> {
        let result = Receipt {
            state: status.into(),
        };
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(state.dir.join("receipts.jsonl"))?;
        use std::io::Write;
        serde_json::to_writer(&mut f, &serde_json::json!({"id":id,"receipt":result}))?;
        f.write_all(b"\n")?;
        f.sync_data()?;
        state.receipts.insert(id.into(), result.clone());
        Ok(result)
    }
    async fn drain<R: tokio::io::AsyncRead + Unpin>(
        mut reader: R,
        path: PathBuf,
        state: Shared,
        notify: Arc<Notify>,
        stdout: bool,
    ) -> io::Result<()> {
        let f = std::fs::OpenOptions::new()
            .append(true)
            .mode(0o600)
            .open(path)?;
        let mut file = tokio::fs::File::from_std(f);
        let mut bytes = [0; 65536];
        loop {
            let n = reader.read(&mut bytes).await?;
            if n == 0 {
                break;
            }
            file.write_all(&bytes[..n]).await?;
            file.flush().await?;
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
        file.sync_data().await
    }
    async fn handle(
        request: Request,
        state: &Shared,
        notify: &Arc<Notify>,
    ) -> io::Result<serde_json::Value> {
        match request {
            Request::Status => Ok(serde_json::to_value(&state.lock().await.status)?),
            Request::Receipt { id } => {
                Ok(serde_json::to_value(state.lock().await.receipts.get(&id))?)
            }
            Request::Write { id, bytes } => {
                let input = state.lock().await.input.clone();
                let mut input = input.lock().await;
                {
                    let mut s = state.lock().await;
                    if let Some(r) = s.receipts.get(&id) {
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
                    return Ok(serde_json::to_value(s.receipts.get(&id))?);
                }
                Ok(serde_json::to_value(receipt(&mut s, &id, "written")?)?)
            }
            Request::CloseStdin { id } => {
                let input = state.lock().await.input.clone();
                let mut input = input.lock().await;
                let mut s = state.lock().await;
                if let Some(r) = s.receipts.get(&id) {
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
            Request::Read { stream, offset } => {
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
                    let mut file = tokio::fs::File::open(path).await?;
                    file.seek(std::io::SeekFrom::Start(offset)).await?;
                    let cap = (len - offset).min(65536) as usize;
                    let mut buf = vec![0u8; cap];
                    let n = file.read(&mut buf).await?;
                    buf.truncate(n);
                    buf
                } else {
                    Vec::new()
                };
                let n = buf.len();
                if n == 0
                    && let Some(error) = &state.lock().await.status.output_error
                {
                    return Err(io::Error::other(error.clone()));
                }
                Ok(serde_json::to_value(ReadResult {
                    bytes: buf,
                    next_offset: offset + n as u64,
                    eof: exited && n == 0,
                })?)
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
        command.process_group(0);
        let mut child = command.spawn()?;
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let (signals, mut signal_requests) = tokio::sync::mpsc::unbounded_channel::<(
            i32,
            tokio::sync::oneshot::Sender<io::Result<()>>,
        )>();
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
            receipts: HashMap::new(),
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
        tokio::spawn(async move {
            // Keep the exited child unreaped until inherited output pipes close.
            // Its zombie reserves the process-group identity, so explicit stop can
            // still terminate descendants without risking a reused PGID.
            #[cfg(target_os = "linux")]
            loop {
                let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
                let rc = unsafe {
                    libc::waitid(
                        libc::P_PID,
                        child.id().unwrap(),
                        &mut info,
                        libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                    )
                };
                if rc == 0 && unsafe { info.si_pid() } != 0 {
                    let status = unsafe { info.si_status() };
                    let exited = info.si_code == libc::CLD_EXITED;
                    let mut s = waiting.lock().await;
                    if s.status.exit.is_none() {
                        s.status.exit = Some(Exit {
                            code: exited.then_some(status),
                            signal: (!exited).then_some(status),
                        });
                        if let Err(error) = persist(&s) {
                            eprintln!("sdkhost exit persistence: {error}");
                        }
                    }
                    if s.status.stdout_eof && s.status.stderr_eof {
                        break;
                    }
                }
                tokio::select! {
                    Some((signal, reply)) = signal_requests.recv() => {
                        let result = if unsafe { libc::kill(-(child.id().unwrap() as i32), signal) } == 0 {
                            Ok(())
                        } else { Err(io::Error::last_os_error()) };
                        let _ = reply.send(result);
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {}
                }
            }
            #[cfg(not(target_os = "linux"))]
            loop {
                tokio::select! {
                    biased;
                    exit = child.wait() => {
                        if let Ok(exit) = exit {
                            let mut s = waiting.lock().await;
                            s.status.exit = Some(Exit { code: exit.code(), signal: exit.signal() });
                            let _ = persist(&s);
                        }
                        break;
                    }
                    Some((signal, reply)) = signal_requests.recv() => {
                        let result = if child.id().is_some_and(|pid| unsafe { libc::kill(-(pid as i32), signal) } == 0) { Ok(()) }
                            else { Err(io::Error::last_os_error()) };
                        let _ = reply.send(result);
                    }
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
                let s = waiting.lock().await;
                if s.status.stdout_eof && s.status.stderr_eof {
                    break;
                }
                drop(s);
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            std::process::exit(0);
        });
        let (socket_path, _directory) = sdkhost_client::socket_path(&dir)?;
        let listener = tokio::net::UnixListener::bind(socket_path)?;
        loop {
            let (socket, _) = listener.accept().await?;
            if socket.peer_cred()?.uid() != unsafe { libc::geteuid() } {
                continue;
            }
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
#[cfg(unix)]
#[tokio::main]
async fn main() {
    if let Err(e) = unix::run().await {
        eprintln!("sdkhost: {e}");
        std::process::exit(1);
    }
}
#[cfg(not(unix))]
fn main() {
    eprintln!("sdkhost requires Unix");
    std::process::exit(1);
}
