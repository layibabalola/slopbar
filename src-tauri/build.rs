fn main() {
    use std::process::Command;
    for (name, args) in [
        ("SLOPBAR_BUILD_COMMIT", vec!["rev-parse", "HEAD"]),
        ("SLOPBAR_BUILD_TREE", vec!["rev-parse", "HEAD^{tree}"]),
    ] {
        if let Ok(out) = Command::new("git").args(args).output() {
            let value = String::from_utf8_lossy(&out.stdout).trim().to_owned();
            if out.status.success()
                && value.len() == 40
                && value.bytes().all(|b| b.is_ascii_hexdigit())
            {
                println!("cargo:rustc-env={name}={value}");
            }
        }
    }
    let clean = Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=normal"])
        .output()
        .is_ok_and(|o| o.status.success() && o.stdout.is_empty());
    println!("cargo:rustc-env=SLOPBAR_BUILD_DIRTY={}", !clean);
    // Also rebuild provenance after a commit that doesn't change application source.
    if let Ok(out) = Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .output()
    {
        if out.status.success() {
            let dir = std::path::PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
            println!("cargo:rerun-if-changed={}", dir.join("HEAD").display());
            println!("cargo:rerun-if-changed={}", dir.join("refs").display());
            println!("cargo:rerun-if-changed={}", dir.join("index").display());
        }
    }
    // The frontend and icons are embedded at compile time; rebuild when they change.
    println!("cargo:rerun-if-changed=../src");
    println!("cargo:rerun-if-changed=icons");
    #[cfg(feature = "desktop")]
    {
        // Tauri's dependency tells us the ACTUAL custom-protocol configuration.
        println!(
            "cargo:rustc-env=SLOPBAR_TAURI_DEV={}",
            tauri_build::is_dev()
        );
        tauri_build::build()
    }
}
