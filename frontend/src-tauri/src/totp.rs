//! RFC 6238 TOTP (time-based one-time passwords) for optional vault MFA.
//!
//! Algorithm: HMAC-SHA1 dynamic truncation (RFC 4226 HOTP) over a 30-second
//! time step, 6-digit output — the interoperable baseline every
//! authenticator app (Google Authenticator, 1Password, Authy, Aegis)
//! implements. SHA-256 variants exist but several apps silently ignore the
//! `algorithm` parameter, so SHA-1 stays the default.
//!
//! Security note (honest scope): TOTP here gates the *unlock path* — an
//! interactive second factor for someone operating this app. The enrolled
//! secret lives inside `vault.enc`, so MFA does not add encryption strength
//! against offline file theft; the master password alone protects the file.
//! What it does stop is casual or unattended-session misuse: knowing the
//! password is not enough to unlock the workspace through the app.

use hmac::{digest::KeyInit, Hmac, Mac};
use sha1::Sha1;
use zeroize::Zeroizing;

type HmacSha1 = Hmac<Sha1>;

/// 30-second step — the authenticator-app default.
pub const TOTP_STEP_SECS: u64 = 30;
/// 6-digit codes — the authenticator-app default.
pub const TOTP_DIGITS: u32 = 6;
/// 160-bit secret — the RFC 4226/6238 recommended minimum length.
pub const TOTP_SECRET_LEN: usize = 20;
/// Steps of clock-skew tolerance on each side of the current step.
const SKEW_STEPS: i64 = 1;

/// Current unix time in whole seconds.
pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Generate a fresh 160-bit TOTP secret.
pub fn generate_secret() -> Zeroizing<[u8; TOTP_SECRET_LEN]> {
    use rand::RngCore;
    let mut secret = Zeroizing::new([0u8; TOTP_SECRET_LEN]);
    rand::thread_rng().fill_bytes(&mut *secret);
    secret
}

/// RFC 4648 base32 (uppercase, no padding) — the encoding authenticator
/// apps expect for manual entry and `otpauth://` URIs.
pub fn encode_secret_base32(secret: &[u8]) -> String {
    base32::encode(base32::Alphabet::Rfc4648 { padding: false }, secret)
}

/// Decode a base32 secret, tolerating lowercase and the whitespace/grouping
/// users paste in (`"ABCD EF12"`). Rejects invalid encodings fail-closed.
pub fn decode_secret_base32(encoded: &str) -> anyhow::Result<Zeroizing<Vec<u8>>> {
    let cleaned: String = encoded
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| c.to_ascii_uppercase())
        .collect();
    if cleaned.is_empty() {
        anyhow::bail!("empty base32 secret");
    }
    base32::decode(base32::Alphabet::Rfc4648 { padding: false }, &cleaned)
        .map(Zeroizing::new)
        .ok_or_else(|| anyhow::anyhow!("invalid base32 TOTP secret"))
}

