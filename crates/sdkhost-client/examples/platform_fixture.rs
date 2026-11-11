//! Synthetic browser-suite driver. Each request is a fresh client process.
use sdkhost_client::{Client, Stream};
use std::{
    io::{self, BufRead, Write},
    path::Path,
};
fn child(mode: &str) -> io::Result<()> {
    println!("ready:{}", std::process::id());
    if mode == "echo" {
        let mixed = std::env::var("SdkHost_Mixed").ok();
        let removed = std::env::var("SdkHost_Removed").ok();
        let expected_mixed = if cfg!(windows) { "new" } else { "old" };
        let expected_removed = if cfg!(windows) {
            None
        } else {
            Some("old".to_string())
        };
        println!(
            "env:override={}",
            mixed.as_deref() == Some(expected_mixed)
                && std::env::var("SDKHOST_MIXED").as_deref() == Ok("new")
        );
        println!(
            "env:removed={}",
            removed == expected_removed && std::env::var_os("SDKHOST_REMOVED").is_none()
        );
        println!(
            "env:unicode={}",
            std::env::var("SDKHOST_FIXTURE").as_deref() == Ok("unicode-中文")
        );
        println!(
            "arg:unicode={}",
            std::env::args().nth(3).as_deref() == Some("参数 中文")
        );
    }
    eprintln!("stderr:ready");
    if mode == "worker" {
        loop {
            println!("worker:alive");
            io::stdout().flush()?;
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
    for line in io::stdin().lock().lines() {
        let line = line?;
        if line == "fork" {
            let mut cmd = std::process::Command::new(std::env::current_exe()?);
            cmd.args(["child", "worker"]);
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                cmd.process_group(0);
            }
            let child = cmd.spawn()?;
            println!("descendant:{}", child.id());
            io::stdout().flush()?;
            return Ok(());
        }
        println!("echo:{line}");
        eprintln!("err:{line}");
        io::stdout().flush()?;
    }
    println!("stdin:eof");
    Ok(())
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("child") {
        child(&args[2]).unwrap();
        return;
    }
    // Before constructing the runtime: no other threads exist in this fixture.
    unsafe {
        std::env::set_var("SdkHost_Mixed", "old");
        std::env::set_var("SdkHost_Removed", "old");
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let result = runtime.block_on(run());
    match result {
        Ok(v) => println!("{}", serde_json::json!({"value":v})),
        Err(e) => println!("{}", serde_json::json!({"error":e.to_string()})),
    }
}
async fn run() -> io::Result<serde_json::Value> {
    let mut bytes = String::new();
    io::Read::read_to_string(&mut io::stdin(), &mut bytes)?;
    let r: serde_json::Value = serde_json::from_str(&bytes)?;
    let root = Path::new(r["root"].as_str().unwrap());
    let key = r["key"].as_str().unwrap();
    let action = r["action"].as_str().unwrap();
    if action == "paths" {
        sdkhost_client::private_dir(root)?;
        let first = root.join(format!("nested-{}", "a".repeat(80)));
        let second = first.join(format!("nested-{}", "b".repeat(80)));
        let third = second.join("control");
        let leaf = third.join("key");
        #[cfg(windows)]
        sdkhost_client::private_dir(&leaf)?;
        // Unix callers create parents explicitly; Windows must create all missing
        // layers itself, then each new layer must independently pass ACL validation.
        for path in [&first, &second, &third, &leaf] {
            sdkhost_client::private_dir(path)?;
        }
        let length = leaf.as_os_str().len();
        std::fs::remove_dir_all(first)?;
        return Ok(serde_json::json!({"path_length": length}));
    }
    let c = if action == "launch" {
        let mut cmd = tokio::process::Command::new(std::env::current_exe()?);
        cmd.args(["child", "echo", "参数 中文"]);
        cmd.env("SDKHOST_MIXED", "new")
            .env_remove("SDKHOST_REMOVED");
        cmd.env("SDKHOST_FIXTURE", "unicode-中文");
        Client::launch_or_attach(root, key, &cmd).await?
    } else {
        Client::attach(root, key).await?
    };
    Ok(match action {
        "launch" | "attach" | "status" => serde_json::to_value(c.status().await?)?,
        "write" => serde_json::to_value(
            c.write(
                r["id"].as_str().unwrap(),
                r["text"].as_str().unwrap().as_bytes(),
            )
            .await?,
        )?,
        "receipt" => serde_json::to_value(c.receipt(r["id"].as_str().unwrap()).await?)?,
        "close" => serde_json::to_value(c.close_stdin(r["id"].as_str().unwrap()).await?)?,
        "stop" => {
            c.signal(9).await?;
            serde_json::Value::Null
        }
        "read" => {
            let stream = if r["stream"] == "stderr" {
                Stream::Stderr
            } else {
                Stream::Stdout
            };
            serde_json::to_value(c.read(stream, r["offset"].as_u64().unwrap_or(0)).await?)?
        }
        "time" => {
            serde_json::to_value(c.received_at(Stream::Stdout, r["offset"].as_u64().unwrap())?)?
        }
        "remove" => {
            c.remove().await?;
            serde_json::Value::Null
        }
        _ => return Err(io::Error::other("unknown fixture action")),
    })
}
