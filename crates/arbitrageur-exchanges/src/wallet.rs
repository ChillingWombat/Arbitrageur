//! Local EVM wallet signing for venues that expose **no API-key trading**.
//!
//! # Why this module exists
//!
//! Some venues (Variational Omni, PopDEX beta) have no REST trading API at
//! all: Variational settles RFQ quotes on-chain on Arbitrum, and PopDEX is a
//! closed-beta CLOB whose frontend proxies signed requests through its own
//! backend. The only secure way to trade on such venues is the same way
//! their own frontends do it — hold the user's EVM key locally, sign
//! transactions/quotes **in-process**, and submit them to chain RPC.
//!
//! # Security contract (read before touching this file)
//!
//! - Private key material enters as a hex string (from the encrypted vault),
//!   is parsed once into `Zeroizing<[u8; 32]>`, and is **never** stored as
//!   `String`, logged, or included in `Debug` output.
//! - Signing happens in-process via `k256` (secp256k1). The key never leaves
//!   the host; only signatures and the derived address cross the network.
//! - `sign_personal_message` implements EIP-191 (`"\x19Ethereum Signed
//!   Message:\n" + len + msg`) so signatures are not replayable as raw
//!   transactions. `sign_digest` is exposed for EIP-712 flows where the
//!   caller already built the typed-data digest.
//! - Execution against no-API venues is still **fail-closed** until each
//!   venue's RFQ/contract flow is implemented and audited. Use
//!   [`wallet_execution_unavailable`] for the standard refusal message.

#[cfg(test)]
use k256::ecdsa::VerifyingKey;
use k256::ecdsa::{RecoveryId, SigningKey};
use sha3::{Digest, Keccak256};
use zeroize::{Zeroize, Zeroizing};

/// Parse a `0x`-prefixed (or bare) hex EVM private key into zeroizing bytes.
fn parse_secret_hex(hex_key: &str) -> anyhow::Result<Zeroizing<[u8; 32]>> {
    let clean = hex_key.trim().strip_prefix("0x").unwrap_or(hex_key.trim());
    let mut bytes =
        hex::decode(clean).map_err(|e| anyhow::anyhow!("Invalid EVM private key hex: {e}"))?;
    if bytes.len() != 32 {
        bytes.zeroize();
        anyhow::bail!("Invalid EVM private key length: expected 32 bytes");
    }
    let mut secret = Zeroizing::new([0u8; 32]);
    secret.copy_from_slice(&bytes);
    bytes.zeroize();
    // Validate curve membership eagerly so a bad key fails at load time.
    // The raw string is never retained — only the zeroizing bytes.
    let key = SigningKey::from_bytes((&*secret).into())
        .map_err(|e| anyhow::anyhow!("EVM private key is not a valid secp256k1 scalar: {e}"))?;
    drop(key);
    Ok(secret)
}

fn keccak256(data: &[u8]) -> [u8; 32] {
    let mut hasher = Keccak256::new();
    hasher.update(data);
    hasher.finalize().into()
}

/// Derive the checksummed-lowercase `0x` address from an uncompressed
/// secp256k1 public key (64 bytes, X||Y without the `0x04` prefix).
pub fn eth_address_from_pubkey_uncompressed(pubkey: &[u8; 64]) -> [u8; 20] {
    let hash = keccak256(pubkey);
    let mut addr = [0u8; 20];
    addr.copy_from_slice(&hash[12..]);
    addr
}

/// Local EVM wallet: holds key bytes (zeroized on drop) plus the derived
/// address. Cheap to clone — clones re-derive nothing and copy only the
/// secret bytes + address.
pub struct EvmWalletSigner {
    secret: Zeroizing<[u8; 32]>,
    address: [u8; 20],
}

impl EvmWalletSigner {
    /// Load a wallet from a vault-held hex private key. Fails closed on bad
    /// hex, wrong length, or non-scalar values — the raw string is never
    /// retained.
    pub fn from_hex(hex_key: &str) -> anyhow::Result<Self> {
        let secret = parse_secret_hex(hex_key)?;
        let key = SigningKey::from_bytes((&*secret).into())
            .map_err(|e| anyhow::anyhow!("EVM private key is not a valid secp256k1 scalar: {e}"))?;
        let uncompressed = key.verifying_key().to_sec1_point(false);
        let mut raw = [0u8; 64];
        raw.copy_from_slice(&uncompressed.as_bytes()[1..65]);
        let address = eth_address_from_pubkey_uncompressed(&raw);
        Ok(Self { secret, address })
    }

    /// Raw 20-byte address.
    pub fn address_bytes(&self) -> [u8; 20] {
        self.address
    }

    /// Compressed (SEC1) 33-byte secp256k1 public key — Cosmos-style
    /// `secp256k1.PubKey` payloads use this form.
    pub fn compressed_pubkey(&self) -> anyhow::Result<[u8; 33]> {
        let key = self.signing_key()?;
        let bytes = key.verifying_key().to_sec1_bytes();
        bytes
            .as_ref()
            .try_into()
            .map_err(|_| anyhow::anyhow!("secp256k1 pubkey is not the 33-byte compressed form"))
    }

    /// Lowercase `0x` address string (suitable for API fields / RPC `from`).
    pub fn address_hex(&self) -> String {
        format!("0x{}", hex::encode(self.address))
    }

    /// EIP-55 checksummed `0x` address — the mixed-case form venue UIs show,
    /// so an operator can eyeball the signer against an agent list.
    pub fn address_checksum(&self) -> String {
        let lower = hex::encode(self.address);
        let hash = keccak256(lower.as_bytes());
        let mut out = String::with_capacity(42);
        out.push_str("0x");
        for (i, c) in lower.chars().enumerate() {
            if c.is_ascii_digit() {
                out.push(c);
            } else {
                let nibble = (hash[i / 2] >> (if i % 2 == 0 { 4 } else { 0 })) & 0xf;
                out.push(if nibble >= 8 {
                    c.to_ascii_uppercase()
                } else {
                    c
                });
            }
        }
        out
    }

