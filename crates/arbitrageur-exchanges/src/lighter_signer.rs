//! FFI bindings to the Lighter signing library (`liblighter_signer.so`).
//!
//! Lighter orders use a ZK-friendly signature scheme (Schnorr over the
//! Goldilocks-quintic curve with Poseidon2 hashing) for which no Rust crate
//! exists. Signing is therefore delegated to the reference Go implementation
//! (`github.com/elliottech/lighter-go`, Apache-2.0), compiled as a C-shared
//! library and vendored under `native/lighter/`.
//!
//! The library is loaded at runtime with `libloading` so a missing library
//! is a clean fail-closed error — never a build break and never a fabricated
//! signature. Only pure-local signing calls are used here (`CreateClient`
//! with explicit parameters, `SignCreateOrder`, `SignCancelOrder`); nonces
//! are fetched and transactions submitted over HTTP by the adapter, exactly
//! like the official Python SDK (`sendTx` form POST).

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_uchar, c_void};
use std::path::PathBuf;
use std::sync::OnceLock;

use parking_lot::Mutex;

#[repr(C)]
struct SignedTxResponse {
    tx_type: c_uchar,
    tx_info: *mut c_char,
    tx_hash: *mut c_char,
    message_to_sign: *mut c_char,
    err: *mut c_char,
}

#[repr(C)]
struct StrOrErr {
    str: *mut c_char,
    err: *mut c_char,
}

type CreateClientFn =
    unsafe extern "C" fn(*mut c_char, *mut c_char, c_int, c_int, i64) -> *mut c_char;
#[allow(clippy::too_many_arguments)]
type SignCreateOrderFn = unsafe extern "C" fn(
    c_int,
    i64,
    i64,
    c_int,
    c_int,
    c_int,
    c_int,
    c_int,
    c_int,
    i64,
    i64,
    c_int,
    c_int,
    c_uchar,
    c_uchar,
    c_uchar,
    i64,
    c_int,
    i64,
) -> SignedTxResponse;
type SignCancelOrderFn =
    unsafe extern "C" fn(c_int, i64, c_uchar, i64, c_int, i64) -> SignedTxResponse;
type CreateAuthTokenFn = unsafe extern "C" fn(i64, c_int, i64) -> StrOrErr;
type FreeFn = unsafe extern "C" fn(*mut c_void);

/// Signed Lighter L2 transaction ready for `POST /api/v1/sendTx`.
#[derive(Debug, Clone)]
pub struct SignedLighterTx {
    pub tx_type: u8,
    pub tx_info: String,
    pub tx_hash: String,
}

/// Approved SHA-256 digest of the vendored `liblighter_signer.so`.
/// Only a byte-identical library may be loaded; anything else is a hard
/// integrity error, never a fallback.
const APPROVED_LIGHTER_SIGNER_SHA256: &str =
    "8cb2fbcadab0b9b3fed4a524998b825818b677fcf9c49bc1f18b4ca02a233753";

