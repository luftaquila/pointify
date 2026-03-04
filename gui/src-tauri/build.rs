fn main() {
    let version = std::process::Command::new("git")
        .args(["describe", "--tags", "--abbrev=0", "--always"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default();
    println!("cargo:rustc-env=GIT_VERSION={}", version.trim());
    tauri_build::build();
}