    fn signing_key(&self) -> anyhow::Result<SigningKey> {
        SigningKey::from_bytes((&*self.secret).into())
            .map_err(|e| anyhow::anyhow!("EVM signing key reconstruction failed: {e}"))
    }

    /// Sign a 32-byte digest, returning the 65-byte `{r || s || v}` signature
    /// with `v` in `{27, 28}` (Ethereum convention).
    pub fn sign_digest(&self, digest: &[u8; 32]) -> anyhow::Result<[u8; 65]> {
        let key = self.signing_key()?;
        let (sig, recid): (k256::ecdsa::Signature, RecoveryId) =
            key.sign_prehash_recoverable(digest);
        let mut out = [0u8; 65];
        out[..32].copy_from_slice(&sig.r().to_bytes());
        out[32..64].copy_from_slice(&sig.s().to_bytes());
        out[64] = recid.to_byte() + 27;
        Ok(out)
    }

    /// Sign an arbitrary message with the EIP-191 personal-sign prefix.
    pub fn sign_personal_message(&self, message: &[u8]) -> anyhow::Result<[u8; 65]> {
        let prefix = format!("\x19Ethereum Signed Message:\n{}", message.len());
        let mut buf = Vec::with_capacity(prefix.len() + message.len());
        buf.extend_from_slice(prefix.as_bytes());
        buf.extend_from_slice(message);
        let digest = keccak256(&buf);
        self.sign_digest(&digest)
    }
}

impl Drop for EvmWalletSigner {
    fn drop(&mut self) {
        self.secret.zeroize();
        self.address.zeroize();
    }
}

// Manual `Debug` — must never leak key material.
impl std::fmt::Debug for EvmWalletSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EvmWalletSigner")
            .field("address", &self.address_hex())
            .field("has_key", &true)
            .finish()
    }
}

/// Standard fail-closed error for order paths on venues with no API-key
/// trading. Names the required wallet/contract flow so the UI can surface an
/// actionable message instead of a bare "not implemented".
pub fn wallet_execution_unavailable(venue: &str, chain: &str, flow: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "{venue} exposes no API-key trading; orders require local wallet signing + on-chain settlement on {chain} ({flow}). \
         Live execution stays disabled until that flow is implemented and audited — refusing to fabricate a fill. \
         To prepare, store the venue's EVM private key in Exchange Accounts; the key never leaves this host."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // Scalar 1 — canonical hover vector (address 0x7E5F...5Bdf); holds no funds.
    const TEST_KEY: &str = "0000000000000000000000000000000000000000000000000000000000000001";
    const TEST_ADDR: &str = "0x7e5f4552091a69125d5dfcb7b8c2659029395bdf";

    #[test]
    fn derives_known_address() {
        let w = EvmWalletSigner::from_hex(TEST_KEY).unwrap();
        assert_eq!(w.address_hex(), TEST_ADDR);
        let w2 = EvmWalletSigner::from_hex(&format!("0x{TEST_KEY}")).unwrap();
        assert_eq!(w2.address_hex(), TEST_ADDR);
    }

    #[test]
    fn rejects_bad_keys_without_retaining() {
        assert!(EvmWalletSigner::from_hex("not-hex").is_err());
        assert!(EvmWalletSigner::from_hex("0x1234").is_err());
        assert!(EvmWalletSigner::from_hex(
            "0x0000000000000000000000000000000000000000000000000000000000000000"
        )
        .is_err());
    }

    #[test]
    fn personal_sign_roundtrips_via_ecrecover() {
        let w = EvmWalletSigner::from_hex(TEST_KEY).unwrap();
        let msg = b"arbitrageur venue binding";
        let sig = w.sign_personal_message(msg).unwrap();
        assert!([27u8, 28u8].contains(&sig[64]));

        // Recompute the EIP-191 digest and recover the signer.
        let prefix = format!("\x19Ethereum Signed Message:\n{}", msg.len());
        let mut buf = Vec::new();
        buf.extend_from_slice(prefix.as_bytes());
        buf.extend_from_slice(msg);
        let digest = keccak256(&buf);
        let recid = RecoveryId::from_byte(sig[64] - 27).unwrap();
        let recovered = VerifyingKey::recover_from_prehash(
            &digest,
            &k256::ecdsa::Signature::from_slice(&sig[..64]).unwrap(),
            recid,
        )
        .unwrap();
        let pt = recovered.to_sec1_point(false);
        let mut raw = [0u8; 64];
        raw.copy_from_slice(&pt.as_bytes()[1..65]);
        assert_eq!(
            eth_address_from_pubkey_uncompressed(&raw),
            w.address_bytes()
        );
    }

    #[test]
    fn debug_redacts_key_material() {
        let w = EvmWalletSigner::from_hex(TEST_KEY).unwrap();
        let dbg = format!("{w:?}");
        assert!(dbg.contains(TEST_ADDR));
        assert!(!dbg.contains(TEST_KEY));
    }

    #[test]
    fn checksum_matches_eip55() {
        // Key 0x…01 → 0x7E5F4552091A69125d5DfCb7b8C2659029395Bdf (EIP-55).
        let w = EvmWalletSigner::from_hex(TEST_KEY).unwrap();
        assert_eq!(
            w.address_checksum(),
            "0x7E5F4552091A69125d5DfCb7b8C2659029395Bdf"
        );
    }
}
