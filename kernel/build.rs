use std::process::Command;

fn main() {
    let sha = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output().ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=RUOS_GIT_SHA={}", sha);

    let date = Command::new("date")
        .args(["-u", "+%Y-%m-%d"])
        .output().ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=RUOS_BUILD_DATE={}", date);

    println!("cargo:rerun-if-changed=src/");
    println!("cargo:rerun-if-changed=../.git/HEAD");
}
