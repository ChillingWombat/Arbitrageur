fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let sysroot_lib =
        std::path::Path::new(&manifest_dir).join("../../.sysroot/usr/lib/x86_64-linux-gnu");
    // Only a Linux target can consume this sysroot, and only when it holds
    // real ELF shared objects. A checkout that crossed a Windows filesystem
    // turns the `.so` symlinks into `IntxLNK` placeholder files, which the
    // linker then parses as object files ("unknown directive: IntxLNK") and
    // fails the link. Emitting the search path in that state is worse than
    // useless — it breaks an otherwise healthy container build.
    if target_os == "linux" && sysroot_is_usable(&sysroot_lib) {
        println!("cargo:rustc-link-search=native={}", sysroot_lib.display());
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", sysroot_lib.display());
    }
    ship_lighter_signer_sidecar(&manifest_dir);
    tauri_build::build()
}

/// True when `dir` holds a coherent set of real ELF shared objects: at least
/// one readable `.so`, and **every** readable `.so` starts with the `\x7fELF`
/// magic. A single `IntxLNK…` placeholder (what a `.so` symlink becomes when
/// the checkout crosses a Windows filesystem) disqualifies the directory —
/// the linker would pick it up for a bare `-l` name and fail with
/// "unknown directive: IntxLNK". Dangling `.so` symlinks are ignored.
fn sysroot_is_usable(dir: &std::path::Path) -> bool {
    use std::io::Read;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    let mut seen = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("so") {
            continue;
        }
        let Ok(mut f) = std::fs::File::open(&path) else {
            if path.is_symlink() && !path.exists() {
                continue;
            }
            return false;
        };
        seen += 1;
        let mut magic = [0u8; 4];
        if f.read_exact(&mut magic).is_err() || &magic != b"\x7fELF" {
            return false;
        }
    }
    seen > 0
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
