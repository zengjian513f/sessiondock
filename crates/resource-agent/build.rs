use std::{env, path::PathBuf, process::Command};
fn main() {
    println!("cargo:rerun-if-changed=bpf/io.bpf.c");
    println!("cargo:rerun-if-env-changed=BPF_CLANG");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return;
    }
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("io.bpf.o");
    let clang = env::var("BPF_CLANG").unwrap_or_else(|_| "clang-18".into());
    let status = Command::new(clang)
        .args([
            "-target",
            "bpf",
            "-O2",
            "-g",
            "-Wall",
            "-Werror",
            "-I/usr/include/x86_64-linux-gnu",
            "-c",
            "bpf/io.bpf.c",
            "-o",
        ])
        .arg(output)
        .status()
        .expect("build-time clang required for embedded I/O BPF asset");
    assert!(status.success(), "I/O BPF compilation failed");
}