fn sha256_hex_reader(reader: impl std::io::Read, path: &std::path::Path) -> anyhow::Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut reader = std::io::BufReader::new(reader);
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| anyhow::anyhow!("Cannot read {} for hashing: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

#[allow(dead_code)] // exercised by tests; non-unix verify path
fn sha256_hex(path: &std::path::Path) -> anyhow::Result<String> {
    let file = std::fs::File::open(path)
        .map_err(|e| anyhow::anyhow!("Cannot read {} for hashing: {e}", path.display()))?;
    sha256_hex_reader(file, path)
}

/// Result of digest verification: the path `dlopen` should be pointed at
/// plus a guard keeping the hashed inode pinned open until dlopen has
/// consumed it — closes the hash→dlopen TOCTOU (review §M-6). On unix the
/// target is `/proc/self/fd/N`, so a rename or replace of the on-disk path
/// between hashing and loading cannot swap in a different file. Elsewhere
/// the digest check and the open stay path-based.
struct VerifiedSigner {
    /// The open fd the digest was computed over — held purely for its
    /// pinning side-effect; dropped after `dlopen` returns.
    _guard: Option<std::fs::File>,
    target: String,
}

/// Validate `expected_hex`, verify `path`'s SHA-256 against it, and return
/// a guard plus the path `dlopen` should be pointed at. On unix the guard
/// is the open fd the hash was computed over and the target is
/// `/proc/self/fd/N` — the same inode, pinned. Elsewhere the digest check
/// and the open stay path-based.
fn verify_signer_digest(
    path: &std::path::Path,
    expected_hex: &str,
) -> anyhow::Result<VerifiedSigner> {
    let expected = expected_hex.trim();
    if expected.len() != 64 || !expected.chars().all(|c| c.is_ascii_hexdigit()) {
        anyhow::bail!(
            "Invalid expected SHA-256 digest '{expected_hex}': must be exactly 64 hex chars"
        );
    }
    let expected_lower = expected.to_lowercase();
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let file = std::fs::File::open(path)
            .map_err(|e| anyhow::anyhow!("Cannot read {} for hashing: {e}", path.display()))?;
        let actual = sha256_hex_reader(&file, path)?;
        if actual != expected_lower {
            anyhow::bail!(
                "Lighter signer integrity mismatch for {}: expected {expected_lower}, got {actual}",
                path.display()
            );
        }
        let target = format!("/proc/self/fd/{}", file.as_raw_fd());
        Ok(VerifiedSigner {
            _guard: Some(file),
            target,
        })
    }
    #[cfg(not(unix))]
    {
        let actual = sha256_hex(path)?;
        if actual != expected_lower {
            anyhow::bail!(
                "Lighter signer integrity mismatch for {}: expected {expected_lower}, got {actual}",
                path.display()
            );
        }
        Ok(VerifiedSigner {
            _guard: None,
            target: path.display().to_string(),
        })
    }
}

/// Candidate locations for `liblighter_signer.so`, in priority order:
/// explicit env override, deployed sidecar next to the executable, then the
/// vendored copy inside the crate (covers `cargo test` and dev runs).
/// The flag marks the explicit `LIGHTER_SIGNER_LIB` override, which
/// `load_library` verifies against `LIGHTER_SIGNER_SHA256` instead of the
/// approved constant and never falls back from.
/// Platform-native file name for the vendored signer library. The digest
/// check makes the name cosmetic for *which* bytes load, but a wrong
/// extension means the sidecar candidate never resolves on non-Linux
/// targets, so the loader must look for the file the OS actually uses.
fn signer_lib_filename() -> &'static str {
    if cfg!(target_os = "macos") {
        "liblighter_signer.dylib"
    } else if cfg!(target_os = "windows") {
        "lighter_signer.dll"
    } else {
        "liblighter_signer.so"
    }
}

fn candidate_lib_paths() -> Vec<(PathBuf, bool)> {
    let mut out = Vec::new();
    if let Ok(p) = std::env::var("LIGHTER_SIGNER_LIB") {
        if !p.trim().is_empty() {
            out.push((PathBuf::from(p), true));
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            out.push((dir.join(signer_lib_filename()), false));
        }
    }
    out.push((
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("native/lighter")
            .join(signer_lib_filename()),
        false,
    ));
    out
}

fn load_library() -> anyhow::Result<libloading::Library> {
    let candidates = candidate_lib_paths();
    // Explicit override: verify against the caller-supplied digest and never
    // fall back to sidecar/vendored on any failure.
    if let Some((env_path, _)) = candidates.iter().find(|(_, is_override)| *is_override) {
        let expected = std::env::var("LIGHTER_SIGNER_SHA256").map_err(|_| {
            anyhow::anyhow!("LIGHTER_SIGNER_LIB override requires LIGHTER_SIGNER_SHA256 to be set")
        })?;
        let expected = expected.trim().to_string();
        if expected.len() != 64 || !expected.chars().all(|c| c.is_ascii_hexdigit()) {
            anyhow::bail!("LIGHTER_SIGNER_SHA256 must be exactly 64 hex chars");
        }
        let verified = verify_signer_digest(env_path, &expected)?;
        // SAFETY: digest-verified explicit override loaded through the
        // pinned fd; all calls go through narrow, validated wrappers below.
        return unsafe { libloading::Library::new(&verified.target) }.map_err(|e| {
            anyhow::anyhow!(
                "Failed to load LIGHTER_SIGNER_LIB override {}: {e}",
                env_path.display()
            )
        });
    }
    let mut tried = Vec::new();
    for (path, _) in candidates.iter().filter(|(_, is_override)| !is_override) {
        if !path.exists() {
            tried.push(format!("{} (missing)", path.display()));
            continue;
        }
        // Hard integrity error on mismatch: never load, never fall through
        // to the next candidate.
        let verified = verify_signer_digest(path, APPROVED_LIGHTER_SIGNER_SHA256)?;
        // SAFETY: digest matches the approved vendored library, loaded
        // through the pinned fd; all calls go through narrow, validated
        // wrappers below.
        match unsafe { libloading::Library::new(&verified.target) } {
            Ok(lib) => return Ok(lib),
            Err(e) => tried.push(format!("{} ({})", path.display(), e)),
        }
    }
    anyhow::bail!(
        "Lighter signer library not found; tried {}. Set LIGHTER_SIGNER_LIB or ship {} next to the binary",
        tried.join(", "),
        signer_lib_filename()
    )
}

