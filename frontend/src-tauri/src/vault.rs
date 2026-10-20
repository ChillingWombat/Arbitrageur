use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use argon2::{Algorithm, Argon2, Params, Version};
use chrono::{DateTime, Utc};
use hmac::Hmac;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use zeroize::{Zeroize, Zeroizing};

type HmacSha256 = Hmac<Sha256>;

/// Version-2 vault file magic prefix. Legacy files have no magic and begin
/// directly with the 16-byte KDF salt.
const VAULT_MAGIC: &[u8; 8] = b"ARBVLT2\0";
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const MIN_PASSWORD_CHARS: usize = 12;

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct ExchangeCredentials {
    pub venue: String,
    /// Stable binding to the `exchange_accounts.id` profile that owns this
    /// record. `None` marks a legacy venue-keyed record awaiting migration,
    /// or the non-account `remote_hsm` token record.
    #[serde(default)]
    pub profile_id: Option<String>,
    pub api_key: String,
    pub api_secret: String,
    pub passphrase: Option<String>,
    #[serde(default)]
    pub account_id: Option<String>,
    #[serde(default)]
    pub wallet_address: Option<String>,
    pub account_index: Option<u64>,
    pub subaccount_index: Option<u64>,
    #[serde(default)]
    pub api_key_index: Option<u64>,
    #[serde(default)]
    pub private_key: Option<String>,
    /// Optional venue endpoint override — a REST entity domain (e.g. an
    /// OKX key issued on app.okx.com does not exist on www.okx.com, so
    /// the whole venue must be pointed at the right host) or a JSON-RPC
    /// URL (EVM AMM chains). URLs may embed secrets such as provider API
    /// keys; treat the value as secret material.
    #[serde(default)]
    pub base_url: Option<String>,
    pub enabled: bool,
}

impl Drop for ExchangeCredentials {
    fn drop(&mut self) {
        self.api_key.zeroize();
        self.api_secret.zeroize();
        if let Some(passphrase) = self.passphrase.as_mut() {
            passphrase.zeroize();
        }
        if let Some(account_id) = self.account_id.as_mut() {
            account_id.zeroize();
        }
        if let Some(wallet_address) = self.wallet_address.as_mut() {
            wallet_address.zeroize();
        }
        if let Some(private_key) = self.private_key.as_mut() {
            private_key.zeroize();
        }
        // Endpoint URLs can embed provider API keys — wipe like a secret.
        if let Some(base_url) = self.base_url.as_mut() {
            base_url.zeroize();
        }
    }
}

impl std::fmt::Debug for ExchangeCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExchangeCredentials")
            .field("venue", &self.venue)
            .field("profile_bound", &self.profile_id.is_some())
            .field("account_index", &self.account_index)
            .field("subaccount_index", &self.subaccount_index)
            .field("enabled", &self.enabled)
            .field("has_api_key", &!self.api_key.is_empty())
            .field(
                "has_private_key",
                &self.private_key.as_ref().is_some_and(|k| !k.is_empty()),
            )
            .field("has_api_secret", &!self.api_secret.is_empty())
            .field(
                "has_passphrase",
                &self.passphrase.as_ref().is_some_and(|p| !p.is_empty()),
            )
            .field(
                "has_base_url",
                &self.base_url.as_ref().is_some_and(|v| !v.is_empty()),
            )
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct VaultData {
    pub credentials: Vec<ExchangeCredentials>,
    pub updated_at: DateTime<Utc>,
    /// Enrolled TOTP secret (base32) gating vault unlock. Lives inside the
    /// encrypted payload — it is a *verification* secret for the app, not
    /// extra encryption strength for the file.
    #[serde(default)]
    pub mfa_totp_secret: Option<String>,
    /// Last consumed TOTP step. Persisted in the encrypted payload so the
    /// same 30-second code cannot be replayed after restart.
    #[serde(default)]
    pub mfa_last_accepted_step: Option<u64>,
}

impl std::fmt::Debug for VaultData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaultData")
            .field("credentials", &self.credentials)
            .field("updated_at", &self.updated_at)
            .field(
                "mfa_totp_secret",
                &self.mfa_totp_secret.as_ref().map(|_| "<set>"),
            )
            .finish()
    }
}

impl Drop for VaultData {
    fn drop(&mut self) {
        if let Some(secret) = self.mfa_totp_secret.as_mut() {
            secret.zeroize();
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultStatus {
    pub is_initialized: bool,
    pub is_unlocked: bool,
    /// True only while locked by the inactivity timer (not an explicit
    /// operator lock). Auto-lock forgets vault secrets but leaves
    /// provisioned adapter credentials working, so unattended deployments
    /// keep trading; a manual lock wipes adapter credentials instead.
    pub auto_locked: bool,
    pub unlocked_until: Option<DateTime<Utc>>,
    /// Whether TOTP MFA is enrolled — surfaced from a plaintext kv flag so
    /// the locked UI knows to render the code field. Enforcement itself
    /// reads the secret inside the vault (fail-closed on flag drift).
    #[serde(default)]
    pub mfa_enabled: bool,
}

/// Sanitized view of one stored credential record. `profile_id` links the
/// record to an `exchange_accounts` row; `None` marks a legacy venue-keyed
/// record or the non-account `remote_hsm` token. Presence flags describe
/// which fields are populated without exposing their values.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialPresence {
    pub venue: String,
    pub profile_id: Option<String>,
    pub enabled: bool,
    pub has_api_key: bool,
    pub has_api_secret: bool,
    pub has_passphrase: bool,
    pub has_account_id: bool,
    pub has_wallet_address: bool,
    pub has_private_key: bool,
    pub has_base_url: bool,
    pub account_index: Option<u64>,
    pub subaccount_index: Option<u64>,
    pub api_key_index: Option<u64>,
}

/// Account profile IDs use the same charset as the durable state store:
/// 1-128 ASCII alphanumeric, underscore, or hyphen.
fn normalize_profile_id(raw: &str) -> anyhow::Result<String> {
    let v = raw.trim().to_lowercase();
    if v.is_empty()
        || v.len() > 128
        || !v
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        anyhow::bail!("invalid account ID: expected 1-128 ASCII alphanumeric/underscore/hyphen");
    }
    Ok(v)
}

pub struct SecureVault {
    storage_path: PathBuf,
    is_unlocked: bool,
    /// Distinguishes timer-expiry locks (`true`) from explicit operator
    /// locks (`false`). Credential sync treats them differently: auto-lock
    /// keeps provisioned adapter credentials so live operation survives an
    /// unattended session; manual lock wipes them as a panic-halt.
    auto_locked: bool,
    unlocked_until: Option<DateTime<Utc>>,
    decrypted_cache: Option<VaultData>,
    encryption_key: Option<Zeroizing<[u8; 32]>>,
    kdf_salt: Option<[u8; SALT_LEN]>,
}

/// Artifacts of `SecureVault::prepare_password_rotation` — consumed by
/// `commit_password_rotation` only after the Stronghold snapshot has been
/// re-keyed. Carries both snapshot-key generations so a commit failure can
/// roll `vault.hold` back to the old key.
pub struct VaultPasswordRotation {
    encrypted_bytes: Vec<u8>,
    new_key: Zeroizing<[u8; 32]>,
    new_salt: [u8; SALT_LEN],
    /// Snapshot key derived from the post-rotation session key.
    pub new_snapshot_key: Zeroizing<[u8; 32]>,
    /// Snapshot key derived from the pre-rotation session key.
    pub old_snapshot_key: Option<Zeroizing<[u8; 32]>>,
}

impl SecureVault {
    pub fn new_with_path(path: PathBuf) -> Self {
        Self {
            storage_path: path,
            is_unlocked: false,
            auto_locked: false,
            unlocked_until: None,
            decrypted_cache: None,
            encryption_key: None,
            kdf_salt: None,
        }
    }

    pub fn default_path() -> PathBuf {
        let base = if let Ok(home) = std::env::var("HOME") {
            PathBuf::from(home)
        } else if let Ok(profile) = std::env::var("USERPROFILE") {
            PathBuf::from(profile)
        } else {
            PathBuf::from(".")
        };
        base.join(".arbitrageur").join("vault.enc")
    }

    pub fn is_initialized(&self) -> bool {
        self.storage_path.exists()
    }

    /// Path of the Stronghold snapshot holding the shared EVM wallet key —
    /// a sibling of the vault file so both live in the same per-user dir
    /// (`vault.enc` → `vault.hold`).
    pub fn stronghold_snapshot_path(&self) -> PathBuf {
        self.storage_path.with_extension("hold")
    }

