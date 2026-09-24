//! Stronghold-backed custody for the shared EVM wallet signing key.
//!
//! The 11 admitted EVM AMM venues share one secp256k1 wallet. With this
//! module the raw key bytes never enter the app's process memory: they are
//! written into an IOTA Stronghold vault (guarded memory region, encrypted
//! at rest in a snapshot file next to the app vault) and every signature is
//! produced by executing Stronghold's `Secp256k1EcdsaSign` procedure inside
//! the vault. The exchange adapters receive a `StrongholdEvmSigner` — an
//! alloy `TxSigner` whose `sign_transaction` call is the only operation
//! that ever touches the key.
//!
//! ## Credential record sentinel
//!
//! Vault credential records for AMM venues carry `private_key =
//! "stronghold:evm-wallet"` (see `EVM_EXTERNAL_SIGNER_PREFIX` in the
//! exchanges crate). The sentinel keeps `has_private_key` presence flags
//! truthful while containing no key material. On credential sync the
//! adapter marks itself `expects_external_signer` and this module injects
//! the signer via `provision_external_signer`.
//!
//! ## Lock semantics
//!
//! The snapshot key is derived from the vault's session key, so the
//! stronghold only exists while the vault is unlocked. Auto-lock drops the
//! vault's decrypted cache but keeps provisioned adapter credentials — the
//! stronghold stays loaded (the signer IS the provisioned credential).
//! Manual lock drops the stronghold and the sync wipes adapter signers.

use std::ops::Deref;
use std::path::Path;
use std::sync::Arc;

use alloy_consensus::SignableTransaction;
use alloy_network::TxSigner;
use alloy_primitives::{Address, ChainId, Signature, B256};
use iota_stronghold::procedures::{
    GetEvmAddress, RevokeData, Secp256k1EcdsaFlavor, Secp256k1EcdsaSign,
};
use iota_stronghold::{Client, ClientError, Location};
use tauri_plugin_stronghold::stronghold::Stronghold;
use zeroize::Zeroizing;

/// Sentinel stored in a vault credential record's `private_key` field when
/// the shared EVM key is held in Stronghold. Must start with
/// `EVM_EXTERNAL_SIGNER_PREFIX` so the adapter routes it to external
/// provisioning instead of parsing it as hex.
pub const EVM_STRONGHOLD_KEY_REF: &str = "stronghold:evm-wallet";

/// Stronghold client path — one client owns the EVM wallet vault.
const CLIENT_PATH: &[u8] = b"evm-wallet";
/// Vault path and record path of the single shared secp256k1 key.
const VAULT_PATH: &[u8] = b"evm-wallet";
const RECORD_PATH: &[u8] = b"secp256k1-key";

/// Vault-record location of the shared EVM private key.
fn key_location() -> Location {
    Location::generic(VAULT_PATH.to_vec(), RECORD_PATH.to_vec())
}

/// Backend storage for the wallet stronghold: a password-encrypted
/// snapshot file in production, in-memory only for tests.
enum Backend {
    Snapshot(Stronghold),
    #[cfg(test)]
    InMemory(iota_stronghold::Stronghold),
}

impl Deref for Backend {
    type Target = iota_stronghold::Stronghold;
    fn deref(&self) -> &Self::Target {
        match self {
            Self::Snapshot(s) => s,
            #[cfg(test)]
            Self::InMemory(s) => s,
        }
    }
}

/// An opened wallet stronghold: snapshot wrapper plus the loaded EVM
/// client. Cloning a [`Client`] yields another handle into the same vault
/// memory, which is what the per-venue signers hold.
pub struct EvmStronghold {
    backend: Backend,
    client: Client,
}

impl EvmStronghold {
    /// Open (or create) the snapshot at `path` with a 32-byte `password`
    /// derived from the vault session key, then load the EVM client.
    /// `Stronghold::new` decrypts an existing snapshot; a missing file
    /// starts empty and is created on the first `save()`.
    pub fn open(path: &Path, password: &[u8; 32]) -> anyhow::Result<Self> {
        let sh = Stronghold::new(path, password.to_vec())
            .map_err(|e| anyhow::anyhow!("stronghold open failed: {e}"))?;
        let client = load_or_create_client(&sh)?;
        Ok(Self {
            backend: Backend::Snapshot(sh),
            client,
        })
    }

    /// In-memory stronghold for tests — nothing touches disk.
    #[cfg(test)]
    pub fn in_memory() -> anyhow::Result<Self> {
        let sh = iota_stronghold::Stronghold::default();
        let client = load_or_create_client(&sh)?;
        Ok(Self {
            backend: Backend::InMemory(sh),
            client,
        })
    }