/// Process-wide serialisation of signer calls. Order submission already
/// serialises per adapter, but the Go client registry is global, so belt
/// and braces here keeps concurrent venue pairs from interleaving FFI calls.
fn signer_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

struct LoadedLib {
    _lib: libloading::Library,
    create_client: CreateClientFn,
    sign_create_order: SignCreateOrderFn,
    sign_cancel_order: SignCancelOrderFn,
    create_auth_token: CreateAuthTokenFn,
    free_ptr: FreeFn,
}

impl LoadedLib {
    fn load() -> anyhow::Result<Self> {
        let lib = load_library()?;
        // SAFETY: symbol signatures mirror lighter_signer.h (vendored next
        // to the .so); mismatches would be memory-unsafe, so each lookup
        // failure is a hard error rather than a fallback.
        unsafe {
            Ok(Self {
                create_client: *lib.get(b"CreateClient")?,
                sign_create_order: *lib.get(b"SignCreateOrder")?,
                sign_cancel_order: *lib.get(b"SignCancelOrder")?,
                create_auth_token: *lib.get(b"CreateAuthToken")?,
                free_ptr: *lib.get(b"Free")?,
                _lib: lib,
            })
        }
    }

    /// Copy a Go-allocated C string into Rust and free the original.
    ///
    /// SAFETY: `ptr` must be null or a `C.CString` allocation owned by the
    /// signer library (freed with its own `Free`, i.e. C `free`).
    unsafe fn take_string(&self, ptr: *mut c_char) -> Option<String> {
        if ptr.is_null() {
            return None;
        }
        let s = CStr::from_ptr(ptr as *const c_char)
            .to_string_lossy()
            .into_owned();
        (self.free_ptr)(ptr as *mut c_void);
        Some(s)
    }

    unsafe fn take_response(&self, r: SignedTxResponse) -> anyhow::Result<SignedLighterTx> {
        let err = self.take_string(r.err);
        let tx_info = self.take_string(r.tx_info);
        let tx_hash = self.take_string(r.tx_hash);
        // message_to_sign only applies to L1-signed flows; always released.
        self.take_string(r.message_to_sign);
        if let Some(e) = err {
            if !e.trim().is_empty() {
                anyhow::bail!("Lighter signer rejected request: {}", e);
            }
        }
        match (tx_info, tx_hash) {
            (Some(info), Some(hash)) if !info.is_empty() && !hash.is_empty() => {
                Ok(SignedLighterTx {
                    tx_type: r.tx_type,
                    tx_info: info,
                    tx_hash: hash,
                })
            }
            _ => anyhow::bail!("Lighter signer returned an empty signed transaction"),
        }
    }
}

/// Parameters for a Lighter limit/market order signature.
#[allow(clippy::too_many_arguments)]
pub struct CreateOrderParams {
    pub market_index: i64,
    pub client_order_index: i64,
    pub base_amount: i64,
    pub price: u32,
    pub is_ask: bool,
    /// 0 = limit, 1 = market.
    pub order_type: u8,
    /// 0 = IOC, 1 = GTT, 2 = post-only.
    pub time_in_force: u8,
    pub reduce_only: bool,
    /// -1 = 28-day default (limit/GTT), 0 = IOC.
    pub order_expiry_ms: i64,
    pub nonce: i64,
    pub api_key_index: i64,
    pub account_index: i64,
}