    pub fn is_unlocked(&mut self) -> bool {
        if !self.is_unlocked {
            return false;
        }
        if let Some(until) = self.unlocked_until {
            if Utc::now() > until {
                self.apply_lock(true);
                return false;
            }
        }
        true
    }

    /// Whether the current locked state came from the inactivity timer.
    /// Always false while unlocked or after an explicit `lock()`.
    pub fn is_auto_locked(&self) -> bool {
        !self.is_unlocked && self.auto_locked
    }

    /// Whether adapter-authenticated venue calls (entries, exits, cancels,
    /// capability probes) remain permitted: true while unlocked OR after the
    /// inactivity timer auto-locks (which wipes only vault secrets in RAM and
    /// leaves adapter credentials provisioned by design), and false when
    /// never unlocked or after an explicit operator `lock()`.
    pub fn allows_adapter_execution(&mut self) -> bool {
        self.is_unlocked() || self.is_auto_locked()
    }

    /// Force the inactivity timer into the past so the next `is_unlocked`
    /// check exercises the auto-lock path.
    #[cfg(test)]
    pub fn force_timer_expiry(&mut self) {
        self.unlocked_until = Some(Utc::now() - chrono::Duration::seconds(1));
    }

    /// Explicit operator lock — panic-halt. The credential sync wipes
    /// provisioned adapter credentials in addition to vault secrets.
    pub fn lock(&mut self) {
        self.apply_lock(false);
    }

    fn apply_lock(&mut self, auto: bool) {
        self.is_unlocked = false;
        self.auto_locked = auto;
        self.unlocked_until = None;
        self.decrypted_cache = None;
        self.encryption_key = None;
        self.kdf_salt = None;
    }

    pub fn get_status(&mut self) -> VaultStatus {
        let unlocked = self.is_unlocked();
        VaultStatus {
            is_initialized: self.is_initialized(),
            is_unlocked: unlocked,
            auto_locked: self.is_auto_locked(),
            unlocked_until: self.unlocked_until,
            // While locked the vault contents are unreadable — the caller
            // overlays the plaintext kv flag for the locked view.
            mfa_enabled: unlocked && self.mfa_enrolled(),
        }
    }

    /// Derive the Stronghold snapshot key from the live session key via a
    /// domain-separated SHA-256. Returns `None` while locked — the key
    /// exists only while the vault is unlocked, which is exactly the
    /// custody boundary we want: the EVM stronghold can only be opened
    /// when the operator has unlocked the vault with the master password.
    ///
    /// SHA-256 of the session key (rather than reusing the key bytes
    /// directly) keeps the snapshot key cryptographically independent of
    /// the vault encryption key, so a leaked snapshot can't be used to
    /// recover the vault key.
    pub fn stronghold_session_key(&self) -> Option<Zeroizing<[u8; 32]>> {
        let key = self.encryption_key.as_ref()?;
        Some(Self::stronghold_key_for(key))
    }

    /// Domain-separated Stronghold snapshot key for a given vault session
    /// key — shared by `stronghold_session_key` and password rotation, so
    /// the `.hold` file's key always tracks the vault's active session key.
    fn stronghold_key_for(session_key: &[u8; 32]) -> Zeroizing<[u8; 32]> {
        let mut hasher = Sha256::new();
        hasher.update(b"arbitrageur/stronghold-snapshot/v1");
        hasher.update(session_key.as_slice());
        let digest = hasher.finalize();
        let mut out = Zeroizing::new([0u8; 32]);
        out.copy_from_slice(&digest);
        out
    }

    /// Derive the v2 32-byte encryption key using Argon2id
    /// (64 MiB memory, 3 iterations, parallelism 1, 32-byte output).
    fn derive_v2_key(password: &str, salt: &[u8]) -> anyhow::Result<Zeroizing<[u8; 32]>> {
        let params = Params::new(64 * 1024, 3, 1, Some(32))
            .map_err(|e| anyhow::anyhow!("Invalid Argon2 parameters: {}", e))?;
        let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
        let mut key = Zeroizing::new([0u8; 32]);
        argon2
            .hash_password_into(password.as_bytes(), salt, &mut *key)
            .map_err(|e| anyhow::anyhow!("Key derivation failed: {}", e))?;
        Ok(key)
    }

    /// Legacy 32-byte key derivation retained only for reading and migrating
    /// pre-v2 vault files (PBKDF2-HMAC-SHA256, 10,000 rounds).
    fn derive_legacy_key(password: &str, salt: &[u8]) -> Zeroizing<[u8; 32]> {
        let mut key = Zeroizing::new([0u8; 32]);
        let hmac_signer = |data: &[u8]| -> Zeroizing<[u8; 32]> {
            use hmac::{digest::KeyInit, Mac};
            let mut mac = <HmacSha256 as KeyInit>::new_from_slice(password.as_bytes())
                .expect("HMAC can take key of any size");
            mac.update(data);
            let result = mac.finalize();
            let mut out = Zeroizing::new([0u8; 32]);
            out.copy_from_slice(&result.into_bytes());
            out
        };

        let mut u_prev = {
            let mut block_input = Zeroizing::new(Vec::with_capacity(salt.len() + 4));
            block_input.extend_from_slice(salt);
            block_input.extend_from_slice(&1u32.to_be_bytes());
            hmac_signer(&block_input)
        };
        key.copy_from_slice(&u_prev[..]);

        for _ in 1..10_000 {
            u_prev = hmac_signer(&u_prev[..]);
            for i in 0..32 {
                key[i] ^= u_prev[i];
            }
        }
        key
    }

    /// Serialize and encrypt `data` into the v2 container layout:
    /// VAULT_MAGIC || salt || nonce || AES-256-GCM ciphertext+tag.
    fn encrypt_v2(
        data: &VaultData,
        key: &[u8; 32],
        salt: &[u8; SALT_LEN],
    ) -> anyhow::Result<Vec<u8>> {
        let plaintext = Zeroizing::new(serde_json::to_vec(data)?);

        let mut nonce_bytes = [0u8; NONCE_LEN];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);

        let cipher = Aes256Gcm::new_from_slice(key)
            .map_err(|e| anyhow::anyhow!("Cipher init error: {}", e))?;
        #[allow(deprecated)]
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = cipher
            .encrypt(
                nonce,
                Payload {
                    msg: plaintext.as_slice(),
                    aad: VAULT_MAGIC,
                },
            )
            .map_err(|e| anyhow::anyhow!("Encryption failed: {}", e))?;

        let mut output =
            Vec::with_capacity(VAULT_MAGIC.len() + SALT_LEN + NONCE_LEN + ciphertext.len());
        output.extend_from_slice(VAULT_MAGIC);
        output.extend_from_slice(salt);
        output.extend_from_slice(&nonce_bytes);
        output.extend_from_slice(&ciphertext);
        Ok(output)
    }

    /// Atomically replace `path` with `bytes` via a random same-directory temp
    /// file. On Unix the parent directory is enforced 0700 and the vault file
    /// 0600; the rename is atomic, so readers never observe a partial file.
    /// On other platforms the same temp-then-rename sequence is attempted and
    /// any failure returns an error rather than deleting the existing file.
    fn write_file_atomic(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
        let parent = path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Vault path has no parent directory"))?;
        fs::create_dir_all(parent)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        }

        let tmp_path = parent.join(format!(".vault_tmp_{:016x}", rand::random::<u64>()));

