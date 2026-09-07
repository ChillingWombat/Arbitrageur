use std::fs;
use std::path::PathBuf;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use hmac::Hmac;
use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use rand::RngCore;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExchangeCredentials {
    pub venue: String,
    pub api_key: String,
    pub api_secret: String,
    pub passphrase: Option<String>,
    pub account_index: Option<u32>,
    pub subaccount_index: Option<u32>,
    pub enabled: bool,
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
}

impl SecureVault {
    pub fn new_with_path(path: PathBuf) -> Self {
        Self {
            storage_path: path,
            is_unlocked: false,
            unlocked_until: None,
            decrypted_cache: None,
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
    }

    pub fn get_status(&mut self) -> VaultStatus {
        let unlocked = self.is_unlocked();
        VaultStatus {
            is_initialized: self.is_initialized(),
            is_unlocked: unlocked,
            unlocked_until: self.unlocked_until,
        }
    }

    /// Derive 32-byte encryption key from password and salt using PBKDF2-HMAC-SHA256
    fn derive_key(password: &str, salt: &[u8]) -> [u8; 32] {
        let mut key = [0u8; 32];
        let hmac_signer = |data: &[u8]| -> [u8; 32] {
            use hmac::Mac;
            let mut mac = <HmacSha256 as Mac>::new_from_slice(password.as_bytes()).expect("HMAC can take key of any size");
            mac.update(data);
            let result = mac.finalize();
            let mut out = [0u8; 32];
            out.copy_from_slice(&result.into_bytes());
            out
        };

        // PBKDF2 with 10,000 rounds
        let mut u_prev = {
            let mut block_input = Vec::with_capacity(salt.len() + 4);
            block_input.extend_from_slice(salt);
            block_input.extend_from_slice(&1u32.to_be_bytes());
            hmac_signer(&block_input)
        };
        key.copy_from_slice(&u_prev);

        for _ in 1..10_000 {
            u_prev = hmac_signer(&u_prev);
            for i in 0..32 {
                key[i] ^= u_prev[i];
            }
        }
        key
    }

    /// Initialize a new vault with master password
    pub fn initialize(&mut self, password: &str) -> anyhow::Result<()> {
        if password.len() < 6 {
            anyhow::bail!("Master password must be at least 6 characters long");
        }
        let empty_data = VaultData {
            credentials: Vec::new(),
            updated_at: Utc::now(),
        };
        self.save_encrypted(&empty_data, password)?;
        self.is_unlocked = true;
        self.unlocked_until = Some(Utc::now() + chrono::Duration::minutes(15));
        self.decrypted_cache = Some(empty_data);
        Ok(())
    }

    /// Unlock existing vault with master password
    pub fn unlock(&mut self, password: &str) -> anyhow::Result<()> {
        if !self.storage_path.exists() {
            anyhow::bail!("Vault is not initialized. Please set a master password first.");
        }
        let file_bytes = fs::read(&self.storage_path)?;
        if file_bytes.len() < 16 + 12 + 16 {
            anyhow::bail!("Vault file is corrupted or incomplete");
        }

        let salt = &file_bytes[0..16];
        let nonce_bytes = &file_bytes[16..28];
        let ciphertext = &file_bytes[28..];

        let key = Self::derive_key(password, salt);
        let cipher = Aes256Gcm::new_from_slice(&key)
            .map_err(|e| anyhow::anyhow!("Cipher init error: {}", e))?;
        #[allow(deprecated)]
        let nonce = Nonce::from_slice(nonce_bytes);

        let decrypted_bytes = cipher
            .decrypt(nonce, ciphertext)
            .map_err(|_| anyhow::anyhow!("Incorrect master password or corrupted vault"))?;

        let data: VaultData = serde_json::from_slice(&decrypted_bytes)?;
        self.is_unlocked = true;
        self.unlocked_until = Some(Utc::now() + chrono::Duration::minutes(15));
        self.decrypted_cache = Some(data);
        Ok(())
    }

    /// Save updated credentials to encrypted storage
    pub fn save_data(&mut self, data: &VaultData, password: &str) -> anyhow::Result<()> {
        if !self.is_unlocked() {
            anyhow::bail!("Vault is locked. Please unlock before saving.");
        }
        self.save_encrypted(data, password)?;
        self.decrypted_cache = Some(data.clone());
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

    fn save_encrypted(&self, data: &VaultData, password: &str) -> anyhow::Result<()> {
        let plaintext = serde_json::to_vec(data)?;

        let mut salt = [0u8; 16];
        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut salt);
        rand::thread_rng().fill_bytes(&mut nonce_bytes);

        let key = Self::derive_key(password, &salt);
        let cipher = Aes256Gcm::new_from_slice(&key)
            .map_err(|e| anyhow::anyhow!("Cipher init error: {}", e))?;
        #[allow(deprecated)]
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = cipher
            .encrypt(nonce, plaintext.as_ref())
            .map_err(|e| anyhow::anyhow!("Encryption failed: {}", e))?;

        let mut output = Vec::with_capacity(salt.len() + nonce_bytes.len() + ciphertext.len());
        output.extend_from_slice(&salt);
        output.extend_from_slice(&nonce_bytes);
        output.extend_from_slice(&ciphertext);

        if let Some(parent) = self.storage_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&self.storage_path, output)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vault_initialize_unlock_lock() {
        let temp_dir = std::env::temp_dir().join(format!("arbitrager_vault_test_{}", rand::random::<u64>()));
        let vault_file = temp_dir.join("vault.enc");

        let mut vault = SecureVault::new_with_path(vault_file.clone());
        assert!(!vault.is_initialized());

        // Initialize
        assert!(vault.initialize("strong_password123").is_ok());
        assert!(vault.is_initialized());
        assert!(vault.is_unlocked());

        // Lock
        vault.lock();
        assert!(!vault.is_unlocked());
        assert!(vault.get_data().is_err());

        // Unlock with wrong password
        assert!(vault.unlock("wrong_pwd").is_err());
        assert!(!vault.is_unlocked());

        // Unlock with correct password
        assert!(vault.unlock("strong_password123").is_ok());
        assert!(vault.is_unlocked());

        // Save credential
        let mut data = vault.get_data().unwrap().clone();
        data.credentials.push(ExchangeCredentials {
            venue: "hyperliquid".into(),
            api_key: "0x123abc".into(),
            api_secret: "secret_456".into(),
            passphrase: None,
            account_index: Some(0),
            subaccount_index: Some(1),
            enabled: true,
        });
        assert!(vault.save_data(&data, "strong_password123").is_ok());

        // Clean up
        let _ = fs::remove_dir_all(temp_dir);
    }
}