/// Sign a Lighter L2 create-order transaction locally (no network).
///
/// Reclaim a `CString` allocation whose bytes must not linger: zero the
/// buffer (including the NUL) before freeing so key material does not
/// survive in the heap after drop.
///
/// SAFETY: `ptr` must be a pointer previously produced by
/// `CString::into_raw` (or null), and must not be used afterwards.
unsafe fn reclaim_cstring(ptr: *mut c_char) {
    if ptr.is_null() {
        return;
    }
    let len = CStr::from_ptr(ptr).to_bytes().len();
    std::ptr::write_bytes(ptr as *mut u8, 0, len + 1);
    drop(CString::from_raw(ptr));
}

/// `api_key_private_key` is the hex API-key secret (with or without `0x`);
/// `base_url` selects the deployment the client handle binds to.
#[allow(clippy::too_many_arguments)]
pub fn sign_create_order(
    lib_path_hint: Option<&str>,
    base_url: &str,
    chain_id: u32,
    api_key_private_key: &str,
    params: &CreateOrderParams,
) -> anyhow::Result<SignedLighterTx> {
    let _guard = signer_lock().lock();
    let lib = LoadedLib::load_from_hint(lib_path_hint)?;
    // SAFETY: argument pointers are short-lived CStrings copied by the
    // library during the call; integers are range-checked by the
    // library itself (bad values come back as `err`, not UB).
    unsafe {
        let url = CString::new(base_url.trim())?;
        let key = CString::new(api_key_private_key.trim())?;
        let url_raw = url.into_raw();
        let key_raw = key.into_raw();
        let err = (lib.create_client)(
            url_raw,
            key_raw,
            chain_id as c_int,
            params.api_key_index as c_int,
            params.account_index,
        );
        // CreateClient copies both strings synchronously (GoString), so the
        // buffers are reclaimed immediately after the call returns — the
        // key buffer is zeroized first.
        drop(CString::from_raw(url_raw));
        reclaim_cstring(key_raw);
        let err_str = if err.is_null() {
            None
        } else {
            let s = CStr::from_ptr(err as *const c_char)
                .to_string_lossy()
                .into_owned();
            (lib.free_ptr)(err as *mut c_void);
            Some(s)
        };
        if let Some(e) = err_str {
            if !e.trim().is_empty() {
                anyhow::bail!("Lighter signer CreateClient failed: {}", e);
            }
        }
        let resp = (lib.sign_create_order)(
            params.market_index as c_int,
            params.client_order_index,
            params.base_amount,
            params.price as c_int,
            params.is_ask as c_int,
            params.order_type as c_int,
            params.time_in_force as c_int,
            params.reduce_only as c_int,
            0,
            params.order_expiry_ms,
            0,
            0,
            0,
            0,
            0,
            0,
            params.nonce,
            params.api_key_index as c_int,
            params.account_index,
        );
        lib.take_response(resp)
    }
}

/// Sign a Lighter L2 cancel-order transaction locally (no network).
#[allow(clippy::too_many_arguments)]
pub fn sign_cancel_order(
    lib_path_hint: Option<&str>,
    base_url: &str,
    chain_id: u32,
    api_key_private_key: &str,
    market_index: i64,
    order_index: i64,
    nonce: i64,
    api_key_index: i64,
    account_index: i64,
) -> anyhow::Result<SignedLighterTx> {
    let _guard = signer_lock().lock();
    let lib = LoadedLib::load_from_hint(lib_path_hint)?;
    unsafe {
        let url = CString::new(base_url.trim())?;
        let key = CString::new(api_key_private_key.trim())?;
        let url_raw = url.into_raw();
        let key_raw = key.into_raw();
        let err = (lib.create_client)(
            url_raw,
            key_raw,
            chain_id as c_int,
            api_key_index as c_int,
            account_index,
        );
        // CreateClient copies both strings synchronously (GoString), so the
        // buffers are reclaimed immediately after the call returns — the
        // key buffer is zeroized first.
        drop(CString::from_raw(url_raw));
        reclaim_cstring(key_raw);
        let err_str = if err.is_null() {
            None
        } else {
            let s = CStr::from_ptr(err as *const c_char)
                .to_string_lossy()
                .into_owned();
            (lib.free_ptr)(err as *mut c_void);
            Some(s)
        };
        if let Some(e) = err_str {
            if !e.trim().is_empty() {
                anyhow::bail!("Lighter signer CreateClient failed: {}", e);
            }
        }
        let resp = (lib.sign_cancel_order)(
            market_index as c_int,
            order_index,
            0,
            nonce,
            api_key_index as c_int,
            account_index,
        );
        lib.take_response(resp)
    }
}

