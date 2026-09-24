fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let sysroot_lib =
        std::path::Path::new(&manifest_dir).join("../../.sysroot/usr/lib/x86_64-linux-gnu");
    if sysroot_lib.exists() {
        println!("cargo:rustc-link-search=native={}", sysroot_lib.display());
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", sysroot_lib.display());
    }
    ship_lighter_signer_sidecar(&manifest_dir);
    tauri_build::build()
}

/// Copy the vendored Lighter signer next to the produced binary so the
/// adapter's sidecar candidate (exe.parent()/liblighter_signer.so) resolves
/// on machines without a source checkout. The loader verifies the SHA-256
/// before dlopen, so a copy is safe: it is either the approved byte stream
/// or a hard integrity error.
fn ship_lighter_signer_sidecar(manifest_dir: &str) {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return;
    }
    let src = std::path::Path::new(manifest_dir)
        .join("../../crates/arbitrageur-exchanges/native/lighter/liblighter_signer.so");
    println!("cargo:rerun-if-changed={}", src.display());
    if !src.exists() {
        println!(
            "cargo:warning=vendored liblighter_signer.so not found at {}",
            src.display()
        );
        return;
    }
    // OUT_DIR = target/<profile>/build/<pkg>-<hash>/out → binary dir.
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let bin_dir = std::path::Path::new(&out_dir)
        .ancestors()
        .nth(3)
        .expect("OUT_DIR must be target/<profile>/build/<pkg>/out");
    let dst = bin_dir.join("liblighter_signer.so");
    if let Err(e) = std::fs::copy(&src, &dst) {
        println!(
            "cargo:warning=could not copy Lighter signer sidecar to {}: {e}",
            dst.display()
        );
    }
}
