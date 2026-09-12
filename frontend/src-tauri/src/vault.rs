use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use argon2::{Algorithm, Argon2, Params, Version};
use chrono::{DateTime, Utc};
use hmac::Hmac;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
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
    pub api_key: String,
    pub api_secret: String,
    pub passphrase: Option<String>,
    #[serde(default)]
    pub account_id: Option<String>,
    #[serde(default)]
    pub wallet_address: Option<String>,
    pub account_index: Option<u32>,
    pub subaccount_index: Option<u32>,
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
    }
}

impl std::fmt::Debug for ExchangeCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExchangeCredentials")
            .field("venue", &self.venue)
            .field("account_index", &self.account_index)
            .field("subaccount_index", &self.subaccount_index)
            .field("enabled", &self.enabled)
            .field("has_api_key", &!self.api_key.is_empty())
            .field("has_api_secret", &!self.api_secret.is_empty())
            .field(
                "has_passphrase",
                &self.passphrase.as_ref().is_some_and(|p| !p.is_empty()),
            )
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VaultData {
    pub credentials: Vec<ExchangeCredentials>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultStatus {
    pub is_initialized: bool,
    pub is_unlocked: bool,
    pub unlocked_until: Option<DateTime<Utc>>,
}

pub struct SecureVault {
    storage_path: PathBuf,
    is_unlocked: bool,
    unlocked_until: Option<DateTime<Utc>>,
    decrypted_cache: Option<VaultData>,
    encryption_key: Option<Zeroizing<[u8; 32]>>,
    kdf_salt: Option<[u8; SALT_LEN]>,
}

impl SecureVault {
    pub fn new_with_path(path: PathBuf) -> Self {
        Self {
            storage_path: path,
            is_unlocked: false,
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
        base.join(".arbitrager").join("vault.enc")
    }

    pub fn is_initialized(&self) -> bool {
        self.storage_path.exists()
    }

    pub fn is_unlocked(&mut self) -> bool {
        if !self.is_unlocked {
            return false;
        }
        if let Some(until) = self.unlocked_until {
            if Utc::now() > until {
                // Auto-lock expired
                self.lock();
                return false;
            }
        }
        true
    }

    pub fn lock(&mut self) {
        self.is_unlocked = false;
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
            unlocked_until: self.unlocked_until,
        }
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
            use hmac::Mac;
            let mut mac = <HmacSha256 as Mac>::new_from_slice(password.as_bytes())
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

    /// Initialize a new vault with master password
    pub fn initialize(&mut self, password: &str) -> anyhow::Result<()> {
        if self.is_initialized() {
            anyhow::bail!(
                "Vault is already initialized; refusing to overwrite existing credentials"
            );
        }
        if password.chars().count() < MIN_PASSWORD_CHARS {
            anyhow::bail!("Master password must be at least 12 characters long");
        }

        let empty_data = VaultData {
            credentials: Vec::new(),
            updated_at: Utc::now(),
        };
        let mut salt = [0u8; SALT_LEN];
        rand::thread_rng().fill_bytes(&mut salt);
        let key = Self::derive_v2_key(password, &salt)?;
        let bytes = Self::encrypt_v2(&empty_data, &key, &salt)?;
        Self::write_file_atomic(&self.storage_path, &bytes)?;

        self.is_unlocked = true;
        self.unlocked_until = Some(Utc::now() + chrono::Duration::minutes(15));
        self.decrypted_cache = Some(empty_data);
        self.encryption_key = Some(key);
        self.kdf_salt = Some(salt);
        Ok(())
    }

    /// Unlock existing vault with master password. Legacy (unversioned PBKDF2)
    /// files are transparently migrated to the v2 format on success.
    pub fn unlock(&mut self, password: &str) -> anyhow::Result<()> {
        self.lock();
        if !self.storage_path.exists() {
            anyhow::bail!("Vault is not initialized. Please set a master password first.");
        }
        let file_bytes = fs::read(&self.storage_path)?;

        if file_bytes.starts_with(VAULT_MAGIC.as_slice()) {
            if file_bytes.len() < VAULT_MAGIC.len() + SALT_LEN + NONCE_LEN + 16 {
                anyhow::bail!("Incorrect master password or corrupted vault");
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
                    .map_err(|_| anyhow::anyhow!("Incorrect master password or corrupted vault"))?,
            );

            let data: VaultData = serde_json::from_slice(decrypted_bytes.as_slice())
                .map_err(|_| anyhow::anyhow!("Incorrect master password or corrupted vault"))?;

            let mut salt_arr = [0u8; SALT_LEN];
            salt_arr.copy_from_slice(salt);

            self.is_unlocked = true;
            self.unlocked_until = Some(Utc::now() + chrono::Duration::minutes(15));
            self.decrypted_cache = Some(data);
            self.encryption_key = Some(key);
            self.kdf_salt = Some(salt_arr);
            Ok(())
        } else {
            // Legacy format: salt || nonce || ciphertext, no magic, no AAD
            if file_bytes.len() < SALT_LEN + NONCE_LEN + 16 {
                anyhow::bail!("Incorrect master password or corrupted vault");
            }
            let salt = &file_bytes[0..SALT_LEN];
            let nonce_bytes = &file_bytes[SALT_LEN..SALT_LEN + NONCE_LEN];
            let ciphertext = &file_bytes[SALT_LEN + NONCE_LEN..];

            let legacy_key = Self::derive_legacy_key(password, salt);
            let cipher = Aes256Gcm::new_from_slice(&*legacy_key)
                .map_err(|e| anyhow::anyhow!("Cipher init error: {}", e))?;
            #[allow(deprecated)]
            let nonce = Nonce::from_slice(nonce_bytes);

            let decrypted_bytes =
                Zeroizing::new(cipher.decrypt(nonce, ciphertext).map_err(|_| {
                    anyhow::anyhow!("Incorrect master password or corrupted vault")
                })?);

            let data: VaultData = serde_json::from_slice(decrypted_bytes.as_slice())
                .map_err(|_| anyhow::anyhow!("Incorrect master password or corrupted vault"))?;
            drop(legacy_key);

            // Migrate to v2: re-encrypt with Argon2id and a fresh salt
            let mut new_salt = [0u8; SALT_LEN];
            rand::thread_rng().fill_bytes(&mut new_salt);
            let new_key = Self::derive_v2_key(password, &new_salt)?;
            let bytes = Self::encrypt_v2(&data, &new_key, &new_salt)?;
            Self::write_file_atomic(&self.storage_path, &bytes)?;

            self.is_unlocked = true;
            self.unlocked_until = Some(Utc::now() + chrono::Duration::minutes(15));
            self.decrypted_cache = Some(data);
            self.encryption_key = Some(new_key);
            self.kdf_salt = Some(new_salt);
            Ok(())
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
        self.unlocked_until = Some(Utc::now() + chrono::Duration::minutes(15));
        Ok(())
    }

    /// Read data from decrypted cache
    pub fn get_data(&mut self) -> anyhow::Result<&VaultData> {
        if !self.is_unlocked() {
            anyhow::bail!("Vault is locked. Please enter master password to view credentials.");
        }
        self.decrypted_cache
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Decrypted vault cache is empty"))
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

    /// Merge submitted credentials by exact lowercase venue ID and persist.
    /// Nonempty submitted values replace stored ones; empty strings and `None`
    /// preserve existing values. `enabled` and index fields always update.
    pub fn upsert_credentials(
        &mut self,
        submitted: Vec<ExchangeCredentials>,
    ) -> anyhow::Result<()> {
        if !self.is_unlocked() {
            anyhow::bail!("Vault is locked. Please unlock before saving credentials.");
        }
        let mut data = self.decrypted_cache.clone().unwrap_or_default();

        for sub in submitted {
            let venue_key = sub.venue.to_lowercase();
            if let Some(existing) = data
                .credentials
                .iter_mut()
                .find(|c| c.venue.to_lowercase() == venue_key)
            {
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
                existing.enabled = sub.enabled;
            } else {
                data.credentials.push(sub);
            }
        }

        data.updated_at = Utc::now();
        self.save_data(&data)
    }

    /// Update the enabled flag on existing credential records by lowercase
    /// venue ID. Unknown venues are ignored, stored secrets are never
    /// touched, and the vault is only rewritten when a flag changed.
    /// Requires an unlocked vault.
    pub fn update_enabled(&mut self, enablement: &[(String, bool)]) -> anyhow::Result<()> {
        if !self.is_unlocked() {
            anyhow::bail!("Vault is locked. Please unlock before updating credentials.");
        }
        let mut data = self.decrypted_cache.clone().unwrap_or_default();
        let mut changed = false;

        for (venue, enabled) in enablement {
            let venue_key = venue.to_lowercase();
            if let Some(existing) = data
                .credentials
                .iter_mut()
                .find(|c| c.venue.to_lowercase() == venue_key)
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
            "arbitrager_vault_test_{}_{}",
            tag,
            rand::random::<u64>()
        ));
        let file = dir.join("vault.enc");
        (dir, file)
    }

    #[test]
    fn test_vault_initialize_rejects_short_password_and_reinit() {
        let (dir, file) = temp_vault_path("init");
        let mut vault = SecureVault::new_with_path(file);
        assert!(!vault.is_initialized());

        let err = vault.initialize("short").unwrap_err();
        assert!(err.to_string().contains("at least 12 characters"));

        assert!(vault.initialize("a-valid-long-password").is_ok());
        let err = vault.initialize("another-long-password").unwrap_err();
        assert!(err.to_string().contains("already initialized"));

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
            api_key: "0x123abc".into(),
            api_secret: "secret_456".into(),
            passphrase: None,
            account_id: None,
            wallet_address: Some("0xwallet".into()),
            account_index: Some(0),
            subaccount_index: Some(1),
            enabled: true,
        });
        vault.save_data(&data).unwrap();

        let file_bytes = fs::read(&file).unwrap();
        assert!(file_bytes.starts_with(VAULT_MAGIC.as_slice()));

        vault.lock();
        assert!(!vault.is_unlocked());
        assert!(vault.get_data().is_err());
        assert!(vault.configured_venues().is_err());

        vault.unlock("correct horse battery").unwrap();
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

        let err = vault.unlock("incorrect password!!").unwrap_err();
        assert!(err.to_string().contains("Incorrect master password"));
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
    fn test_legacy_vault_migrates_to_v2() {
        let (dir, file) = temp_vault_path("legacy");

        // Construct a legacy fixture: salt || nonce || AES-GCM ciphertext, no magic/AAD
        let legacy_data = VaultData {
            credentials: vec![ExchangeCredentials {
                venue: "lighter".into(),
                api_key: "legacy_key".into(),
                api_secret: "legacy_secret".into(),
                passphrase: None,
                account_id: None,
                wallet_address: None,
                account_index: Some(7),
                subaccount_index: None,
                enabled: true,
            }],
            updated_at: Utc::now(),
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
        vault.unlock("legacy password ok").unwrap();
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
        vault2.unlock("legacy password ok").unwrap();
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
            api_key: "SENTINEL_KEY_9f3".into(),
            api_secret: "SENTINEL_SECRET_2b7".into(),
            passphrase: Some("SENTINEL_PASS_c41".into()),
            account_id: Some("SENTINEL_ACCT_d88".into()),
            wallet_address: Some("SENTINEL_WALLET_e5a".into()),
            account_index: Some(3),
            subaccount_index: Some(9),
            enabled: true,
        };
        let debug = format!("{:?}", creds);
        for sentinel in [
            "SENTINEL_KEY_9f3",
            "SENTINEL_SECRET_2b7",
            "SENTINEL_PASS_c41",
            "SENTINEL_ACCT_d88",
            "SENTINEL_WALLET_e5a",
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
    }

    #[test]
    fn test_upsert_credentials_merge_semantics() {
        let (dir, file) = temp_vault_path("upsert");
        let mut vault = SecureVault::new_with_path(file);
        vault.initialize("correct horse battery").unwrap();

        vault
            .upsert_credentials(vec![ExchangeCredentials {
                venue: "HyperLiquid".into(),
                api_key: "key_v1".into(),
                api_secret: "secret_v1".into(),
                passphrase: None,
                account_id: None,
                wallet_address: Some("0xaaa".into()),
                account_index: Some(1),
                subaccount_index: None,
                enabled: true,
            }])
            .unwrap();

        // Merge: new secret replaces, empty key preserves, indexes/enabled always update
        vault
            .upsert_credentials(vec![ExchangeCredentials {
                venue: "hyperliquid".into(),
                api_key: "".into(),
                api_secret: "secret_v2".into(),
                passphrase: None,
                account_id: None,
                wallet_address: None,
                account_index: Some(2),
                subaccount_index: Some(5),
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
                api_key: "".into(),
                api_secret: "SENTINEL_SECRET_keep".into(),
                passphrase: None,
                account_id: None,
                wallet_address: Some("0xkeep".into()),
                account_index: Some(1),
                subaccount_index: None,
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
                api_key: "".into(),
                api_secret: "SENTINEL_SECRET_keep".into(),
                passphrase: None,
                account_id: None,
                wallet_address: Some("0xkeep".into()),
                account_index: Some(1),
                subaccount_index: None,
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
                    api_key: "hl_key".into(),
                    api_secret: "hl_secret".into(),
                    passphrase: None,
                    account_id: None,
                    wallet_address: Some("0xhl".into()),
                    account_index: Some(1),
                    subaccount_index: None,
                    enabled: true,
                },
                ExchangeCredentials {
                    venue: "lighter".into(),
                    api_key: "lgt_key".into(),
                    api_secret: "lgt_secret".into(),
                    passphrase: None,
                    account_id: None,
                    wallet_address: None,
                    account_index: None,
                    subaccount_index: None,
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
        vault2.unlock("correct horse battery").unwrap();
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
                api_key: "k".into(),
                api_secret: "s".into(),
                passphrase: None,
                account_id: None,
                wallet_address: None,
                account_index: None,
                subaccount_index: None,
                enabled: true,
            }])
            .unwrap();

        for bad in ["", "   ", "hyper liquid", "hyperliquid;drop", "../etc/passwd"] {
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
        vault2.unlock("correct horse battery").unwrap();
        assert_eq!(vault2.get_data().unwrap().credentials.len(), 1);

        let _ = fs::remove_dir_all(dir);
    }
}