/// Create a Lighter API auth token locally (no network). The token is the
/// `authorization` header value for authenticated indexer endpoints such
/// as `/api/v1/accountOrders`.
///
/// `deadline_unix` is the token's expiry as seconds since epoch; callers
/// should pass a short horizon (minutes, not days) so a leaked token is
/// worthless quickly.
#[allow(clippy::too_many_arguments)]
pub fn create_auth_token(
    lib_path_hint: Option<&str>,
    base_url: &str,
    chain_id: u32,
    api_key_private_key: &str,
    deadline_unix: i64,
    api_key_index: i64,
    account_index: i64,
) -> anyhow::Result<String> {
    let _guard = signer_lock().lock();
    let lib = LoadedLib::load_from_hint(lib_path_hint)?;
    unsafe {
        let url = CString::new(base_url.trim())?;
        let key = CString::new(api_key_private_key.trim())?;
        let url_raw = url.into_raw();
        let key_raw = key.into_raw();
        let err = (lib.create_client)(
            url_raw,
            key_raw,
            chain_id as c_int,
            api_key_index as c_int,
            account_index,
        );
        drop(CString::from_raw(url_raw));
        reclaim_cstring(key_raw);
        let err_str = if err.is_null() {
            None
        } else {
            let s = CStr::from_ptr(err as *const c_char)
                .to_string_lossy()
                .into_owned();
            (lib.free_ptr)(err as *mut c_void);
            Some(s)
        };
        if let Some(e) = err_str {
            if !e.trim().is_empty() {
                anyhow::bail!("Lighter signer CreateClient failed: {}", e);
            }
        }
        let resp = (lib.create_auth_token)(deadline_unix, api_key_index as c_int, account_index);
        let err = lib.take_string(resp.err);
        let token = lib.take_string(resp.str);
        if let Some(e) = err {
            if !e.trim().is_empty() {
                anyhow::bail!("Lighter signer CreateAuthToken failed: {}", e);
            }
        }
        let token = token
            .filter(|t| !t.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("Lighter signer returned an empty auth token"))?;
        Ok(token)
    }
}

impl LoadedLib {
    fn load_from_hint(_hint: Option<&str>) -> anyhow::Result<&'static Self> {
        // Reserved for per-account library selection; the standard search
        // order applies for now.
        shared_lib()
    }
}

/// Process-wide shared signer handle.
///
/// A Go `-buildmode=c-shared` library must never be `dlclose`d: its runtime
/// threads, timers, and async-preemption signal handlers keep executing on
/// unmapped code after unload, corrupting the host heap — observed here as
/// delayed `corrupted double-linked list` aborts minutes after the last
/// signing call (the previous implementation `dlopen`ed per call and let
/// `libloading::Library` drop/unload at scope end). The handle is loaded
/// once and intentionally leaked: the Go runtime stays mapped for the
/// process lifetime.
fn shared_lib() -> anyhow::Result<&'static LoadedLib> {
    static LIB: OnceLock<anyhow::Result<LoadedLib>> = OnceLock::new();
    LIB.get_or_init(LoadedLib::load)
        .as_ref()
        .map_err(|e| anyhow::anyhow!("{e:#}"))
}

