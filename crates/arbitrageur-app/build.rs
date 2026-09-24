fn main() {
    ship_lighter_signer_sidecar();
}

/// Copy the vendored Lighter signer next to the produced binary so the
/// adapter's sidecar candidate (exe.parent()/liblighter_signer.so) resolves
/// on machines without a source checkout. The loader verifies the SHA-256
/// before dlopen, so a copy is safe: it is either the approved byte stream
/// or a hard integrity error.
fn ship_lighter_signer_sidecar() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return;
    }
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let src = std::path::Path::new(&manifest_dir)
        .join("../arbitrageur-exchanges/native/lighter/liblighter_signer.so");
    println!("cargo:rerun-if-changed={}", src.display());
    if !src.exists() {
        // Vendored binary is repo-shipped; absent means a source-filtered
        // build (e.g. `cargo package`) — the exe-relative fallback simply
        // won't find a sidecar and the loader reports it at first use.
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
