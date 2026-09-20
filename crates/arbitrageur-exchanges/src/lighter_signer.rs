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
type FreeFn = unsafe extern "C" fn(*mut c_void);

/// Signed Lighter L2 transaction ready for `POST /api/v1/sendTx`.
#[derive(Debug, Clone)]
pub struct SignedLighterTx {
    pub tx_type: u8,
    pub tx_info: String,
    pub tx_hash: String,
}

/// Candidate locations for `liblighter_signer.so`, in priority order:
/// explicit env override, deployed sidecar next to the executable, then the
/// vendored copy inside the crate (covers `cargo test` and dev runs).
fn candidate_lib_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(p) = std::env::var("LIGHTER_SIGNER_LIB") {
        if !p.trim().is_empty() {
            out.push(PathBuf::from(p));
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            out.push(dir.join("liblighter_signer.so"));
        }
    }
    out.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("native/lighter/liblighter_signer.so"));
    out
}

fn load_library() -> anyhow::Result<libloading::Library> {
    let candidates = candidate_lib_paths();
    let mut tried = Vec::new();
    for path in &candidates {
        // SAFETY: loading a known-vendored signing library; all calls go
        // through narrow, validated wrappers below.
        match unsafe { libloading::Library::new(path) } {
            Ok(lib) => return Ok(lib),
            Err(e) => tried.push(format!("{} ({})", path.display(), e)),
        }
    }
    anyhow::bail!(
        "Lighter signer library not found; tried {}. Set LIGHTER_SIGNER_LIB or ship liblighter_signer.so next to the binary",
        tried.join(", ")
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
        // buffers are reclaimed immediately after the call returns.
        drop(CString::from_raw(url_raw));
        drop(CString::from_raw(key_raw));
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
        // buffers are reclaimed immediately after the call returns.
        drop(CString::from_raw(url_raw));
        drop(CString::from_raw(key_raw));
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

impl LoadedLib {
    fn load_from_hint(_hint: Option<&str>) -> anyhow::Result<Self> {
        // Reserved for per-account library selection; the standard search
        // order applies for now.
        Self::load()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signer_lib_loads_from_vendored_copy() {
        let lib = LoadedLib::load().expect("vendored liblighter_signer.so must load");
        // GenerateAPIKey exercises the library without any credentials.
        unsafe {
            let gen: libloading::Symbol<unsafe extern "C" fn() -> ApiKeyResponse> =
                lib._lib.get(b"GenerateAPIKey").expect("GenerateAPIKey symbol");
            let r = gen();
            assert!(
                r.err.is_null()
                    || CStr::from_ptr(r.err as *const c_char).to_bytes().is_empty(),
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
        let key = "0x0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
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
        let v: serde_json::Value =
            serde_json::from_str(&tx.tx_info).expect("tx_info is JSON");
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
        let v2: serde_json::Value =
            serde_json::from_str(&tx2.tx_info).expect("tx_info is JSON");
        for field in ["MarketIndex", "BaseAmount", "ApiKeyIndex", "AccountIndex", "Nonce"] {
            assert_eq!(v2.get(field), v.get(field), "field {field} stable");
        }
    }

    #[test]
    fn sign_cancel_order_round_trip_local() {
        let key = "0x0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
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
}
