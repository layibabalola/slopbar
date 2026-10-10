fn main() {
    // The frontend and icons are embedded at compile time; rebuild when they change.
    println!("cargo:rerun-if-changed=../src");
    println!("cargo:rerun-if-changed=icons");
    #[cfg(feature = "desktop")]
    tauri_build::build()
}