    /// Persist the snapshot (no-op for the in-memory backend).
    fn save(&self) -> anyhow::Result<()> {
        match &self.backend {
            Backend::Snapshot(sh) => sh
                .save()
                .map_err(|e| anyhow::anyhow!("stronghold snapshot commit failed: {e}"))?,
            #[cfg(test)]
            Backend::InMemory(_) => {}
        }
        Ok(())
    }

    /// Store the raw 32-byte secp256k1 key in the vault record and commit
    /// the snapshot. The caller's copy is consumed by value.
    pub fn store_key(&self, key: Zeroizing<[u8; 32]>) -> anyhow::Result<()> {
        self.client
            .vault(VAULT_PATH)
            .write_secret(key_location(), Zeroizing::new(key.to_vec()))
            .map_err(|e| anyhow::anyhow!("stronghold key write failed: {e}"))?;
        self.save()
    }

    /// Whether the vault record exists (a shared key has been stored).
    pub fn has_key(&self) -> bool {
        self.client.record_exists(&key_location()).unwrap_or(false)
    }

    /// Revoke the key record and garbage-collect it, then commit. The
    /// record becomes unreadable; adapters holding signers built on it
    /// fail closed on their next sign call.
    pub fn revoke_key(&self) -> anyhow::Result<()> {
        self.client
            .execute_procedure(RevokeData {
                location: key_location(),
                should_gc: true,
            })
            .map_err(|e| anyhow::anyhow!("stronghold key revoke failed: {e}"))?;
        self.save()
    }

    /// Derive the EVM address from the stored key inside the vault — the
    /// key never leaves guarded memory.
    pub fn evm_address(&self) -> anyhow::Result<Address> {
        let out: [u8; 20] = self
            .client
            .execute_procedure(GetEvmAddress {
                private_key: key_location(),
            })
            .map_err(|e| anyhow::anyhow!("stronghold GetEvmAddress failed: {e}"))?;
        Ok(Address::from_slice(&out))
    }

    /// Build a `TxSigner` handle over the stored key for one venue's
    /// chain. `chain_id` is optional; when set it is enforced on every
    /// signed transaction (mismatch refuses, matching `PrivateKeySigner`
    /// semantics).
    pub fn signer(
        &self,
        chain_id: Option<ChainId>,
    ) -> anyhow::Result<Arc<dyn TxSigner<Signature> + Send + Sync>> {
        let address = self.evm_address()?;
        Ok(Arc::new(StrongholdEvmSigner {
            client: self.client.clone(),
            location: key_location(),
            address,
            chain_id,
        }))
    }
}

/// Load the EVM client from snapshot data, create it on a fresh
/// stronghold, or reattach to an already-loaded one.
fn load_or_create_client(sh: &iota_stronghold::Stronghold) -> anyhow::Result<Client> {
    match sh.load_client(CLIENT_PATH) {
        Ok(c) => Ok(c),
        Err(ClientError::ClientDataNotPresent) => sh
            .create_client(CLIENT_PATH)
            .map_err(|e| anyhow::anyhow!("stronghold client create failed: {e}")),
        Err(ClientError::ClientAlreadyLoaded(_)) => sh
            .get_client(CLIENT_PATH)
            .map_err(|e| anyhow::anyhow!("stronghold client lookup failed: {e}")),
        Err(e) => Err(anyhow::anyhow!("stronghold client load failed: {e}")),
    }
}

/// Alloy `TxSigner` that signs EVM transactions inside Stronghold. Holds
/// no key material — only a `Client` handle and the record `Location`.
///
/// Signing path: `encode_for_signing` produces the transaction's RLP
/// signing preimage; Stronghold's `Secp256k1EcdsaSign` with the
/// `Keccak256` flavor computes `keccak256(preimage)` (= the tx
/// `signature_hash`) and returns `r‖s‖recovery_id` — identical to what
/// `PrivateKeySigner::sign_transaction` produces.
pub struct StrongholdEvmSigner {
    client: Client,
    location: Location,
    address: Address,
    chain_id: Option<ChainId>,
}

impl std::fmt::Debug for StrongholdEvmSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StrongholdEvmSigner")
            .field("address", &self.address)
            .field("chain_id", &self.chain_id)
            // Never print the record location or client internals.
            .finish_non_exhaustive()
    }
}

#[async_trait::async_trait]
impl TxSigner<Signature> for StrongholdEvmSigner {
    fn address(&self) -> Address {
        self.address
    }

