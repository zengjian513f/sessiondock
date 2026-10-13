use std::{env, path::PathBuf, process::Command};
fn main() {
    println!("cargo:rerun-if-changed=bpf/io.bpf.c");
    println!("cargo:rerun-if-env-changed=BPF_CLANG");
    println!("cargo:rerun-if-env-changed=BPF_INCLUDE_DIR");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return;
    }
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("io.bpf.o");
    let clang = env::var("BPF_CLANG").unwrap_or_else(|_| "clang".into());
    let mut compiler = Command::new(clang);
    if let Some(paths) = env::var_os("BPF_INCLUDE_DIR") {
        for path in env::split_paths(&paths) {
            compiler.arg("-I").arg(path);
        }
    } else {
        // Debian-family Linux headers put asm/ under the target's multiarch
        // directory. Other distributions expose them in the default include path.
        let triple = env::var("TARGET")
            .unwrap_or_default()
            .replace("-unknown-", "-");
        let headers = PathBuf::from("/usr/include").join(triple);
        if headers.is_dir() {
            compiler.arg("-I").arg(headers);
        }
    }
    let status = compiler
        .args([
            "-target",
            "bpf",
            "-O2",
            "-g",
            "-Wall",
            "-Werror",
            "-c",
            "bpf/io.bpf.c",
            "-o",
        ])
        .arg(output)
        .status()
        .expect("build-time clang required for embedded I/O BPF asset");
    assert!(status.success(), "I/O BPF compilation failed");
}
