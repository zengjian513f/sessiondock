//! Persistent command channel for native machine clients. Opaque argv/results;
//! no session, API, model, approval or retry policy lives in this worker.
use base64::Engine;
use serde::Deserialize;
use std::{io, path::PathBuf, process::Stdio, sync::Arc};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

#[derive(Deserialize)]
struct Request {
    id: String,
    args: Vec<String>,
    cwd: Vec<u8>,
}

pub async fn run(args: &[std::ffi::OsString]) -> io::Result<()> {
    let directory = PathBuf::from(
        args.first()
            .ok_or_else(|| io::Error::other("missing capture directory"))?,
    );
    sdkhost_client::private_dir(&directory)?;
    let program = args
        .get(1)
        .ok_or_else(|| io::Error::other("missing command program"))?
        .clone();
    let fixed = args[2..].to_vec();
    let output = Arc::new(tokio::sync::Mutex::new(tokio::io::stdout()));
    let mut input = BufReader::new(tokio::io::stdin()).lines();
    let mut tasks = tokio::task::JoinSet::new();
    let mut next = 0u64;
    while let Some(line) = input.next_line().await? {
        let request: Request = serde_json::from_str(&line)?;
        next += 1;
        let path = directory.join(format!("capture-{next}"));
        let (program, fixed, output) = (program.clone(), fixed.clone(), output.clone());
        tasks.spawn(async move {
            let result = execute(&program, &fixed, &request, &path).await;
            let _ = tokio::fs::remove_file(&path).await;
            let value = match result {
                Ok(value) => serde_json::json!({"id":request.id,"result":value}),
                Err(error) => serde_json::json!({"id":request.id,"error":error.to_string()}),
            };
            let mut bytes = serde_json::to_vec(&value)?;
            bytes.push(b'\n');
            let mut output = output.lock().await;
            output.write_all(&bytes).await?;
            output.flush().await
        });
        while let Some(result) = tasks.try_join_next() {
            result.map_err(io::Error::other)??;
        }
    }
    while let Some(result) = tasks.join_next().await {
        result.map_err(io::Error::other)??;
    }
    Ok(())
}

async fn execute(
    program: &std::ffi::OsStr,
    fixed: &[std::ffi::OsString],
    request: &Request,
    path: &std::path::Path,
) -> io::Result<serde_json::Value> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let stdout = options.open(path)?;
    let mut command = tokio::process::Command::new(program);
    command
        .args(fixed)
        .args(&request.args)
        .current_dir(PathBuf::from(sdkhost_client::os(request.cwd.clone())))
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let mut child = command.spawn()?;
    let mut stderr = child.stderr.take().unwrap();
    let diagnostic = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).await.map(|_| bytes)
    });
    let result = tokio::time::timeout(std::time::Duration::from_secs(30), child.wait()).await;
    let status = match result {
        Ok(status) => status?,
        Err(_) => {
            #[cfg(unix)]
            if let Some(pid) = child.id() {
                // Only this call's process group, within the enclosing host's session.
                unsafe {
                    libc::kill(-(pid as i32), libc::SIGKILL);
                }
            }
            child.kill().await?;
            let _ = child.wait().await;
            diagnostic.abort();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "native API request timed out; outcome unknown",
            ));
        }
    };
    let stdout = tokio::fs::read(path).await?;
    let stderr = diagnostic.await.map_err(io::Error::other)??;
    Ok(
        serde_json::json!({"success":status.success(),"code":status.code(),"stdout":base64::engine::general_purpose::STANDARD.encode(stdout),"stderr":base64::engine::general_purpose::STANDARD.encode(stderr)}),
    )
}
