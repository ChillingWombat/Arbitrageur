fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let sysroot_lib =
        std::path::Path::new(&manifest_dir).join("../../.sysroot/usr/lib/x86_64-linux-gnu");
    if sysroot_lib.exists() {
        println!("cargo:rustc-link-search=native={}", sysroot_lib.display());
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", sysroot_lib.display());
    }
    tauri_build::build()
}