// Every test here loads or hashes the vendored Linux `.so`, which is the
// only platform the signer is published for. Gate the module so a
// `cargo test` on macOS/Windows stays green instead of failing to find a
// library that was never shipped for those targets.
#[cfg(target_os = "linux")]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signer_lib_loads_from_vendored_copy() {
        // Route through the process-wide handle: a Go c-shared lib must
        // never be dlclosed (orphaned runtime threads corrupt the heap).
        let lib = shared_lib().expect("vendored liblighter_signer.so must load");
        // GenerateAPIKey exercises the library without any credentials.
        unsafe {
            let gen: libloading::Symbol<unsafe extern "C" fn() -> ApiKeyResponse> = lib
                ._lib
                .get(b"GenerateAPIKey")
                .expect("GenerateAPIKey symbol");
            let r = gen();
            assert!(
                r.err.is_null() || CStr::from_ptr(r.err as *const c_char).to_bytes().is_empty(),
                "GenerateAPIKey must succeed"
            );
            let privk = CStr::from_ptr(r.privateKey as *const c_char)
                .to_string_lossy()
                .into_owned();
            let pubk = CStr::from_ptr(r.publicKey as *const c_char)
                .to_string_lossy()
                .into_owned();
            (lib.free_ptr)(r.privateKey as *mut c_void);
            (lib.free_ptr)(r.publicKey as *mut c_void);
            if !r.err.is_null() {
                (lib.free_ptr)(r.err as *mut c_void);
            }
            assert!(privk.starts_with("0x"), "privkey hex: {privk}");
            assert!(pubk.starts_with("0x"), "pubkey hex: {pubk}");
            assert_eq!(privk.len(), 82, "40-byte scalar hex");
            assert_eq!(pubk.len(), 82, "40-byte pubkey hex");
        }
    }

    #[test]
    fn sign_create_order_round_trip_local() {
        // Pure-local signing: fixed key, explicit nonce, no network.
        let key =
            "0x0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let params = CreateOrderParams {
            market_index: 1,
            client_order_index: 7,
            base_amount: 1000,
            price: 50000,
            is_ask: false,
            order_type: 0,
            time_in_force: 1,
            reduce_only: false,
            order_expiry_ms: 4102444800000,
            nonce: 42,
            api_key_index: 2,
            account_index: 11111,
        };
        let tx = sign_create_order(
            None,
            "https://mainnet.zklighter.elliot.ai",
            304,
            key,
            &params,
        )
        .expect("local signing must succeed");
        assert_eq!(tx.tx_type, 14, "create-order tx type");
        assert!(!tx.tx_hash.is_empty());
        let v: serde_json::Value = serde_json::from_str(&tx.tx_info).expect("tx_info is JSON");
        assert_eq!(v.get("MarketIndex").and_then(|m| m.as_i64()), Some(1));
        assert_eq!(v.get("BaseAmount").and_then(|m| m.as_i64()), Some(1000));
        assert_eq!(v.get("ApiKeyIndex").and_then(|m| m.as_u64()), Some(2));
        // A repeat signing must also succeed with identical request fields
        // (only the signature / signed hash / ms-resolution expiry may move).
        let tx2 = sign_create_order(
            None,
            "https://mainnet.zklighter.elliot.ai",
            304,
            key,
            &params,
        )
        .expect("repeat signing must succeed");
        assert_eq!(tx2.tx_type, tx.tx_type);
        let v2: serde_json::Value = serde_json::from_str(&tx2.tx_info).expect("tx_info is JSON");
        for field in [
            "MarketIndex",
            "BaseAmount",
            "ApiKeyIndex",
            "AccountIndex",
            "Nonce",
        ] {
            assert_eq!(v2.get(field), v.get(field), "field {field} stable");
        }
    }

    #[test]
    fn sign_cancel_order_round_trip_local() {
        let key =
            "0x0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let tx = sign_cancel_order(
            None,
            "https://mainnet.zklighter.elliot.ai",
            304,
            key,
            1,
            7,
            43,
            2,
            11111,
        )
        .expect("local cancel signing must succeed");
        assert_eq!(tx.tx_type, 15, "cancel-order tx type");
        assert!(!tx.tx_hash.is_empty());
    }

    #[repr(C)]
    #[allow(non_snake_case)]
    struct ApiKeyResponse {
        privateKey: *mut c_char,
        publicKey: *mut c_char,
        err: *mut c_char,
    }

    #[test]
    fn vendored_signer_matches_approved_digest() {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("native/lighter/liblighter_signer.so");
        let actual = sha256_hex(&path).expect("vendored .so must hash");
        assert_eq!(actual, APPROVED_LIGHTER_SIGNER_SHA256);
    }

    #[test]
    fn modified_signer_copy_is_rejected() {
        let src =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("native/lighter/liblighter_signer.so");
        let bytes = std::fs::read(&src).expect("read vendored .so");
        let mut tmp = std::env::temp_dir();
        tmp.push(format!(
            "lighter_signer_modified_{}_{}.so",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let mut mutated = bytes;
        let last = mutated.len() - 1;
        mutated[last] ^= 0x01;
        std::fs::write(&tmp, &mutated).expect("write temp copy");
        let r = verify_signer_digest(&tmp, APPROVED_LIGHTER_SIGNER_SHA256);
        let _ = std::fs::remove_file(&tmp);
        assert!(r.is_err(), "flipped-byte copy must be rejected");
    }
}