        let write_result = (|| -> anyhow::Result<()> {
            let mut opts = fs::OpenOptions::new();
            opts.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            let mut file = opts.open(&tmp_path)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&tmp_path, path)?;
            Ok(())
        })();

        if write_result.is_err() {
            let _ = fs::remove_file(&tmp_path);
        }
        write_result?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
            // fsync the directory so the rename is durable across crashes
            fs::File::open(parent)?.sync_all()?;
        }
        Ok(())
    }

    /// Initialize a new vault with password
    pub fn initialize(&mut self, password: &str) -> anyhow::Result<()> {
        if self.is_initialized() {
            anyhow::bail!(
                "Vault is already initialized; refusing to overwrite existing credentials"
            );
        }
        if password.chars().count() < MIN_PASSWORD_CHARS {
            anyhow::bail!("Password must be at least {MIN_PASSWORD_CHARS} characters long");
        }

        let empty_data = VaultData {
            credentials: Vec::new(),
            updated_at: Utc::now(),
            mfa_totp_secret: None,
            mfa_last_accepted_step: None,
        };
        let mut salt = [0u8; SALT_LEN];
        rand::thread_rng().fill_bytes(&mut salt);
        let key = Self::derive_v2_key(password, &salt)?;
        let bytes = Self::encrypt_v2(&empty_data, &key, &salt)?;
        Self::write_file_atomic(&self.storage_path, &bytes)?;

        self.is_unlocked = true;
        self.auto_locked = false;
        self.unlocked_until = Some(Utc::now() + chrono::Duration::minutes(15));
        self.decrypted_cache = Some(empty_data);
        self.encryption_key = Some(key);
        self.kdf_salt = Some(salt);
        Ok(())
    }

    /// Post-decrypt admission: when a TOTP secret is enrolled, verify the
    /// presented code BEFORE the session fields are set — a missing or bad
    /// code leaves the vault locked. Secrets that fail to decode are a
    /// corrupted-vault error, never a silent bypass.
    fn admit_unlocked(
        &mut self,
        data: VaultData,
        key: Zeroizing<[u8; 32]>,
        salt: [u8; SALT_LEN],
        totp_code: Option<&str>,
    ) -> anyhow::Result<()> {
        if let Some(secret_b32) = data.mfa_totp_secret.as_deref() {
            let secret = crate::totp::decode_secret_base32(secret_b32).map_err(|_| {
                anyhow::anyhow!(
                    "Vault MFA data is corrupted — the enrolled secret cannot be decoded"
                )
            })?;
            let code = totp_code.map(str::trim).filter(|c| !c.is_empty()).ok_or_else(|| {
                anyhow::anyhow!(
                    "Two-factor authentication is enabled — enter the 6-digit code from your authenticator app"
                )
            })?;
            let now = crate::totp::unix_now();
            if !crate::totp::verify_after_step(&secret, code, now, data.mfa_last_accepted_step) {
                anyhow::bail!("Invalid authentication code");
            }
            let step = crate::totp::accepted_step_for(&secret, code, now)
                .ok_or_else(|| anyhow::anyhow!("Invalid authentication code"))?;
            let mut updated = data.clone();
            updated.mfa_last_accepted_step = Some(step);
            let encrypted = Self::encrypt_v2(&updated, &key, &salt)?;
            Self::write_file_atomic(&self.storage_path, &encrypted)?;
            return self.admit_unlocked_without_totp(updated, key, salt);
        }
        self.admit_unlocked_without_totp(data, key, salt)
    }

    fn admit_unlocked_without_totp(
        &mut self,
        data: VaultData,
        key: Zeroizing<[u8; 32]>,
        salt: [u8; SALT_LEN],
    ) -> anyhow::Result<()> {
        self.is_unlocked = true;
        self.auto_locked = false;
        self.unlocked_until = Some(Utc::now() + chrono::Duration::minutes(15));
        self.decrypted_cache = Some(data);
        self.encryption_key = Some(key);
        self.kdf_salt = Some(salt);
        Ok(())
    }

    /// Unlock existing vault with password (+ TOTP code when MFA is
    /// enrolled). Legacy (unversioned PBKDF2) files are transparently
    /// migrated to the v2 format on success.
    pub fn unlock(&mut self, password: &str, totp_code: Option<&str>) -> anyhow::Result<()> {
        self.lock();
        if !self.storage_path.exists() {
            anyhow::bail!("Vault is not initialized. Please set a password first.");
        }
        let file_bytes = fs::read(&self.storage_path)?;

        if file_bytes.starts_with(VAULT_MAGIC.as_slice()) {
            if file_bytes.len() < VAULT_MAGIC.len() + SALT_LEN + NONCE_LEN + 16 {
                anyhow::bail!("Incorrect password or corrupted vault");
            }
            let salt = &file_bytes[VAULT_MAGIC.len()..VAULT_MAGIC.len() + SALT_LEN];
            let nonce_start = VAULT_MAGIC.len() + SALT_LEN;
            let nonce_bytes = &file_bytes[nonce_start..nonce_start + NONCE_LEN];
            let ciphertext = &file_bytes[nonce_start + NONCE_LEN..];

            let key = Self::derive_v2_key(password, salt)?;
            let cipher = Aes256Gcm::new_from_slice(&*key)
                .map_err(|e| anyhow::anyhow!("Cipher init error: {}", e))?;
            #[allow(deprecated)]
            let nonce = Nonce::from_slice(nonce_bytes);

            let decrypted_bytes = Zeroizing::new(
                cipher
                    .decrypt(
                        nonce,
                        Payload {
                            msg: ciphertext,
                            aad: VAULT_MAGIC,
                        },
                    )
                    .map_err(|_| anyhow::anyhow!("Incorrect password or corrupted vault"))?,
            );

            let data: VaultData = serde_json::from_slice(decrypted_bytes.as_slice())
                .map_err(|_| anyhow::anyhow!("Incorrect password or corrupted vault"))?;

            let mut salt_arr = [0u8; SALT_LEN];
            salt_arr.copy_from_slice(salt);
            self.admit_unlocked(data, key, salt_arr, totp_code)
        } else {
            // Legacy format: salt || nonce || ciphertext, no magic, no AAD
            if file_bytes.len() < SALT_LEN + NONCE_LEN + 16 {
                anyhow::bail!("Incorrect password or corrupted vault");
            }
            let salt = &file_bytes[0..SALT_LEN];
            let nonce_bytes = &file_bytes[SALT_LEN..SALT_LEN + NONCE_LEN];
            let ciphertext = &file_bytes[SALT_LEN + NONCE_LEN..];

            let legacy_key = Self::derive_legacy_key(password, salt);
            let cipher = Aes256Gcm::new_from_slice(&*legacy_key)
                .map_err(|e| anyhow::anyhow!("Cipher init error: {}", e))?;
            #[allow(deprecated)]
            let nonce = Nonce::from_slice(nonce_bytes);

            let decrypted_bytes = Zeroizing::new(
                cipher
                    .decrypt(nonce, ciphertext)
                    .map_err(|_| anyhow::anyhow!("Incorrect password or corrupted vault"))?,
            );

            let data: VaultData = serde_json::from_slice(decrypted_bytes.as_slice())
                .map_err(|_| anyhow::anyhow!("Incorrect password or corrupted vault"))?;
            drop(legacy_key);

            // Migrate to v2: re-encrypt with Argon2id and a fresh salt
            let mut new_salt = [0u8; SALT_LEN];
            rand::thread_rng().fill_bytes(&mut new_salt);
            let new_key = Self::derive_v2_key(password, &new_salt)?;
            let bytes = Self::encrypt_v2(&data, &new_key, &new_salt)?;
            Self::write_file_atomic(&self.storage_path, &bytes)?;

            self.admit_unlocked(data, new_key, new_salt, totp_code)
        }
    }

    /// Save updated credentials to encrypted storage. Requires an unlocked
    /// vault with a cached v2 key and salt; writes under a fresh nonce.
    pub fn save_data(&mut self, data: &VaultData) -> anyhow::Result<()> {
        if !self.is_unlocked() {
            anyhow::bail!("Vault is locked. Please unlock before saving.");
        }
        let key = self
            .encryption_key
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Vault encryption key is unavailable"))?
            .clone();
        let salt = self
            .kdf_salt
            .ok_or_else(|| anyhow::anyhow!("Vault key salt is unavailable"))?;

        let bytes = Self::encrypt_v2(data, &key, &salt)?;
        Self::write_file_atomic(&self.storage_path, &bytes)?;

        self.decrypted_cache = Some(data.clone());
        self.encryption_key = Some(key);
        self.kdf_salt = Some(salt);
        self.auto_locked = false;
        self.unlocked_until = Some(Utc::now() + chrono::Duration::minutes(15));
        Ok(())
    }

    /// Verify `current` against the on-disk vault and prepare the rotated
    /// ciphertext — nothing is written yet. The caller re-keys `vault.hold`
    /// under `rotation.new_snapshot_key` BEFORE calling
    /// `commit_password_rotation`, so the vault file and the Stronghold
    /// snapshot never sit on different password generations (a rotated
    /// vault + stale snapshot silently orphans every AMM signer).
    ///
    /// Password verification runs against the file bytes, not the session
    /// key — a legacy (PBKDF2) vault's session key is not the Argon2 key,
    /// so comparing derived keys would false-reject a correct password.
    /// `totp_code` is forwarded to the check-unlock so a rotation is gated
    /// by the same second factor as a normal unlock.
    pub fn prepare_password_rotation(
        &mut self,
        current: &str,
        new_password: &str,
        totp_code: Option<&str>,
    ) -> anyhow::Result<VaultPasswordRotation> {
        if !self.is_unlocked() {
            anyhow::bail!("Vault is locked — unlock before changing the password");
        }
        let mut check = SecureVault::new_with_path(self.storage_path.clone());
        check
            .unlock(current, totp_code)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        if new_password.chars().count() < MIN_PASSWORD_CHARS {
            anyhow::bail!("New password must be at least {MIN_PASSWORD_CHARS} characters long");
        }

        let data = self
            .decrypted_cache
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Decrypted vault cache is empty"))?;
        let mut new_salt = [0u8; SALT_LEN];
        rand::thread_rng().fill_bytes(&mut new_salt);
        let new_key = Self::derive_v2_key(new_password, &new_salt)?;
        let encrypted_bytes = Self::encrypt_v2(&data, &new_key, &new_salt)?;
        let new_snapshot_key = Self::stronghold_key_for(&new_key);
        let old_snapshot_key = self.stronghold_session_key();
        Ok(VaultPasswordRotation {
            encrypted_bytes,
            new_key,
            new_salt,
            new_snapshot_key,
            old_snapshot_key,
        })
    }

    /// Commit a prepared rotation: atomic vault-file write, then the
    /// session-key swap. Only called after the `.hold` snapshot has been
    /// re-keyed under `rotation.new_snapshot_key` (or confirmed absent).
    pub fn commit_password_rotation(
        &mut self,
        rotation: VaultPasswordRotation,
    ) -> anyhow::Result<()> {
        Self::write_file_atomic(&self.storage_path, &rotation.encrypted_bytes)?;
        self.encryption_key = Some(rotation.new_key);
        self.kdf_salt = Some(rotation.new_salt);
        self.auto_locked = false;
        self.unlocked_until = Some(Utc::now() + chrono::Duration::minutes(15));
        Ok(())
    }

    /// Read data from decrypted cache
    pub fn get_data(&mut self) -> anyhow::Result<&VaultData> {
        if !self.is_unlocked() {
            anyhow::bail!("Vault is locked. Please enter password to view credentials.");
        }
        self.decrypted_cache
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Decrypted vault cache is empty"))
    }

    /// Whether a TOTP secret is enrolled — meaningful only while unlocked
    /// (a locked vault cannot read its own payload).
    pub fn mfa_enrolled(&self) -> bool {
        self.decrypted_cache
            .as_ref()
            .is_some_and(|d| d.mfa_totp_secret.is_some())
    }

    /// Verify a TOTP code against the enrolled secret. Requires an unlocked
    /// vault; `Ok(false)` is a wrong code, an error means no secret is
    /// enrolled or it is corrupted. The secret never leaves the module.
    pub fn verify_mfa_code(&mut self, code: &str) -> anyhow::Result<bool> {
        let data = self.get_data()?.clone();
        let Some(secret_b32) = data.mfa_totp_secret.as_deref() else {
            anyhow::bail!("Two-factor authentication is not enrolled");
        };
        let secret = crate::totp::decode_secret_base32(secret_b32)
            .map_err(|e| anyhow::anyhow!("vault MFA secret decode failed: {e}"))?;
        let now = crate::totp::unix_now();
        let Some(step) = crate::totp::accepted_step_for(&secret, code.trim(), now) else {
            return Ok(false);
        };
        if data.mfa_last_accepted_step.is_some_and(|last| step <= last) {
            return Ok(false);
        }
        let mut updated = data;
        updated.mfa_last_accepted_step = Some(step);
        let key = self
            .encryption_key
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Vault is locked"))?;
        let salt = self
            .kdf_salt
            .ok_or_else(|| anyhow::anyhow!("Vault encryption metadata is missing"))?;
        let encrypted = Self::encrypt_v2(&updated, key, &salt)?;
        Self::write_file_atomic(&self.storage_path, &encrypted)?;
        self.decrypted_cache = Some(updated);
        Ok(true)
    }

    /// Enroll (or replace) the TOTP secret and persist. The secret is
    /// decoded once before storing so an unverifiable value can never be
    /// written. Requires an unlocked vault.
    pub fn set_mfa_secret(&mut self, secret_b32: String) -> anyhow::Result<()> {
        if !self.is_unlocked() {
            anyhow::bail!("Vault is locked. Please unlock before configuring MFA.");
        }
        crate::totp::decode_secret_base32(&secret_b32)
            .map_err(|e| anyhow::anyhow!("invalid MFA secret: {e}"))?;
        let mut data = self.decrypted_cache.clone().unwrap_or_default();
        data.mfa_totp_secret = Some(secret_b32);
        data.updated_at = Utc::now();
        self.save_data(&data)
    }

    /// Remove the TOTP secret and persist. Requires an unlocked vault.
    /// Callers verify a live code first — this primitive deliberately does
    /// not, so that a forced reset path remains possible.
    pub fn clear_mfa_secret(&mut self) -> anyhow::Result<()> {
        if !self.is_unlocked() {
            anyhow::bail!("Vault is locked. Please unlock before configuring MFA.");
        }
        let mut data = self.decrypted_cache.clone().unwrap_or_default();
        if let Some(mut secret) = data.mfa_totp_secret.take() {
            secret.zeroize();
            data.updated_at = Utc::now();
            self.save_data(&data)?;
        }
        Ok(())
    }

    /// Sorted, deduplicated, normalized (trimmed lowercase) venue IDs that
    /// have stored credential records, regardless of their enabled flag.
    /// Presence means credentials exist; venue activation remains gated by
    /// settings toggles and read-only admission. Requires an unlocked vault;
    /// never exposes credential values.
    pub fn configured_venues(&mut self) -> anyhow::Result<Vec<String>> {
        let data = self.get_data()?;
        let mut venues: Vec<String> = data
            .credentials
            .iter()
            .map(|c| c.venue.trim().to_lowercase())
            .collect();
        venues.sort();
        venues.dedup();
        Ok(venues)
    }

    /// Merge nonempty submitted values onto an existing record. Empty strings
    /// and `None` preserve stored values — except `base_url`, where an
    /// explicit `Some("")` clears the stored endpoint while `None` preserves
    /// it. `enabled` and index fields always update. `profile_id` and
    /// `venue` are never mutated by a merge.
    fn merge_credential_fields(existing: &mut ExchangeCredentials, sub: &ExchangeCredentials) {
        if !sub.api_key.is_empty() {
            existing.api_key = sub.api_key.clone();
        }
        if !sub.api_secret.is_empty() {
            existing.api_secret = sub.api_secret.clone();
        }
        if let Some(passphrase) = &sub.passphrase {
            if !passphrase.is_empty() {
                existing.passphrase = Some(passphrase.clone());
            }
        }
        if let Some(account_id) = &sub.account_id {
            if !account_id.is_empty() {
                existing.account_id = Some(account_id.clone());
            }
        }
        if let Some(wallet_address) = &sub.wallet_address {
            if !wallet_address.is_empty() {
                existing.wallet_address = Some(wallet_address.clone());
            }
        }
        existing.account_index = sub.account_index;
        existing.subaccount_index = sub.subaccount_index;
        existing.api_key_index = sub.api_key_index;
        if let Some(private_key) = &sub.private_key {
            if !private_key.is_empty() {
                existing.private_key = Some(private_key.clone());
            }
        }
        // Endpoint override follows patch semantics: absent (`None`)
        // preserves the stored URL, `Some("")` clears it back to the
        // adapter's default host, a non-empty value replaces it.
        if let Some(base_url) = &sub.base_url {
            let trimmed = base_url.trim();
            existing.base_url = (!trimmed.is_empty()).then(|| trimmed.to_string());
        }
        existing.enabled = sub.enabled;
    }

    /// Merge submitted legacy/unbound credentials by exact lowercase venue
    /// ID and persist. Only records with `profile_id == None` participate, so
    /// account-bound records can never be matched or overwritten by venue.
    /// Nonempty submitted values replace stored ones; empty strings and
    /// `None` preserve existing values — except `base_url`, where `Some("")`
    /// clears. `enabled` and index fields always update. Used only for the
    /// non-account `remote_hsm` record and for pre-migration writes.
    pub fn upsert_credentials(
        &mut self,
        submitted: Vec<ExchangeCredentials>,
    ) -> anyhow::Result<()> {
        if !self.is_unlocked() {
            anyhow::bail!("Vault is locked. Please unlock before saving credentials.");
        }
        let mut data = self.decrypted_cache.clone().unwrap_or_default();

        for mut sub in submitted {
            let venue_key = sub.venue.to_lowercase();
            if let Some(existing) = data
                .credentials
                .iter_mut()
                .find(|c| c.profile_id.is_none() && c.venue.to_lowercase() == venue_key)
            {
                Self::merge_credential_fields(existing, &sub);
            } else {
                sub.profile_id = None;
                // Never persist the clear sentinel on a new record.
                if sub.base_url.as_deref().is_some_and(|u| u.trim().is_empty()) {
                    sub.base_url = None;
                }
                data.credentials.push(sub);
            }
        }

        data.updated_at = Utc::now();
        self.save_data(&data)
    }

    /// Merge submitted credentials into the record bound to `profile_id`,
    /// creating a bound record when none exists. Matching is exclusively by
    /// `profile_id`; the venue on the record is informational. Requires an
    /// unlocked vault.
    pub fn upsert_profile_credentials(
        &mut self,
        profile_id: &str,
        cred: ExchangeCredentials,
    ) -> anyhow::Result<()> {
        if !self.is_unlocked() {
            anyhow::bail!("Vault is locked. Please unlock before saving credentials.");
        }
        let pid = normalize_profile_id(profile_id)?;
        let mut data = self.decrypted_cache.clone().unwrap_or_default();

        if let Some(existing) = data
            .credentials
            .iter_mut()
            .find(|c| c.profile_id.as_deref() == Some(pid.as_str()))
        {
            Self::merge_credential_fields(existing, &cred);
        } else {
            let mut record = cred;
            record.profile_id = Some(pid);
            // Never persist the clear sentinel on a new record.
            if record
                .base_url
                .as_deref()
                .is_some_and(|u| u.trim().is_empty())
            {
                record.base_url = None;
            }
            data.credentials.push(record);
        }

        data.updated_at = Utc::now();
        self.save_data(&data)
    }

    /// Sanitized per-record presence metadata. `profile_id` identifies which
    /// account profile a record is bound to (`None` = legacy venue-keyed or
    /// `remote_hsm`). No secret values are ever returned. Requires an
    /// unlocked vault.
    pub fn credential_presence(&mut self) -> anyhow::Result<Vec<CredentialPresence>> {
        let data = self.get_data()?;
        Ok(data
            .credentials
            .iter()
            .map(|c| CredentialPresence {
                venue: c.venue.trim().to_lowercase(),
                profile_id: c.profile_id.clone(),
                enabled: c.enabled,
                has_api_key: !c.api_key.is_empty(),
                has_api_secret: !c.api_secret.is_empty(),
                has_passphrase: c.passphrase.as_ref().is_some_and(|p| !p.is_empty()),
                has_account_id: c.account_id.as_ref().is_some_and(|v| !v.is_empty()),
                has_wallet_address: c.wallet_address.as_ref().is_some_and(|v| !v.is_empty()),
                has_private_key: c.private_key.as_ref().is_some_and(|v| !v.is_empty()),
                has_base_url: c.base_url.as_ref().is_some_and(|v| !v.is_empty()),
                account_index: c.account_index,
                subaccount_index: c.subaccount_index,
                api_key_index: c.api_key_index,
            })
            .collect())
    }

    /// Legacy venue-keyed records still lacking a `profile_id` binding.
    pub fn unbound_credentials(&mut self) -> anyhow::Result<Vec<CredentialPresence>> {
        Ok(self
            .credential_presence()?
            .into_iter()
            .filter(|p| p.profile_id.is_none())
            .collect())
    }

    /// Bind the single unbound record for `venue` to `profile_id`. Fails
    /// closed when the venue has zero or multiple unbound records, or when
    /// another record is already bound to that profile. Requires an unlocked
    /// vault.
    pub fn bind_profile(&mut self, venue: &str, profile_id: &str) -> anyhow::Result<()> {
        if !self.is_unlocked() {
            anyhow::bail!("Vault is locked. Please unlock before migrating credentials.");
        }
        let venue_key = venue.trim().to_lowercase();
        let pid = normalize_profile_id(profile_id)?;
        let mut data = self.decrypted_cache.clone().unwrap_or_default();

        if data
            .credentials
            .iter()
            .any(|c| c.profile_id.as_deref() == Some(pid.as_str()))
        {
            anyhow::bail!("account {pid} already has a bound credential record");
        }
        let matches: Vec<usize> = data
            .credentials
            .iter()
            .enumerate()
            .filter(|(_, c)| c.profile_id.is_none() && c.venue.trim().to_lowercase() == venue_key)
            .map(|(i, _)| i)
            .collect();
        match matches.len() {
            0 => anyhow::bail!("no unbound credential record for venue {venue_key}"),
            1 => data.credentials[matches[0]].profile_id = Some(pid),
            n => anyhow::bail!(
                "corrupted vault: {n} unbound credential records for venue {venue_key}"
            ),
        }

        data.updated_at = Utc::now();
        self.save_data(&data)
    }

    /// Delete the credential record bound to `profile_id`. Requires an
    /// unlocked vault. Removed records are dropped so their `Drop` impl
    /// zeroizes secret fields.
    pub fn delete_profile_credentials(&mut self, profile_id: &str) -> anyhow::Result<bool> {
        if !self.is_unlocked() {
            anyhow::bail!("Vault is locked. Please unlock before deleting credentials.");
        }
        let pid = normalize_profile_id(profile_id)?;
        let mut data = self.decrypted_cache.clone().unwrap_or_default();
        let before = data.credentials.len();
        data.credentials
            .retain(|c| c.profile_id.as_deref() != Some(pid.as_str()));
        if data.credentials.len() == before {
            return Ok(false);
        }
        data.updated_at = Utc::now();
        self.save_data(&data)?;
        Ok(true)
    }

    /// Update the enabled flag on credential records by bound profile ID.
    /// Unknown profiles are ignored and the vault is only rewritten when a
    /// flag changed. The account profile in the state store remains the
    /// authoritative enablement source; this mirrors the flag for file-format
    /// consistency. Requires an unlocked vault.
    pub fn update_profile_enabled(&mut self, enablement: &[(String, bool)]) -> anyhow::Result<()> {
        if !self.is_unlocked() {
            anyhow::bail!("Vault is locked. Please unlock before updating credentials.");
        }
        let mut data = self.decrypted_cache.clone().unwrap_or_default();
        let mut changed = false;

        for (profile_id, enabled) in enablement {
            let pid = normalize_profile_id(profile_id)?;
            if let Some(existing) = data
                .credentials
                .iter_mut()
                .find(|c| c.profile_id.as_deref() == Some(pid.as_str()))
            {
                if existing.enabled != *enabled {
                    existing.enabled = *enabled;
                    changed = true;
                }
            }
        }

        if changed {
            data.updated_at = Utc::now();
            self.save_data(&data)?;
        }
        Ok(())
    }

    /// Strip wallet material — `private_key` and `wallet_address` — from the
    /// record bound to `profile_id`, preserving endpoint overrides, index
    /// fields and the enabled flag. Used by the shared EVM-wallet commands so
    /// a "forget wallet" removes signing ability without wiping per-venue RPC
    /// endpoints or account rows. Requires an unlocked vault; Ok(false) when
    /// no record exists or it already held no wallet material.
    pub fn clear_profile_wallet_fields(&mut self, profile_id: &str) -> anyhow::Result<bool> {
        if !self.is_unlocked() {
            anyhow::bail!("Vault is locked. Please unlock before clearing wallet credentials.");
        }
        let pid = normalize_profile_id(profile_id)?;
        let mut data = self.decrypted_cache.clone().unwrap_or_default();
        let Some(existing) = data
            .credentials
            .iter_mut()
            .find(|c| c.profile_id.as_deref() == Some(pid.as_str()))
        else {
            return Ok(false);
        };
        if existing.private_key.is_none() && existing.wallet_address.is_none() {
            return Ok(false);
        }
        if let Some(mut private_key) = existing.private_key.take() {
            private_key.zeroize();
        }
        if let Some(mut wallet_address) = existing.wallet_address.take() {
            wallet_address.zeroize();
        }
        data.updated_at = Utc::now();
        self.save_data(&data)?;
        Ok(true)
    }

    /// Update the enabled flag on every credential record for a lowercase
    /// venue ID (all bound accounts plus any legacy unbound record). Unknown
    /// venues are ignored, stored secrets are never touched, and the vault is
    /// only rewritten when a flag changed. Requires an unlocked vault.
    pub fn update_enabled(&mut self, enablement: &[(String, bool)]) -> anyhow::Result<()> {
        if !self.is_unlocked() {
            anyhow::bail!("Vault is locked. Please unlock before updating credentials.");
        }
        let mut data = self.decrypted_cache.clone().unwrap_or_default();
        let mut changed = false;

        for (venue, enabled) in enablement {
            let venue_key = venue.to_lowercase();
            for existing in data
                .credentials
                .iter_mut()
                .filter(|c| c.venue.to_lowercase() == venue_key)
            {
                if existing.enabled != *enabled {
                    existing.enabled = *enabled;
                    changed = true;
                }
            }
        }

        if changed {
            data.updated_at = Utc::now();
            self.save_data(&data)?;
        }
        Ok(())
    }

    /// Delete every credential record for an exact lowercase venue ID.
    /// Requires an unlocked vault. Removed records are dropped so their
    /// `Drop` impl zeroizes secret fields, and the vault file is rewritten
    /// only when at least one record was removed.
    pub fn delete_credentials(&mut self, venue: &str) -> anyhow::Result<bool> {
        if !self.is_unlocked() {
            anyhow::bail!("Vault is locked. Please unlock before deleting credentials.");
        }
        let venue_key = venue.trim().to_lowercase();
        if venue_key.is_empty()
            || !venue_key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            anyhow::bail!("Invalid venue ID for credential deletion");
        }
        let mut data = self.decrypted_cache.clone().unwrap_or_default();
        let before = data.credentials.len();
        data.credentials
            .retain(|c| c.venue.to_lowercase() != venue_key);
        if data.credentials.len() == before {
            return Ok(false);
        }
        data.updated_at = Utc::now();
        self.save_data(&data)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_vault_path(tag: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "arbitrageur_vault_test_{}_{}",
            tag,
            rand::random::<u64>()
        ));
        let file = dir.join("vault.enc");
        (dir, file)
    }

    #[test]
    fn test_password_rotation_two_phase() {
        let (dir, file) = temp_vault_path("pwrotate");
        let mut vault = SecureVault::new_with_path(file.clone());
        vault.initialize("old-password-1234").expect("init");
        let old_snap = vault
            .stronghold_session_key()
            .expect("unlocked session key");

        // Wrong current password must not prepare anything.
        assert!(vault
            .prepare_password_rotation("wrong-password", "new-password-5678", None)
            .is_err());
        // Short new password rejected before any key material is produced.
        assert!(vault
            .prepare_password_rotation("old-password-1234", "short", None)
            .is_err());
        // A locked vault refuses rotation outright.
        let mut locked = SecureVault::new_with_path(file.clone());
        assert!(locked
            .prepare_password_rotation("old-password-1234", "new-password-5678", None)
            .is_err());

        let rotation = vault
            .prepare_password_rotation("old-password-1234", "new-password-5678", None)
            .expect("prepare");
        // The snapshot key must track the session-key generation.
        assert_eq!(
            rotation
                .old_snapshot_key
                .as_ref()
                .expect("old snapshot key")
                .as_slice(),
            old_snap.as_slice()
        );
        assert_ne!(
            rotation.new_snapshot_key.as_slice(),
            old_snap.as_slice(),
            "rotation must produce a new snapshot key"
        );
        // Nothing committed yet — old password still opens the file.
        let mut probe = SecureVault::new_with_path(file.clone());
        assert!(probe.unlock("old-password-1234", None).is_ok());
        drop(probe);

        vault.commit_password_rotation(rotation).expect("commit");

        let mut old_probe = SecureVault::new_with_path(file.clone());
        assert!(old_probe.unlock("old-password-1234", None).is_err());
        let mut new_probe = SecureVault::new_with_path(file);
        assert!(new_probe.unlock("new-password-5678", None).is_ok());
        // Session key swapped — the live vault now derives the new snapshot key.
        assert_ne!(
            vault
                .stronghold_session_key()
                .expect("session key")
                .as_slice(),
            old_snap.as_slice()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_vault_initialize_rejects_short_password_and_reinit() {
        let (dir, file) = temp_vault_path("init");
        let mut vault = SecureVault::new_with_path(file);
        assert!(!vault.is_initialized());

        let err = vault.initialize("short").unwrap_err();
        assert!(
            err.to_string().contains("at least 12 characters"),
            "got: {err}"
        );

        assert!(vault.initialize("valid-password").is_ok());
        let err = vault.initialize("another-long-password").unwrap_err();
        assert!(err.to_string().contains("already initialized"));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_vault_requires_twelve_characters_without_composition_rules() {
        let (dir, file) = temp_vault_path("too_short");
        let mut vault = SecureVault::new_with_path(file);
        let err = vault.initialize("abcde").unwrap_err();
        assert!(err.to_string().contains("at least 12 characters"));
        let _ = fs::remove_dir_all(dir);

        for (kind, password) in [
            ("letters", "abcdefghijkl"),
            ("numbers", "123456789012"),
            ("symbols", "!@#$%^&*()_+"),
            ("mixed", "a1!b2@c3#d4$"),
            ("longer", "abcdefghijkl"),
        ] {
            let (dir, file) = temp_vault_path(kind);
            let mut vault = SecureVault::new_with_path(file);
            vault
                .initialize(password)
                .unwrap_or_else(|err| panic!("{kind} password should be accepted: {err}"));
            let _ = fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn test_vault_initialization_persists_across_instances() {
        let (dir, file) = temp_vault_path("persisted_init");
        {
            let mut vault = SecureVault::new_with_path(file.clone());
            vault.initialize("correct horse battery").unwrap();
            let status = vault.get_status();
            assert!(status.is_initialized);
            assert!(status.is_unlocked);
        }

        let mut reopened = SecureVault::new_with_path(file);
        let locked_status = reopened.get_status();
        assert!(locked_status.is_initialized);
        assert!(!locked_status.is_unlocked);
        reopened.unlock("correct horse battery", None).unwrap();
        assert!(reopened.get_status().is_unlocked);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_vault_v2_roundtrip_lock_reunlock() {
        let (dir, file) = temp_vault_path("v2");
        let mut vault = SecureVault::new_with_path(file.clone());
        vault.initialize("correct horse battery").unwrap();

        let mut data = vault.get_data().unwrap().clone();
        data.credentials.push(ExchangeCredentials {
            venue: "hyperliquid".into(),
            profile_id: None,
            api_key: "0x123abc".into(),
            api_secret: "secret_456".into(),
            passphrase: None,
            account_id: None,
            wallet_address: Some("0xwallet".into()),
            account_index: Some(0),
            subaccount_index: Some(1),
            api_key_index: None,
            private_key: None,
            base_url: None,

            enabled: true,
        });
        vault.save_data(&data).unwrap();

        let file_bytes = fs::read(&file).unwrap();
        assert!(file_bytes.starts_with(VAULT_MAGIC.as_slice()));

        vault.lock();
        assert!(!vault.is_unlocked());
        assert!(vault.get_data().is_err());
        assert!(vault.configured_venues().is_err());

        vault.unlock("correct horse battery", None).unwrap();
        assert!(vault.is_unlocked());
        let stored = vault.get_data().unwrap();
        assert_eq!(stored.credentials.len(), 1);
        assert_eq!(stored.credentials[0].api_secret, "secret_456");
        assert_eq!(
            vault.configured_venues().unwrap(),
            vec!["hyperliquid".to_string()]
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_vault_wrong_password_clears_all_caches() {
        let (dir, file) = temp_vault_path("wrong");
        let mut vault = SecureVault::new_with_path(file);
        vault.initialize("correct horse battery").unwrap();

        let err = vault.unlock("incorrect password!!", None).unwrap_err();
        assert!(err.to_string().contains("Incorrect password"));
        assert!(!vault.is_unlocked());
        assert!(vault.decrypted_cache.is_none());
        assert!(vault.encryption_key.is_none());
        assert!(vault.kdf_salt.is_none());

        let _ = fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn test_vault_file_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, file) = temp_vault_path("perm");
        let mut vault = SecureVault::new_with_path(file.clone());
        vault.initialize("correct horse battery").unwrap();

        let file_mode = fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(file_mode, 0o600);
        let dir_mode = fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_corrupt_vault_fails_closed_no_secret_material() {
        let (dir, file) = temp_vault_path("corrupt");
        fs::create_dir_all(&dir).unwrap();

        // Garbage without magic: falls to the legacy path and bails on
        // length/decrypt — never a panic, never an unlock.
        fs::write(&file, b"not-a-vault-file-garbage-bytes").unwrap();
        let mut vault = SecureVault::new_with_path(file.clone());
        assert!(vault.is_initialized(), "file exists = initialized marker");
        assert!(vault.unlock("correct horse battery", None).is_err());
        assert!(!vault.is_unlocked());
        assert!(vault.get_data().is_err());

        // Valid magic but truncated header.
        let mut truncated = VAULT_MAGIC.to_vec();
        truncated.extend_from_slice(&[0u8; 4]);
        fs::write(&file, &truncated).unwrap();
        let mut vault = SecureVault::new_with_path(file.clone());
        assert!(vault.unlock("correct horse battery", None).is_err());
        assert!(!vault.is_unlocked());

        // Real vault, then ciphertext corruption — GCM tag rejects it and
        // no partial plaintext reaches the cache.
        fs::remove_file(&file).unwrap();
        let mut vault = SecureVault::new_with_path(file.clone());
        vault.initialize("correct horse battery").unwrap();
        let mut bytes = fs::read(&file).unwrap();
        let n = bytes.len();
        bytes[n - 1] ^= 0xff;
        bytes[n - 5] ^= 0x01;
        fs::write(&file, &bytes).unwrap();
        let mut vault = SecureVault::new_with_path(file);
        let err = vault.unlock("correct horse battery", None).unwrap_err();
        assert!(
            err.to_string()
                .contains("Incorrect password or corrupted vault"),
            "{err}"
        );
        assert!(!vault.is_unlocked());
        assert!(vault.decrypted_cache.is_none());
        assert!(vault.encryption_key.is_none());

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_legacy_vault_migrates_to_v2() {
        let (dir, file) = temp_vault_path("legacy");

        // Construct a legacy fixture: salt || nonce || AES-GCM ciphertext, no magic/AAD
        let legacy_data = VaultData {
            credentials: vec![ExchangeCredentials {
                venue: "lighter".into(),
                profile_id: None,
                api_key: "legacy_key".into(),
                api_secret: "legacy_secret".into(),
                passphrase: None,
                account_id: None,
                wallet_address: None,
                account_index: Some(7),
                subaccount_index: None,
                api_key_index: None,
                private_key: None,
                base_url: None,

                enabled: true,
            }],
            updated_at: Utc::now(),
            mfa_totp_secret: None,
            mfa_last_accepted_step: None,
        };
        let plaintext = serde_json::to_vec(&legacy_data).unwrap();
        let mut salt = [0u8; SALT_LEN];
        let mut nonce_bytes = [0u8; NONCE_LEN];
        rand::thread_rng().fill_bytes(&mut salt);
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let legacy_key = SecureVault::derive_legacy_key("legacy password ok", &salt);
        let cipher = Aes256Gcm::new_from_slice(&*legacy_key).unwrap();
        #[allow(deprecated)]
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = cipher.encrypt(nonce, plaintext.as_ref()).unwrap();
        let mut legacy_bytes = Vec::new();
        legacy_bytes.extend_from_slice(&salt);
        legacy_bytes.extend_from_slice(&nonce_bytes);
        legacy_bytes.extend_from_slice(&ciphertext);
        fs::create_dir_all(&dir).unwrap();
        fs::write(&file, &legacy_bytes).unwrap();

        let mut vault = SecureVault::new_with_path(file.clone());
        vault.unlock("legacy password ok", None).unwrap();
        assert!(vault.is_unlocked());
        let stored = vault.get_data().unwrap();
        assert_eq!(stored.credentials.len(), 1);
        assert_eq!(stored.credentials[0].api_key, "legacy_key");
        assert_eq!(stored.credentials[0].account_index, Some(7));

        let migrated = fs::read(&file).unwrap();
        assert!(migrated.starts_with(VAULT_MAGIC.as_slice()));

        // Re-unlock against the migrated v2 file from a fresh vault instance
        let mut vault2 = SecureVault::new_with_path(file.clone());
        vault2.lock();
        vault2.unlock("legacy password ok", None).unwrap();
        assert_eq!(
            vault2.configured_venues().unwrap(),
            vec!["lighter".to_string()]
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_credentials_debug_never_leaks_secrets() {
        let creds = ExchangeCredentials {
            venue: "hyperliquid".into(),
            profile_id: None,
            api_key: "SENTINEL_KEY_9f3".into(),
            api_secret: "SENTINEL_SECRET_2b7".into(),
            passphrase: Some("SENTINEL_PASS_c41".into()),
            account_id: Some("SENTINEL_ACCT_d88".into()),
            wallet_address: Some("SENTINEL_WALLET_e5a".into()),
            account_index: Some(3),
            subaccount_index: Some(9),
            api_key_index: None,
            private_key: Some("SENTINEL_PRIVKEY_3f9".into()),
            // Endpoint URLs can embed provider API keys — never Debug them.
            base_url: Some("SENTINEL_RPCURL_b2c".into()),

            enabled: true,
        };
        let debug = format!("{:?}", creds);
        for sentinel in [
            "SENTINEL_KEY_9f3",
            "SENTINEL_SECRET_2b7",
            "SENTINEL_PASS_c41",
            "SENTINEL_ACCT_d88",
            "SENTINEL_WALLET_e5a",
            "SENTINEL_PRIVKEY_3f9",
            "SENTINEL_RPCURL_b2c",
        ] {
            assert!(
                !debug.contains(sentinel),
                "Debug output leaked {}",
                sentinel
            );
        }
        assert!(debug.contains("hyperliquid"));
        assert!(debug.contains("has_api_key: true"));
        assert!(debug.contains("has_api_secret: true"));
        assert!(debug.contains("has_passphrase: true"));
        assert!(debug.contains("has_private_key: true"));
        assert!(debug.contains("has_base_url: true"));
    }

    #[test]
    fn test_upsert_credentials_merge_semantics() {
        let (dir, file) = temp_vault_path("upsert");
        let mut vault = SecureVault::new_with_path(file);
        vault.initialize("correct horse battery").unwrap();

        vault
            .upsert_credentials(vec![ExchangeCredentials {
                venue: "HyperLiquid".into(),
                profile_id: None,
                api_key: "key_v1".into(),
                api_secret: "secret_v1".into(),
                passphrase: None,
                account_id: None,
                wallet_address: Some("0xaaa".into()),
                account_index: Some(1),
                subaccount_index: None,
                api_key_index: None,
                private_key: None,
                base_url: None,

                enabled: true,
            }])
            .unwrap();

        // Merge: new secret replaces, empty key preserves, indexes/enabled always update
        vault
            .upsert_credentials(vec![ExchangeCredentials {
                venue: "hyperliquid".into(),
                profile_id: None,
                api_key: "".into(),
                api_secret: "secret_v2".into(),
                passphrase: None,
                account_id: None,
                wallet_address: None,
                account_index: Some(2),
                subaccount_index: Some(5),
                api_key_index: None,
                private_key: None,
                base_url: None,

                enabled: false,
            }])
            .unwrap();

        let stored = vault.get_data().unwrap();
        assert_eq!(stored.credentials.len(), 1);
        let c = &stored.credentials[0];
        assert_eq!(c.api_key, "key_v1");
        assert_eq!(c.api_secret, "secret_v2");
        assert_eq!(c.wallet_address.as_deref(), Some("0xaaa"));
        assert_eq!(c.account_index, Some(2));
        assert_eq!(c.subaccount_index, Some(5));
        assert!(!c.enabled);
        assert_eq!(
            vault.configured_venues().unwrap(),
            vec!["hyperliquid".to_string()]
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_update_enabled_toggles_without_deleting_secrets() {
        let (dir, file) = temp_vault_path("enabled");
        let mut vault = SecureVault::new_with_path(file);
        vault.initialize("correct horse battery").unwrap();

        vault
            .upsert_credentials(vec![ExchangeCredentials {
                venue: "hyperliquid".into(),
                profile_id: None,
                api_key: "".into(),
                api_secret: "SENTINEL_SECRET_keep".into(),
                passphrase: None,
                account_id: None,
                wallet_address: Some("0xkeep".into()),
                account_index: Some(1),
                subaccount_index: None,
                api_key_index: None,
                private_key: None,
                base_url: None,

                enabled: true,
            }])
            .unwrap();
        assert_eq!(
            vault.configured_venues().unwrap(),
            vec!["hyperliquid".to_string()]
        );

        // Toggle off: the stored record stays discoverable while its internal
        // enabled flag flips; the secret remains stored
        vault
            .update_enabled(&[("HYPERLIQUID".to_string(), false)])
            .unwrap();
        assert_eq!(
            vault.configured_venues().unwrap(),
            vec!["hyperliquid".to_string()]
        );
        let stored = vault.get_data().unwrap();
        assert_eq!(stored.credentials[0].api_secret, "SENTINEL_SECRET_keep");
        assert!(!stored.credentials[0].enabled);

        // A disabled record can still be deleted
        assert!(vault.delete_credentials("hyperliquid").unwrap());
        assert!(vault.configured_venues().unwrap().is_empty());

        // Recreate and toggle back on: metadata restored, secret intact
        vault
            .upsert_credentials(vec![ExchangeCredentials {
                venue: "hyperliquid".into(),
                profile_id: None,
                api_key: "".into(),
                api_secret: "SENTINEL_SECRET_keep".into(),
                passphrase: None,
                account_id: None,
                wallet_address: Some("0xkeep".into()),
                account_index: Some(1),
                subaccount_index: None,
                api_key_index: None,
                private_key: None,
                base_url: None,

                enabled: true,
            }])
            .unwrap();
        vault
            .update_enabled(&[("hyperliquid".to_string(), true)])
            .unwrap();
        assert_eq!(
            vault.configured_venues().unwrap(),
            vec!["hyperliquid".to_string()]
        );
        let stored = vault.get_data().unwrap();
        assert_eq!(stored.credentials[0].api_secret, "SENTINEL_SECRET_keep");
        assert!(stored.credentials[0].enabled);

        // Unknown venues are ignored without error
        vault
            .update_enabled(&[("kraken_spot".to_string(), true)])
            .unwrap();

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_delete_credentials_persists_across_lock_cycle() {
        let (dir, file) = temp_vault_path("delete");
        let mut vault = SecureVault::new_with_path(file.clone());
        vault.initialize("correct horse battery").unwrap();
        vault
            .upsert_credentials(vec![
                ExchangeCredentials {
                    venue: "hyperliquid".into(),
                    profile_id: None,
                    api_key: "hl_key".into(),
                    api_secret: "hl_secret".into(),
                    passphrase: None,
                    account_id: None,
                    wallet_address: Some("0xhl".into()),
                    account_index: Some(1),
                    subaccount_index: None,
                    api_key_index: None,
                    private_key: None,
                    base_url: None,

                    enabled: true,
                },
                ExchangeCredentials {
                    venue: "lighter".into(),
                    profile_id: None,
                    api_key: "lgt_key".into(),
                    api_secret: "lgt_secret".into(),
                    passphrase: None,
                    account_id: None,
                    wallet_address: None,
                    account_index: None,
                    subaccount_index: None,
                    api_key_index: None,
                    private_key: None,
                    base_url: None,

                    enabled: true,
                },
            ])
            .unwrap();

        assert!(vault.delete_credentials(" HyperLiquid ").unwrap());
        assert_eq!(
            vault.configured_venues().unwrap(),
            vec!["lighter".to_string()]
        );

        vault.lock();
        let mut vault2 = SecureVault::new_with_path(file.clone());
        vault2.unlock("correct horse battery", None).unwrap();
        let stored = vault2.get_data().unwrap();
        assert_eq!(stored.credentials.len(), 1);
        assert_eq!(stored.credentials[0].venue, "lighter");
        assert_eq!(
            vault2.configured_venues().unwrap(),
            vec!["lighter".to_string()]
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_delete_credentials_unknown_or_absent_returns_false() {
        let (dir, file) = temp_vault_path("delete_none");
        let mut vault = SecureVault::new_with_path(file);
        vault.initialize("correct horse battery").unwrap();

        assert!(!vault.delete_credentials("kalshi").unwrap());
        assert!(vault.get_data().unwrap().credentials.is_empty());

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_delete_credentials_rejects_invalid_and_locked() {
        let (dir, file) = temp_vault_path("delete_invalid");
        let mut vault = SecureVault::new_with_path(file.clone());
        vault.initialize("correct horse battery").unwrap();
        vault
            .upsert_credentials(vec![ExchangeCredentials {
                venue: "hyperliquid".into(),
                profile_id: None,
                api_key: "k".into(),
                api_secret: "s".into(),
                passphrase: None,
                account_id: None,
                wallet_address: None,
                account_index: None,
                subaccount_index: None,
                api_key_index: None,
                private_key: None,
                base_url: None,

                enabled: true,
            }])
            .unwrap();

        for bad in [
            "",
            "   ",
            "hyper liquid",
            "hyperliquid;drop",
            "../etc/passwd",
        ] {
            assert!(
                vault.delete_credentials(bad).is_err(),
                "delete accepted invalid venue {:?}",
                bad
            );
        }
        assert_eq!(vault.get_data().unwrap().credentials.len(), 1);

        vault.lock();
        let err = vault.delete_credentials("hyperliquid").unwrap_err();
        assert!(err.to_string().contains("locked"));

        let mut vault2 = SecureVault::new_with_path(file.clone());
        vault2.unlock("correct horse battery", None).unwrap();
        assert_eq!(vault2.get_data().unwrap().credentials.len(), 1);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_mfa_enrollment_gates_unlock() {
        let (dir, file) = temp_vault_path("mfa_unlock");
        let mut vault = SecureVault::new_with_path(file.clone());
        vault.initialize("correct horse battery").unwrap();

        // Enroll a known secret so codes are deterministic.
        let secret = crate::totp::generate_secret();
        let secret_b32 = crate::totp::encode_secret_base32(&*secret);
        vault.set_mfa_secret(secret_b32).expect("enroll");
        assert!(vault.mfa_enrolled());

        vault.lock();

        // Password alone is not enough — the factor is mandatory.
        let err = vault.unlock("correct horse battery", None).unwrap_err();
        assert!(err.to_string().contains("Two-factor"), "got: {err}");
        assert!(!vault.is_unlocked());

        // A wrong code still refuses.
        let err = vault
            .unlock("correct horse battery", Some("000000"))
            .unwrap_err();
        assert!(
            err.to_string().contains("Invalid authentication code"),
            "got: {err}"
        );
        assert!(!vault.is_unlocked());

        // A live code from the enrolled secret unlocks.
        let code = crate::totp::code_at(&*secret, crate::totp::unix_now());
        vault
            .unlock("correct horse battery", Some(&code))
            .expect("unlock with TOTP");
        assert!(vault.is_unlocked());
        assert!(vault.mfa_enrolled());
        assert!(vault.get_status().mfa_enabled);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_mfa_secret_persists_across_vault_instances() {
        let (dir, file) = temp_vault_path("mfa_persist");
        let secret = crate::totp::generate_secret();
        let secret_b32 = crate::totp::encode_secret_base32(&*secret);
        {
            let mut vault = SecureVault::new_with_path(file.clone());
            vault.initialize("correct horse battery").unwrap();
            vault.set_mfa_secret(secret_b32).expect("enroll");
        }
        // A fresh handle on the same file must still demand the factor.
        let mut reopened = SecureVault::new_with_path(file.clone());
        assert!(reopened.unlock("correct horse battery", None).is_err());
        let code = crate::totp::code_at(&*secret, crate::totp::unix_now());
        reopened
            .unlock("correct horse battery", Some(&code))
            .expect("unlock with TOTP");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_mfa_clear_restores_password_only_unlock() {
        let (dir, file) = temp_vault_path("mfa_clear");
        let mut vault = SecureVault::new_with_path(file.clone());
        vault.initialize("correct horse battery").unwrap();
        let secret = crate::totp::generate_secret();
        vault
            .set_mfa_secret(crate::totp::encode_secret_base32(&*secret))
            .unwrap();

        // Disable path: a live code verifies, then the secret is cleared.
        let code = crate::totp::code_at(&*secret, crate::totp::unix_now());
        assert!(vault.verify_mfa_code(&code).unwrap());
        vault.clear_mfa_secret().unwrap();
        assert!(!vault.mfa_enrolled());
        assert!(vault.verify_mfa_code(&code).is_err());

        vault.lock();
        vault
            .unlock("correct horse battery", None)
            .expect("password-only unlock after disable");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_mfa_gates_password_rotation() {
        let (dir, file) = temp_vault_path("mfa_rotate");
        let mut vault = SecureVault::new_with_path(file.clone());
        vault.initialize("old-password-1234").unwrap();
        let secret = crate::totp::generate_secret();
        vault
            .set_mfa_secret(crate::totp::encode_secret_base32(&*secret))
            .unwrap();

        // Without a code, rotation refuses even with the right password.
        assert!(vault
            .prepare_password_rotation("old-password-1234", "new-password-5678", None)
            .is_err());
        // A live code lets the rotation prepare.
        let code = crate::totp::code_at(&*secret, crate::totp::unix_now());
        vault
            .prepare_password_rotation("old-password-1234", "new-password-5678", Some(&code))
            .expect("rotation gated by TOTP");
        let _ = fs::remove_dir_all(dir);
    }
}