/// HOTP dynamic truncation (RFC 4226 §5.3) → `TOTP_DIGITS`-digit value.
fn hotp(secret: &[u8], counter: u64) -> u32 {
    let mut mac = <HmacSha1 as KeyInit>::new_from_slice(secret).expect("HMAC accepts any key size");
    mac.update(&counter.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = (digest[19] & 0x0f) as usize;
    let code = ((digest[offset] as u32 & 0x7f) << 24)
        | ((digest[offset + 1] as u32) << 16)
        | ((digest[offset + 2] as u32) << 8)
        | (digest[offset + 3] as u32);
    code % 10u32.pow(TOTP_DIGITS)
}

/// The 6-digit code for a unix timestamp (zero-padded).
pub fn code_at(secret: &[u8], unix_secs: u64) -> String {
    format!("{:0>6}", hotp(secret, unix_secs / TOTP_STEP_SECS))
}

/// Verify a user-entered code at `unix_secs` with ±`SKEW_STEPS` tolerance
/// for device clock drift. Rejects anything that isn't exactly 6 digits.
pub fn verify(secret: &[u8], code: &str, unix_secs: u64) -> bool {
    let code = code.trim();
    if code.len() != TOTP_DIGITS as usize || !code.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    let current_step = (unix_secs / TOTP_STEP_SECS) as i64;
    for offset in -SKEW_STEPS..=SKEW_STEPS {
        if current_step + offset < 0 {
            continue;
        }
        if code_at(secret, (current_step + offset) as u64 * TOTP_STEP_SECS) == code {
            return true;
        }
    }
    false
}

/// `otpauth://` enrollment URI — scanned or pasted into authenticator apps.
pub fn otpauth_uri(secret_b32: &str, issuer: &str, account: &str) -> String {
    format!(
        "otpauth://totp/{}:{}?secret={}&issuer={}&algorithm=SHA1&digits={}&period={}",
        urlencoding::encode(issuer),
        urlencoding::encode(account),
        secret_b32,
        urlencoding::encode(issuer),
        TOTP_DIGITS,
        TOTP_STEP_SECS,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 6238 Appendix B SHA-1 test secret: ASCII "12345678901234567890".
    const RFC_SECRET: &[u8] = b"12345678901234567890";
    /// The same secret in base32 — a documented interop check.
    const RFC_SECRET_B32: &str = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";

    /// RFC 6238 Appendix B publishes 8-digit codes; ours are the same values
    /// reduced mod 10^6.
    fn rfc_six_digit(eight_digit: &str) -> String {
        eight_digit[2..].to_string()
    }

    #[test]
    fn rfc6238_sha1_vectors() {
        let cases = [
            (59u64, "94287082"),
            (1111111109, "07081804"),
            (1111111111, "14050471"),
            (1234567890, "89005924"),
            (2000000000, "69279037"),
            (20000000000, "65353130"),
        ];
        for (t, expected8) in cases {
            assert_eq!(
                code_at(RFC_SECRET, t),
                rfc_six_digit(expected8),
                "TOTP mismatch at t={t}"
            );
        }
    }

    #[test]
    fn base32_roundtrip_and_tolerant_decode() {
        let b32 = encode_secret_base32(RFC_SECRET);
        assert_eq!(b32, RFC_SECRET_B32);
        // Lowercase + grouped whitespace (the way apps display secrets).
        let decoded = decode_secret_base32("gezd gnbv gy3t qojq gezdgnbvgy3tqojq").unwrap();
        assert_eq!(&*decoded, RFC_SECRET);
        assert!(decode_secret_base32("not!base32!!").is_err());
        assert!(decode_secret_base32("   ").is_err());
    }

    #[test]
    fn verify_accepts_current_and_skew_steps() {
        let now = 1_700_000_000u64;
        let current = code_at(RFC_SECRET, now);
        assert!(verify(RFC_SECRET, &current, now));
        // Codes one step either side of "now" are accepted.
        let prev = code_at(RFC_SECRET, now - TOTP_STEP_SECS);
        let next = code_at(RFC_SECRET, now + TOTP_STEP_SECS);
        assert!(verify(RFC_SECRET, &prev, now));
        assert!(verify(RFC_SECRET, &next, now));
        // Two steps out is rejected.
        let stale = code_at(RFC_SECRET, now - 2 * TOTP_STEP_SECS);
        assert!(!verify(RFC_SECRET, &stale, now));
    }

    #[test]
    fn verify_rejects_malformed_codes() {
        let now = 1_700_000_000u64;
        assert!(!verify(RFC_SECRET, "", now));
        assert!(!verify(RFC_SECRET, "12345", now));
        assert!(!verify(RFC_SECRET, "1234567", now));
        assert!(!verify(RFC_SECRET, "12x456", now));
        assert!(!verify(RFC_SECRET, "000000", now));
    }

    #[test]
    fn generated_secret_encodes_to_32_chars() {
        let secret = generate_secret();
        let b32 = encode_secret_base32(&*secret);
        assert_eq!(b32.len(), 32);
        assert_eq!(&*decode_secret_base32(&b32).unwrap(), &*secret);
    }

    #[test]
    fn otpauth_uri_is_sha1_totp() {
        let uri = otpauth_uri(RFC_SECRET_B32, "Arbitrageur", "operator");
        assert!(uri.starts_with("otpauth://totp/Arbitrageur:operator?"));
        assert!(uri.contains(&format!("secret={RFC_SECRET_B32}")));
        assert!(uri.contains("issuer=Arbitrageur"));
        assert!(uri.contains("algorithm=SHA1"));
        assert!(uri.contains("digits=6"));
        assert!(uri.contains("period=30"));
    }
}