    #[inline]
    async fn sign_transaction(
        &self,
        tx: &mut dyn SignableTransaction<Signature>,
    ) -> alloy_signer::Result<Signature> {
        // Mirror `sign_transaction_with_chain_id!`: apply the signer's
        // chain id when set and refuse a mismatch.
        if let Some(chain_id) = self.chain_id {
            if !tx.set_chain_id_checked(chain_id) {
                return Err(alloy_signer::Error::TransactionChainIdMismatch {
                    signer: chain_id,
                    tx: tx.chain_id().unwrap_or_default(),
                });
            }
        }
        // Secp256k1EcdsaSign hashes `msg` with keccak256 internally — the
        // unsigned signing preimage goes in, NOT the precomputed digest
        // (a digest would be hashed twice and produce a garbage signer).
        let preimage = tx.encoded_for_signing();
        let sig: [u8; 65] = self
            .client
            .execute_procedure(Secp256k1EcdsaSign {
                flavor: Secp256k1EcdsaFlavor::Keccak256,
                msg: preimage,
                private_key: self.location.clone(),
            })
            .map_err(|e| {
                alloy_signer::Error::other(format!("stronghold secp256k1 sign failed: {e}"))
            })?;
        Ok(Signature::from_scalars_and_parity(
            B256::from_slice(&sig[0..32]),
            B256::from_slice(&sig[32..64]),
            sig[64] != 0,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::TxEip1559;
    use alloy_primitives::{address, Bytes, TxKind, U256};

    const TEST_KEY: [u8; 32] = [7u8; 32];

    fn open_with_key() -> EvmStronghold {
        let sh = EvmStronghold::in_memory().expect("in-memory stronghold");
        sh.store_key(Zeroizing::new(TEST_KEY)).expect("store key");
        sh
    }

    fn local_signer() -> alloy_signer_local::PrivateKeySigner {
        alloy_signer_local::PrivateKeySigner::from_slice(&TEST_KEY).expect("local signer")
    }

    fn sample_tx() -> TxEip1559 {
        TxEip1559 {
            chain_id: 1,
            nonce: 3,
            gas_limit: 21000,
            max_fee_per_gas: 20_000_000_000,
            max_priority_fee_per_gas: 1_000_000_000,
            to: TxKind::Call(address!("d8dA6BF26964aF9D7eEd9e03E53415D37aA96045")),
            value: U256::from(12345u64),
            access_list: Default::default(),
            input: Bytes::from(vec![0xaa, 0xbb]),
        }
    }

    /// The whole point of the upgrade: a Stronghold-produced signature must
    /// be byte-identical to what `PrivateKeySigner` (the current in-memory
    /// signer) produces for the same transaction — same `signature_hash`,
    /// same `r‖s`, same y-parity.
    #[tokio::test]
    async fn stronghold_signature_matches_private_key_signer() {
        let sh = open_with_key();
        let signer = sh.signer(Some(1)).expect("stronghold signer");
        let local = local_signer();

        let mut tx_sh = sample_tx();
        let mut tx_local = sample_tx();
        let sig_sh = signer
            .sign_transaction(&mut tx_sh)
            .await
            .expect("stronghold sign");
        let sig_local = local
            .sign_transaction(&mut tx_local)
            .await
            .expect("local sign");

        assert_eq!(sig_sh, sig_local);
        // Sanity: the signature recovers to the expected signer address on
        // the transaction's own sighash.
        let recovered = sig_local
            .recover_address_from_prehash(&tx_local.signature_hash())
            .expect("recover");
        assert_eq!(recovered, local.address());
    }

    #[tokio::test]
    async fn chain_id_mismatch_refuses() {
        let sh = open_with_key();
        let signer = sh.signer(Some(1)).expect("stronghold signer");
        let mut tx = sample_tx();
        tx.chain_id = 56; // BNB chain — mismatched
        let err = signer
            .sign_transaction(&mut tx)
            .await
            .expect_err("must refuse mismatched chain id");
        assert!(matches!(
            err,
            alloy_signer::Error::TransactionChainIdMismatch { .. }
        ));
    }

    #[test]
    fn stored_key_derives_same_address_as_local_signer() {
        let sh = open_with_key();
        assert_eq!(
            sh.evm_address().expect("evm address"),
            local_signer().address()
        );
    }

    #[test]
    fn missing_key_fails_closed() {
        let sh = EvmStronghold::in_memory().expect("in-memory stronghold");
        assert!(!sh.has_key());
        assert!(sh.evm_address().is_err());
        assert!(sh.signer(None).is_err());
    }

    #[test]
    fn revoke_makes_record_unreadable() {
        let sh = open_with_key();
        assert!(sh.has_key());
        sh.revoke_key().expect("revoke");
        assert!(!sh.has_key());
        assert!(sh.evm_address().is_err());
    }
}
