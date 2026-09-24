//! Vault lifecycle, exchange credentials, account profiles, strategy
//! routing, and venue balances. Split out of `commands.rs`; the few shared
//! helpers it needs are reached via `crate::commands`.

use crate::commands::{
    live_account_or_cached, margin_monitor_from_settings, position_signature, stale_grace_ms,
};
use crate::state::{AppEngineState, VenueBalanceInfo};
use crate::state_store::{AccountProfile, StrategyAccountRoute};
use crate::vault::{CredentialPresence, ExchangeCredentials, VaultStatus};
use arbitrageur_exchanges::evm_amm::{
    ADMITTED_EVM_AMM_VENUE_IDS, EVM_EXTERNAL_SIGNER_PREFIX, LIVE_EVM_AMM_VENUE_IDS,
};
use arbitrageur_risk::margin_monitor::MarginHealthStatus;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::State;

pub fn get_vault_status_impl(state: &AppEngineState) -> VaultStatus {
    let mut vault = state.vault.write();
    vault.get_status()
}

pub fn initialize_vault_impl(
    password: &str,
    state: &AppEngineState,
) -> Result<VaultStatus, String> {
    let status = {
        let mut vault = state.vault.write();
        vault.initialize(password).map_err(|e| e.to_string())?;
        vault.get_status()
    };
    ensure_credential_migration(state)?;
    state.open_evm_stronghold();
    migrate_evm_keys_to_stronghold(state);
    state.sync_vault_credentials_to_adapters();
    Ok(status)
}

/// Exponential backoff for failed unlock attempts (online password-guess
/// throttle). Failure `n` waits `min(2^n, 30)` seconds before the next try.
fn unlock_backoff_secs(failures: u32) -> u64 {
    (1u64 << failures.min(5)).min(30)
}

pub fn unlock_vault_impl(password: &str, state: &AppEngineState) -> Result<VaultStatus, String> {
    {
        let backoff = state.vault_unlock_backoff.lock();
        let retry_in = backoff
            .1
            .saturating_duration_since(std::time::Instant::now());
        if backoff.0 > 0 && !retry_in.is_zero() {
            return Err(format!(
                "Too many failed unlock attempts — retry in {}s",
                retry_in.as_secs() + 1
            ));
        }
    }
    let status = {
        let mut vault = state.vault.write();
        match vault.unlock(password) {
            Ok(()) => {
                *state.vault_unlock_backoff.lock() = (0, std::time::Instant::now());
                vault.get_status()
            }
            Err(e) => {
                let mut backoff = state.vault_unlock_backoff.lock();
                backoff.0 = backoff.0.saturating_add(1);
                backoff.1 = std::time::Instant::now()
                    + std::time::Duration::from_secs(unlock_backoff_secs(
                        backoff.0.saturating_sub(1),
                    ));
                return Err(e.to_string());
            }
        }
    };
    // Post-unlock setup must never report failure for an unlock that
    // already succeeded — the vault IS open; a setup failure degrades
    // credential provisioning (adapters stay unauthenticated and fail
    // closed) rather than making the unlock look atomic-rolled-back.
    // Each step logs its own failure and the next still runs.
    if let Err(e) = ensure_credential_migration(state) {
        tracing::warn!("post-unlock credential migration failed: {e}");
    }
    state.open_evm_stronghold();
    migrate_evm_keys_to_stronghold(state);
    state.sync_vault_credentials_to_adapters();
    if crate::state::TradingEnvironment::from_mode(&state.settings.read().trading_mode)
        == crate::state::TradingEnvironment::Live
    {
        let ready_count = state
            .venues_for_env(crate::state::TradingEnvironment::Live)
            .into_iter()
            .filter(|(_, _, adapter)| {
                adapter.live_order_submission_implemented() && adapter.has_live_credentials()
            })
            .count();
        if ready_count >= 2 {
            *state.environment.write() = crate::state::TradingEnvironment::Live;
            // Drop books cached under the previous env — cross-environment
            // prices are never executable.
            state.market_data_hub.clear();
        }
    }
    Ok(status)
}

pub fn lock_vault_impl(state: &AppEngineState) -> Result<VaultStatus, String> {
    // In live mode a locked vault cannot authenticate venue calls — every
    // supervised exit (convergence, funding carry, residual flatten) and
    // every operator flatten would fail until re-unlock. Refuse the lock
    // while live positions are open rather than silently disabling exit
    // supervision; testnet sims need no credentials and lock freely.
    if state.is_live() {
        if let Ok(signals) = state
            .state_store
            .list_open_signals(Some(state.env().as_str()))
        {
            let open: Vec<String> = signals
                .iter()
                .filter(|s| s.status.counts_as_open())
                .map(|s| format!("{}:{}", s.symbol, s.status.as_str()))
                .collect();
            if !open.is_empty() {
                return Err(format!(
                    "Cannot lock the vault while {} live position(s) are open ({}) — a locked vault cannot flatten or supervise exits. Flatten them first.",
                    open.len(),
                    open.join(", ")
                ));
            }
        }
    }
    // Drop the stronghold before the vault forgets its session key —
    // adapter signers are wiped by the sync below either way.
    state.close_evm_stronghold();
    let status = {
        let mut vault = state.vault.write();
        vault.lock();
        vault.get_status()
    };
    state.sync_vault_credentials_to_adapters();
    Ok(status)
}

const DELETABLE_CREDENTIAL_VENUES: &[&str] = &[
    "remote_hsm",
    "hyperliquid",
    "lighter",
    "vanta",
    "robinhood",
    "arcus",
    "popdex",
    "kraken_spot",
    "kraken_futures",
    "polymarket",
    "kalshi",
];

/// Open signals that would lose venue authentication if `venue`'s
/// credentials were deleted — live mode only (testnet strands are
/// recoverable by re-adding the key; a live position cannot wait).
/// `Err` when exposure cannot be verified — a read failure is not "no
/// exposure", so callers must refuse the destructive operation.
fn open_signals_on_venue(venue: &str, state: &AppEngineState) -> Result<Vec<String>, String> {
    let signals = state
        .state_store
        .list_open_signals(Some(state.env().as_str()))
        .map_err(|e| format!("Cannot verify open positions on {venue}: {e}"))?;
    Ok(signals
        .iter()
        .filter(|s| {
            s.status.counts_as_open() && s.legs.iter().any(|l| l.venue.eq_ignore_ascii_case(venue))
        })
        .map(|s| format!("{}:{}", s.symbol, s.status.as_str()))
        .collect())
}

pub fn delete_exchange_credentials_impl(
    venue: &str,
    state: &AppEngineState,
) -> Result<bool, String> {
    let normalized = venue.trim().to_lowercase();
    if !DELETABLE_CREDENTIAL_VENUES.contains(&normalized.as_str()) {
        return Err(format!(
            "Credential deletion is not supported for venue '{}'",
            venue
        ));
    }
    if state.is_live() {
        let open = open_signals_on_venue(&normalized, state)?;
        if !open.is_empty() {
            return Err(format!(
                "Cannot delete {normalized} credentials while {} live position(s) use that venue ({}). Flatten them first.",
                open.len(),
                open.join(", ")
            ));
        }
    }
    let mut vault = state.vault.write();
    if !vault.is_unlocked() {
        return Err("Vault is locked. Please unlock before removing credentials.".into());
    }
    vault
        .delete_credentials(&normalized)
        .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Account profiles and strategy-level account routes
// ---------------------------------------------------------------------------

/// Sanitized account view for the settings UI: durable profile metadata plus
/// credential-presence flags. Never contains secret material.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountDto {
    pub id: String,
    pub venue: String,
    pub label: String,
    pub enabled: bool,
    pub is_default: bool,
    pub created_at: String,
    pub updated_at: String,
    /// `None` while the vault is locked (presence is unknowable); otherwise
    /// whether any credential material is stored for this account.
    pub credentials_configured: Option<bool>,
    pub has_api_key: bool,
    pub has_api_secret: bool,
    pub has_passphrase: bool,
    pub has_account_id: bool,
    pub has_wallet_address: bool,
    pub account_index: Option<u64>,
    pub subaccount_index: Option<u64>,
    pub api_key_index: Option<u64>,
    pub has_private_key: bool,
    /// Whether a venue base-URL override (regional domain/testnet) is stored.
    pub has_base_url: bool,
    /// "standard" | "plus" | "premium" — Lighter-family tier drives the
    /// venue's shared-IP rate budget and fee adjustments.
    pub account_tier: String,
    /// Per-account leverage ceiling 1..=5; `None` = unlimited (backend hard
    /// ceiling of 100 still applies; Risk Control global max also caps).
    pub max_leverage: Option<u32>,
    /// "live" | "testnet" — which venue environment this account's
    /// credentials bind to. Immutable once created.
    pub environment: String,
}

// Intentionally no Debug: may transiently hold credential material.
#[derive(Clone, Deserialize, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AccountCredentialInput {
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub api_secret: String,
    #[serde(default)]
    pub passphrase: Option<String>,
    #[serde(default)]
    pub account_id: Option<String>,
    #[serde(default)]
    pub wallet_address: Option<String>,
    #[serde(default)]
    pub account_index: Option<u64>,
    #[serde(default)]
    pub subaccount_index: Option<u64>,
    #[serde(default)]
    pub api_key_index: Option<u64>,
    #[serde(default)]
    pub private_key: Option<String>,
    /// Optional venue endpoint override — a REST entity domain
    /// (e.g. OKX app.okx.com / eea.okx.com keys) or a JSON-RPC URL
    /// (EVM AMM chains). URLs may embed secrets such as provider API
    /// keys; treat the value as secret material.
    #[serde(default)]
    pub base_url: Option<String>,
}

impl AccountCredentialInput {
    fn into_record(self, venue: &str, profile_id: &str, enabled: bool) -> ExchangeCredentials {
        let trimmed_opt = |v: &Option<String>| {
            v.as_ref()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        };
        ExchangeCredentials {
            venue: venue.to_string(),
            profile_id: Some(profile_id.to_string()),
            api_key: self.api_key.trim().to_string(),
            api_secret: self.api_secret.trim().to_string(),
            passphrase: trimmed_opt(&self.passphrase),
            account_id: trimmed_opt(&self.account_id),
            wallet_address: trimmed_opt(&self.wallet_address),
            account_index: self.account_index,
            subaccount_index: self.subaccount_index,
            api_key_index: self.api_key_index,
            private_key: trimmed_opt(&self.private_key),
            // Patch protocol: preserve presence (omitted stays `None`),
            // trim, but do NOT filter empty — `Some("")` is the explicit
            // clear sentinel the merge layer consumes.
            base_url: self.base_url.map(|s| s.trim().to_string()),
            enabled,
        }
    }
}

// Intentionally no Debug: `credentials` may hold secret material.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountUpsertInput {
    /// Existing account ID for updates; `None` creates a new account.
    #[serde(default)]
    pub id: Option<String>,
    /// Venue ID. Required on create; verified immutable on update.
    #[serde(default)]
    pub venue: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub make_default: bool,
    /// "standard" | "plus" | "premium". `None` preserves the stored tier on
    /// update and means standard on create.
    #[serde(default)]
    pub account_tier: Option<String>,
    /// Per-account leverage ceiling 1..=5. `None` means unlimited; updates
    /// are authoritative (sending `None` clears a previously stored cap).
    #[serde(default)]
    pub max_leverage: Option<u32>,
    /// "live" | "testnet" — create-time only; ignored on update (the stored
    /// environment is immutable). `None` on create means the currently
    /// active trading environment.
    #[serde(default)]
    pub environment: Option<String>,
    #[serde(default)]
    pub credentials: Option<AccountCredentialInput>,
}

// Intentionally no Debug.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StrategyRouteUpsertInput {
    /// Existing route ID for updates; `None` creates a new route.
    #[serde(default)]
    pub id: Option<String>,
    pub strategy_id: String,
    #[serde(default)]
    pub label: String,
    pub account_a_id: String,
    pub account_b_id: String,
    #[serde(default = "default_route_enabled")]
    pub enabled: bool,
}

fn default_route_enabled() -> bool {
    true
}

fn is_valid_account_id(raw: &str) -> bool {
    let v = raw.trim();
    !v.is_empty()
        && v.len() <= 128
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Fetch (creating when necessary) the `{venue}-default` account profile
/// used by the legacy flat settings credential fields. The legacy venue
/// toggle is mirrored onto the default account's enabled switch; label and
/// default status are preserved for existing profiles.
pub(crate) fn ensure_default_profile(
    state: &AppEngineState,
    venue: &str,
    enabled: bool,
) -> Result<AccountProfile, String> {
    let pid = format!("{venue}-default");
    match state
        .state_store
        .get_account(&pid)
        .map_err(|e| e.to_string())?
    {
        Some(existing) => {
            if existing.venue != venue {
                return Err(format!("account ID {pid} is bound to a different venue"));
            }
            if existing.enabled != enabled {
                state
                    .state_store
                    .upsert_account(
                        &pid,
                        venue,
                        &existing.label,
                        enabled,
                        existing.is_default,
                        &existing.account_tier,
                        existing.max_leverage,
                        Some(&existing.environment),
                    )
                    .map_err(|e| e.to_string())
            } else {
                Ok(existing)
            }
        }
        None => state
            .state_store
            .upsert_account(
                &pid,
                venue,
                "Default",
                enabled,
                true,
                "standard",
                None,
                Some("live"),
            )
            .map_err(|e| e.to_string()),
    }
}

/// Migrate legacy venue-keyed credential records onto `{venue}-default`
/// account profiles and prune bound records whose profile no longer exists.
/// No-op while the vault is locked. `remote_hsm` is not an exchange account
/// and is intentionally left unbound.
pub(crate) fn ensure_credential_migration(state: &AppEngineState) -> Result<(), String> {
    let mut vault = state.vault.write();
    if !vault.is_unlocked() {
        return Ok(());
    }

    // Prune bound records whose profile was deleted (e.g. credential cleanup
    // failed after the account row was removed).
    let presence = vault.credential_presence().map_err(|e| e.to_string())?;
    for p in presence.iter() {
        if let Some(pid) = &p.profile_id {
            let exists = state
                .state_store
                .get_account(pid)
                .map_err(|e| e.to_string())?
                .is_some();
            if !exists {
                vault
                    .delete_profile_credentials(pid)
                    .map_err(|e| e.to_string())?;
            }
        }
    }

    for cred in vault.unbound_credentials().map_err(|e| e.to_string())? {
        if cred.venue == "remote_hsm" {
            continue;
        }
        let pid = format!("{}-default", cred.venue);
        if !is_valid_account_id(&pid) {
            tracing::warn!(
                "skipping migration of credential record with invalid venue '{}'",
                cred.venue
            );
            continue;
        }
        match state
            .state_store
            .get_account(&pid)
            .map_err(|e| e.to_string())?
        {
            Some(existing) => {
                if existing.venue != cred.venue {
                    return Err(format!(
                        "cannot migrate {} credentials: {pid} is bound to another venue",
                        cred.venue
                    ));
                }
            }
            None => {
                state
                    .state_store
                    .upsert_account(
                        &pid,
                        &cred.venue,
                        "Default",
                        cred.enabled,
                        true,
                        "standard",
                        None,
                        // Legacy unbound records predate the env split — they
                        // were the live credential set.
                        Some("live"),
                    )
                    .map_err(|e| e.to_string())?;
            }
        }
        vault
            .bind_profile(&cred.venue, &pid)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn account_dto(
    profile: &AccountProfile,
    presence: Option<&CredentialPresence>,
    vault_locked: bool,
) -> AccountDto {
    let configured = presence.is_some_and(|p| {
        p.has_api_key
            || p.has_api_secret
            || p.has_passphrase
            || p.has_account_id
            || p.has_wallet_address
            || p.account_index.is_some()
            || p.subaccount_index.is_some()
            || p.api_key_index.is_some()
            || p.has_private_key
    });
    AccountDto {
        id: profile.id.clone(),
        venue: profile.venue.clone(),
        label: profile.label.clone(),
        enabled: profile.enabled,
        is_default: profile.is_default,
        created_at: profile.created_at.clone(),
        updated_at: profile.updated_at.clone(),
        credentials_configured: if vault_locked { None } else { Some(configured) },
        has_api_key: presence.is_some_and(|p| p.has_api_key),
        has_api_secret: presence.is_some_and(|p| p.has_api_secret),
        has_passphrase: presence.is_some_and(|p| p.has_passphrase),
        has_account_id: presence.is_some_and(|p| p.has_account_id),
        has_wallet_address: presence.is_some_and(|p| p.has_wallet_address),
        has_private_key: presence.is_some_and(|p| p.has_private_key),
        has_base_url: presence.is_some_and(|p| p.has_base_url),
        account_index: presence.and_then(|p| p.account_index),
        subaccount_index: presence.and_then(|p| p.subaccount_index),
        api_key_index: presence.and_then(|p| p.api_key_index),
        account_tier: profile.account_tier.clone(),
        max_leverage: profile.max_leverage,
        environment: profile.environment.clone(),
    }
}

/// Merge durable profiles with sanitized vault credential presence.
fn accounts_with_presence(
    state: &AppEngineState,
    profiles: Vec<AccountProfile>,
) -> Result<Vec<AccountDto>, String> {
    let (presence, locked) = {
        let mut vault = state.vault.write();
        let locked = !vault.is_unlocked();
        let presence = if locked {
            Vec::new()
        } else {
            vault.credential_presence().map_err(|e| e.to_string())?
        };
        (presence, locked)
    };
    let by_profile: std::collections::HashMap<&str, &CredentialPresence> = presence
        .iter()
        .filter_map(|p| p.profile_id.as_deref().map(|id| (id, p)))
        .collect();
    Ok(profiles
        .iter()
        .map(|p| account_dto(p, by_profile.get(p.id.as_str()).copied(), locked))
        .collect())
}

pub fn list_accounts_impl(
    venue: Option<&str>,
    state: &AppEngineState,
) -> Result<Vec<AccountDto>, String> {
    ensure_credential_migration(state)?;
    let env = state.env().as_str().to_string();
    let profiles = state
        .state_store
        .list_accounts(venue)
        .map_err(|e| e.to_string())?
        .into_iter()
        // Accounts are environment-scoped — the page manages the credential
        // set for the active environment only. Switching Trading Mode swaps
        // which profiles appear here.
        .filter(|p| p.environment == env)
        .collect();
    accounts_with_presence(state, profiles)
}

pub fn upsert_account_impl(
    input: AccountUpsertInput,
    state: &AppEngineState,
) -> Result<AccountDto, String> {
    ensure_credential_migration(state)?;

    let profile = match &input.id {
        Some(id) => {
            let existing = state
                .state_store
                .get_account(id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("account {id} does not exist"))?;
            if !input.venue.trim().is_empty()
                && !input.venue.trim().eq_ignore_ascii_case(&existing.venue)
            {
                return Err("account venue is immutable once created".into());
            }
            // Disabling an account with an open live leg strands exit
            // supervision — its adapter drops out of `get_venues`, so
            // closes can no longer authenticate or even resolve.
            if !input.enabled && state.is_live() {
                // Fail closed: a read error is not "no exposure" — refuse
                // the disable when open positions cannot be verified.
                let signals = state
                    .state_store
                    .list_open_signals(Some(state.env().as_str()))
                    .map_err(|e| format!("Cannot verify open positions for account {id}: {e}"))?;
                let open: Vec<String> = signals
                    .iter()
                    .filter(|s| {
                        s.status.counts_as_open()
                            && s.legs
                                .iter()
                                .any(|l| l.account_id.as_deref() == Some(id.as_str()))
                    })
                    .map(|s| format!("{}:{}", s.symbol, s.status.as_str()))
                    .collect();
                if !open.is_empty() {
                    return Err(format!(
                        "Cannot disable account {id} while {} live position(s) route through it ({}). Flatten them first.",
                        open.len(),
                        open.join(", ")
                    ));
                }
            }
            state
                .state_store
                .upsert_account(
                    id,
                    &existing.venue,
                    &input.label,
                    input.enabled,
                    input.make_default,
                    input
                        .account_tier
                        .as_deref()
                        .unwrap_or(&existing.account_tier),
                    input.max_leverage,
                    Some(&existing.environment),
                )
                .map_err(|e| e.to_string())?
        }
        None => {
            let venue_lc = input.venue.trim().to_lowercase();
            if venue_lc.is_empty() {
                return Err("venue is required when creating an account".into());
            }
            let mut candidate;
            loop {
                candidate = format!("{}-{:08x}", venue_lc, rand::random::<u32>());
                if state
                    .state_store
                    .get_account(&candidate)
                    .map_err(|e| e.to_string())?
                    .is_none()
                {
                    break;
                }
            }
            // An explicit environment wins; otherwise the account lands in
            // the environment the operator is currently looking at.
            let env = match input.environment.as_deref() {
                Some(e) if !e.trim().is_empty() => e.trim().to_lowercase(),
                _ => state.env().as_str().to_string(),
            };
            state
                .state_store
                .upsert_account(
                    &candidate,
                    &venue_lc,
                    &input.label,
                    input.enabled,
                    input.make_default,
                    input.account_tier.as_deref().unwrap_or("standard"),
                    input.max_leverage,
                    Some(&env),
                )
                .map_err(|e| e.to_string())?
        }
    };

    // `credentials: None` is the no-update signal; any `Some` payload is
    // processed even when every value is empty — per-field merge semantics
    // (empty string clears base_url, preserves secrets) decide the effect.
    let mut cred_error: Option<String> = None;
    if let Some(creds) = &input.credentials {
        let mut vault = state.vault.write();
        if !vault.is_unlocked() {
            cred_error = Some("vault is locked".to_string());
        } else {
            let record = creds
                .clone()
                .into_record(&profile.venue, &profile.id, profile.enabled);
            if let Err(e) = vault.upsert_profile_credentials(&profile.id, record) {
                cred_error = Some(e.to_string());
            }
        }
    }
    // Mirror the account switch onto the stored record's metadata flag when
    // the vault is reachable; the profile remains the authoritative switch.
    {
        let mut vault = state.vault.write();
        if vault.is_unlocked() {
            let _ = vault.update_profile_enabled(&[(profile.id.clone(), profile.enabled)]);
        }
    }
    // If this write landed a raw hex key on an AMM venue profile, move it
    // into the stronghold before the sync provisions adapters — no-op when
    // no stronghold is loaded (the record keeps working as a local key).
    migrate_evm_keys_to_stronghold(state);
    state.sync_vault_credentials_to_adapters();

    let dto = accounts_with_presence(state, vec![profile])?
        .into_iter()
        .next()
        .ok_or_else(|| "account disappeared after save".to_string())?;
    if let Some(err) = cred_error {
        return Err(format!(
            "account saved but credentials were not stored: {err}"
        ));
    }
    Ok(dto)
}

/* ---------------------------------------------------------------------- */
/* Shared EVM wallet — one signer fans out to every EVM AMM venue          */
/* ---------------------------------------------------------------------- */

/// Summary returned by `save_evm_wallet`: the derived signer plus which
/// venue accounts received the key vs. only the public address.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvmWalletSaveDto {
    /// EIP-55 checksum address derived from the submitted private key.
    pub signer_address: String,
    /// Swap-capable venues whose accounts now hold the private key.
    pub keyed_venues: Vec<String>,
    /// Market-data venues whose accounts hold only the wallet address.
    pub address_only_venues: Vec<String>,
    /// `{venue}-default` account ids created by this save. New rows are
    /// disabled — a wallet save never admits a venue by itself.
    pub created_accounts: Vec<String>,
    /// Where the key bytes ended up: `"stronghold"` (vault record carries
    /// only the sentinel; signing happens inside guarded memory) or
    /// `"vault"` (fallback — encrypted credential record, legacy behavior).
    pub key_storage: String,
}

/// Decode a validated hex private key into bytes for the stronghold
/// write. The caller (`save_evm_wallet_impl`) has already run full
/// secp256k1 validation via `derive_evm_signer_address_impl` — this only
/// re-encodes the bytes; it is not the validation path.
fn evm_key_bytes(private_key: &str) -> Result<zeroize::Zeroizing<[u8; 32]>, String> {
    let raw = hex::decode(private_key.trim().trim_start_matches("0x"))
        .map_err(|e| format!("invalid EVM private key: {e}"))?;
    let bytes: [u8; 32] = raw
        .try_into()
        .map_err(|_| "invalid EVM private key: expected 32 bytes".to_string())?;
    Ok(zeroize::Zeroizing::new(bytes))
}

/// The account a shared-wallet write targets for one AMM venue: the
/// canonical `{venue}-default` profile when it exists, otherwise the
/// venue's default (or first) live account, otherwise a freshly created
/// disabled `{venue}-default`. EVM AMM venues have no public testnet, so
/// their accounts always bind the live environment regardless of which
/// mode the engine is currently running.
fn evm_wallet_target_profile(
    state: &AppEngineState,
    venue: &str,
) -> Result<(AccountProfile, bool), String> {
    let pid = format!("{venue}-default");
    if let Some(p) = state
        .state_store
        .get_account(&pid)
        .map_err(|e| e.to_string())?
    {
        return if p.environment == "live" {
            Ok((p, false))
        } else {
            Err(format!(
                "account {pid} is bound to testnet — delete it before saving an EVM wallet"
            ))
        };
    }
    let live_accounts: Vec<AccountProfile> = state
        .state_store
        .list_accounts(Some(venue))
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|a| a.environment == "live")
        .collect();
    if let Some(p) = live_accounts.first() {
        // list_accounts orders is_default first — the venue's credential-
        // resolving account adopts the shared key without an id change.
        return Ok((p.clone(), false));
    }
    let created = state
        .state_store
        .upsert_account(
            &pid,
            venue,
            "EVM Wallet",
            false, // disabled — admitting a venue stays a per-venue toggle
            false,
            "standard",
            None,
            Some("live"),
        )
        .map_err(|e| e.to_string())?;
    Ok((created, true))
}

/// Save the shared EVM wallet: validates the key, derives its signer, then
/// writes the credential record for every admitted AMM venue. Swap-capable
/// venues (`LIVE_EVM_AMM_VENUE_IDS`) receive the private key; market-data
/// venues receive only the public address — the key is never stored on a
/// venue that cannot trade. A provided `wallet_address` is a cross-check
/// only and must equal the derived signer (the same refusal the adapter
/// applies). Enabled state, labels, tiers and RPC overrides are preserved;
/// venues that gain an account row start disabled.
///
/// When a venue already resolves credentials from a different enabled
/// default account, that account receives the same material too — a wallet
/// save means every enabled AMM venue signs as this wallet immediately,
/// not after a manual default-account shuffle.
pub fn save_evm_wallet_impl(
    private_key: &str,
    wallet_address: Option<&str>,
    state: &AppEngineState,
) -> Result<EvmWalletSaveDto, String> {
    let signer = derive_evm_signer_address_impl(private_key)?;
    if let Some(w) = wallet_address.map(|w| w.trim()).filter(|w| !w.is_empty()) {
        if !w.eq_ignore_ascii_case(&signer) {
            return Err(format!(
                "wallet address {w} does not match the private key's signer {signer} — refusing credentials"
            ));
        }
    }
    let mut vault = state.vault.write();
    if !vault.is_unlocked() {
        return Err("Vault is locked. Please unlock before saving credentials.".into());
    }

    // Preferred custody: write the key into the Stronghold snapshot and
    // store only the sentinel in vault records — the raw bytes never
    // persist. Falls back to vault storage (legacy behavior) when no
    // stronghold is loaded or the write fails; `key_storage` reports
    // which path ran so the UI can show honest custody status.
    let key_storage = match state.evm_stronghold.read().as_ref() {
        Some(sh) => match sh.store_key(evm_key_bytes(private_key)?) {
            Ok(()) => "stronghold",
            Err(e) => {
                tracing::warn!("stronghold key store failed ({e}) — falling back to vault record");
                "vault"
            }
        },
        None => "vault",
    };
    let stored_key = if key_storage == "stronghold" {
        crate::stronghold::EVM_STRONGHOLD_KEY_REF.to_string()
    } else {
        private_key.trim().to_string()
    };

    let mut keyed = Vec::new();
    let mut address_only = Vec::new();
    let mut created = Vec::new();
    for vid in ADMITTED_EVM_AMM_VENUE_IDS {
        let (profile, was_created) = evm_wallet_target_profile(state, vid)?;
        if was_created {
            created.push(profile.id.clone());
        }
        // Both the canonical target and the currently credential-resolving
        // enabled default get the material when they differ.
        let mut targets = vec![profile];
        if let Ok(Some(default)) = state.state_store.default_account(vid, "live") {
            if !targets.iter().any(|p| p.id == default.id) {
                targets.push(default);
            }
        }
        let has_key = LIVE_EVM_AMM_VENUE_IDS.contains(vid);
        for p in &targets {
            if !has_key {
                // Merge semantics cannot clear a secret — strip wallet
                // fields first so a stale key can never linger on a
                // market-data venue, then write the address-only record.
                let _ = vault.clear_profile_wallet_fields(&p.id);
            }
            let record = ExchangeCredentials {
                venue: vid.to_string(),
                profile_id: Some(p.id.clone()),
                api_key: String::new(),
                api_secret: String::new(),
                passphrase: None,
                account_id: None,
                wallet_address: Some(signer.clone()),
                account_index: None,
                subaccount_index: None,
                api_key_index: None,
                private_key: has_key.then(|| stored_key.clone()),
                base_url: None, // preserve any stored RPC override
                enabled: p.enabled,
            };
            vault
                .upsert_profile_credentials(&p.id, record)
                .map_err(|e| e.to_string())?;
        }
        if has_key {
            keyed.push(vid.to_string());
        } else {
            address_only.push(vid.to_string());
        }
    }
    drop(vault);
    state.sync_vault_credentials_to_adapters();
    Ok(EvmWalletSaveDto {
        signer_address: signer,
        keyed_venues: keyed,
        address_only_venues: address_only,
        created_accounts: created,
        key_storage: key_storage.to_string(),
    })
}

/// Remove the shared wallet material (private key + wallet address) from
/// every AMM venue's bound credential records — all accounts, not just the
/// canonical default, in case the wallet was ever written to a different
/// profile. Account rows, enabled flags and RPC endpoint overrides are
/// preserved; venues keep market data but can no longer trade.
pub fn clear_evm_wallet_impl(state: &AppEngineState) -> Result<u32, String> {
    let mut vault = state.vault.write();
    if !vault.is_unlocked() {
        return Err("Vault is locked. Please unlock before clearing credentials.".into());
    }
    let mut cleared = 0u32;
    for vid in ADMITTED_EVM_AMM_VENUE_IDS {
        let accounts = state
            .state_store
            .list_accounts(Some(vid))
            .map_err(|e| e.to_string())?;
        for p in accounts.iter().filter(|a| a.environment == "live") {
            if vault
                .clear_profile_wallet_fields(&p.id)
                .map_err(|e| e.to_string())?
            {
                cleared += 1;
            }
        }
    }
    drop(vault);
    // Revoke the key inside the stronghold so the record is unreadable
    // even to the in-memory vault — a revoked key cannot sign again until
    // a new one is stored. The sync below clears adapter signers
    // regardless, so a revoke failure still leaves adapters signerless.
    if let Some(sh) = state.evm_stronghold.read().as_ref() {
        if let Err(e) = sh.revoke_key() {
            tracing::warn!("stronghold key revoke failed: {e}");
        }
    }
    state.sync_vault_credentials_to_adapters();
    Ok(cleared)
}

/// Move vault-held EVM AMM private keys into the loaded stronghold — the
/// upgrade path for records written before key custody moved out of the
/// encrypted file. Only runs when every raw AMM key derives to the SAME
/// signer: the shared wallet is a single key and divergent keys can't
/// share one stronghold record, so they stay vault-held (still working
/// via the local-signer path). On success each record's `private_key` is
/// rewritten to the sentinel so the key bytes stop persisting in the
/// vault file. Best-effort: any parse/store failure leaves the record
/// untouched.
fn migrate_evm_keys_to_stronghold(state: &AppEngineState) {
    if state.evm_stronghold.read().is_none() {
        return;
    }
    let mut vault = state.vault.write();
    if !vault.is_unlocked() {
        return;
    }
    let evm_records: Vec<ExchangeCredentials> = vault
        .get_data()
        .map(|d| {
            d.credentials
                .iter()
                .filter(|c| ADMITTED_EVM_AMM_VENUE_IDS.contains(&c.venue.to_lowercase().as_str()))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    // Distinct raw keys across all AMM records — sentinel and empty
    // records are already fine.
    let mut keys = std::collections::BTreeSet::new();
    for c in &evm_records {
        let k = c.private_key.as_deref().unwrap_or("").trim();
        if !k.is_empty() && !k.starts_with(EVM_EXTERNAL_SIGNER_PREFIX) {
            keys.insert(k.to_string());
        }
    }
    if keys.len() != 1 {
        return; // nothing to migrate, or divergent keys — keep vault custody
    }
    let key_hex = keys.iter().next().expect("one key");
    if derive_evm_signer_address_impl(key_hex).is_err() {
        return; // malformed — let update_credentials refuse it as before
    }
    let Ok(key_bytes) = evm_key_bytes(key_hex) else {
        return;
    };
    if let Some(sh) = state.evm_stronghold.read().as_ref() {
        if let Err(e) = sh.store_key(key_bytes) {
            tracing::warn!("EVM key migration to stronghold failed: {e}");
            return;
        }
    } else {
        return;
    }
    for c in evm_records {
        let k = c.private_key.as_deref().unwrap_or("").trim();
        if k.is_empty() || k.starts_with(EVM_EXTERNAL_SIGNER_PREFIX) {
            continue;
        }
        let mut record = c.clone();
        record.private_key = Some(crate::stronghold::EVM_STRONGHOLD_KEY_REF.to_string());
        let res = match record.profile_id.clone() {
            Some(pid) => vault.upsert_profile_credentials(&pid, record),
            None => vault.upsert_credentials(vec![record]),
        };
        if let Err(e) = res {
            tracing::warn!("EVM key migration failed for {}: {e}", c.venue);
        }
    }
}

#[tauri::command]
pub async fn save_evm_wallet(
    private_key: String,
    wallet_address: Option<String>,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<EvmWalletSaveDto, String> {
    save_evm_wallet_impl(&private_key, wallet_address.as_deref(), &state)
}

#[tauri::command]
pub async fn clear_evm_wallet(state: State<'_, Arc<AppEngineState>>) -> Result<u32, String> {
    clear_evm_wallet_impl(&state)
}

/// Where the shared EVM key currently lives, for the wallet card's custody
/// chip: `"stronghold"` when a record carries the sentinel (key bytes are
/// inside the stronghold snapshot), `"vault"` when a record still holds a
/// raw key (legacy / stronghold-unavailable fallback), `"none"` when no
/// AMM record holds any key material. `strongholdLoaded` distinguishes
/// "sentinel + live signer" from "sentinel but no snapshot loaded" (which
/// cannot sign — shown as a warning state in the UI).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvmKeyCustodyDto {
    pub custody: String,
    pub stronghold_loaded: bool,
}

pub fn get_evm_key_custody_impl(state: &AppEngineState) -> Result<EvmKeyCustodyDto, String> {
    let mut vault = state.vault.write();
    if !vault.is_unlocked() {
        return Err("Vault is locked.".into());
    }
    let mut custody = "none";
    let data = vault.get_data().map_err(|e| e.to_string())?;
    for c in &data.credentials {
        if !ADMITTED_EVM_AMM_VENUE_IDS.contains(&c.venue.to_lowercase().as_str()) {
            continue;
        }
        let k = c.private_key.as_deref().unwrap_or("").trim();
        if k.is_empty() {
            continue;
        }
        if k.starts_with(EVM_EXTERNAL_SIGNER_PREFIX) {
            custody = "stronghold";
        } else if custody != "stronghold" {
            custody = "vault";
        }
    }
    // "loaded" means a snapshot is open AND holds the key record — an
    // open-but-empty stronghold cannot sign, same as absent.
    let stronghold_loaded = state
        .evm_stronghold
        .read()
        .as_ref()
        .map(|s| s.has_key())
        .unwrap_or(false);
    Ok(EvmKeyCustodyDto {
        custody: custody.to_string(),
        stronghold_loaded,
    })
}

#[tauri::command]
pub async fn get_evm_key_custody(
    state: State<'_, Arc<AppEngineState>>,
) -> Result<EvmKeyCustodyDto, String> {
    get_evm_key_custody_impl(&state)
}

/// Delete an account profile and its bound credential record. Route
/// references are checked before anything is removed so a foreign-key block
/// can never destroy secrets; the vault must be unlocked so bound secrets
/// are never orphaned.
pub fn delete_account_impl(account_id: &str, state: &AppEngineState) -> Result<bool, String> {
    let refs = state
        .state_store
        .routes_referencing(account_id)
        .map_err(|e| e.to_string())?;
    if !refs.is_empty() {
        let route_list = refs
            .iter()
            .map(|r| format!("{}:{}", r.strategy_id, r.label))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "account {account_id} is referenced by {} strategy route(s) ({route_list}); remove or reassign them first",
            refs.len()
        ));
    }
    // Accounts that own recorded position legs keep their attribution; delete
    // is refused so signal history never loses its account binding.
    if state
        .state_store
        .account_has_signal_legs(account_id)
        .map_err(|e| e.to_string())?
    {
        return Err(format!(
            "account {account_id} has recorded signal position history; disable the account instead of deleting it"
        ));
    }
    {
        let mut vault = state.vault.write();
        if !vault.is_unlocked() {
            return Err("Vault is locked. Please unlock before deleting an account.".into());
        }
    }
    let deleted = state
        .state_store
        .delete_account(account_id)
        .map_err(|e| e.to_string())?;
    if deleted {
        let mut vault = state.vault.write();
        if let Err(e) = vault.delete_profile_credentials(account_id) {
            // Orphaned bound records are pruned by ensure_credential_migration
            // on the next unlock; surface in logs only.
            tracing::warn!("account {account_id} deleted but credential cleanup failed: {e}");
        }
    }
    state.sync_vault_credentials_to_adapters();
    Ok(deleted)
}

/// Removes only the credential records bound to one account profile; the
/// account itself is kept. Distinct from `delete_account_impl`, which removes
/// the whole profile, and from `delete_exchange_credentials_impl`, which is
/// venue-wide.
pub fn delete_account_credentials_impl(
    account_id: &str,
    state: &AppEngineState,
) -> Result<bool, String> {
    let exists = state
        .state_store
        .get_account(account_id)
        .map_err(|e| e.to_string())?
        .is_some();
    if !exists {
        return Err(format!("Unknown account '{account_id}'"));
    }
    // Removing this account's credentials while one of its legs is still
    // open strands live exit supervision — the account's orders can no
    // longer authenticate. Same gate as the venue-wide delete above.
    if state.is_live() {
        // Fail closed on read errors — "cannot verify" is not "no exposure".
        let signals = state
            .state_store
            .list_open_signals(Some(state.env().as_str()))
            .map_err(|e| format!("Cannot verify open positions for account {account_id}: {e}"))?;
        let open: Vec<String> = signals
            .iter()
            .filter(|s| {
                s.status.counts_as_open()
                    && s.legs
                        .iter()
                        .any(|l| l.account_id.as_deref() == Some(account_id))
            })
            .map(|s| format!("{}:{}", s.symbol, s.status.as_str()))
            .collect();
        if !open.is_empty() {
            return Err(format!(
                "Cannot remove account {account_id} credentials while {} live position(s) route through it ({}). Flatten them first.",
                open.len(),
                open.join(", ")
            ));
        }
    }
    let res = {
        let mut vault = state.vault.write();
        if !vault.is_unlocked() {
            return Err("Vault is locked. Please unlock before removing credentials.".into());
        }
        vault
            .delete_profile_credentials(account_id)
            .map_err(|e| e.to_string())
    }?;
    state.sync_vault_credentials_to_adapters();
    Ok(res)
}

/// Field-scoped credential reveal (security hardening, 2026-09-19).
/// Returns ONE whitelisted secret field so the UI only pulls a secret across
/// IPC when the user explicitly reveals that field. Unknown fields fail
/// closed; vault must be unlocked.
pub fn get_account_credential_field_impl(
    account_id: &str,
    field: &str,
    state: &AppEngineState,
) -> Result<String, String> {
    let exists = state
        .state_store
        .get_account(account_id)
        .map_err(|e| e.to_string())?
        .is_some();
    if !exists {
        return Err(format!("Unknown account '{account_id}'"));
    }
    let mut vault = state.vault.write();
    if !vault.is_unlocked() {
        return Err("Vault is locked. Please unlock before viewing credentials.".into());
    }
    let data = vault.get_data().map_err(|e| e.to_string())?;
    let cred = data
        .credentials
        .iter()
        .find(|c| c.profile_id.as_deref() == Some(account_id));
    match cred {
        Some(c) => Ok(match field {
            "apiKey" => c.api_key.clone(),
            "apiSecret" => c.api_secret.clone(),
            "passphrase" => c.passphrase.clone().unwrap_or_default(),
            "accountId" => c.account_id.clone().unwrap_or_default(),
            "walletAddress" => c.wallet_address.clone().unwrap_or_default(),
            "privateKey" => {
                let k = c.private_key.clone().unwrap_or_default();
                if k.trim()
                    .starts_with(arbitrageur_exchanges::evm_amm::EVM_EXTERNAL_SIGNER_PREFIX)
                {
                    // The sentinel is not a key — revealing it would hand
                    // the UI a fake credential. External-custody keys are
                    // intentionally non-exportable.
                    return Err("Key is held in Stronghold custody and cannot be revealed".into());
                }
                k
            }
            "baseUrl" => c.base_url.clone().unwrap_or_default(),
            _ => return Err(format!("Unknown credential field '{field}'")),
        }),
        None => Err(format!(
            "No credentials configured for account '{account_id}'"
        )),
    }
}

pub fn set_default_account_impl(
    venue: &str,
    account_id: &str,
    state: &AppEngineState,
) -> Result<(), String> {
    state
        .state_store
        .set_default_account(venue, account_id)
        .map_err(|e| e.to_string())
}

pub fn list_strategy_routes_impl(
    strategy_id: Option<&str>,
    state: &AppEngineState,
) -> Result<Vec<StrategyAccountRoute>, String> {
    state
        .state_store
        .list_strategy_routes(strategy_id)
        .map_err(|e| e.to_string())
}

pub fn upsert_strategy_route_impl(
    input: StrategyRouteUpsertInput,
    state: &AppEngineState,
) -> Result<StrategyAccountRoute, String> {
    let id = match &input.id {
        Some(id) => id.clone(),
        None => {
            let sid = input.strategy_id.trim().to_lowercase();
            let existing: std::collections::HashSet<String> = state
                .state_store
                .list_strategy_routes(None)
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(|r| r.id)
                .collect();
            let mut candidate;
            loop {
                candidate = format!("{}-{:08x}", sid, rand::random::<u32>());
                if !existing.contains(&candidate) {
                    break;
                }
            }
            candidate
        }
    };
    // Labels are required by the store; an omitted label preserves the
    // existing route's label on update, or derives a readable default.
    let label = if input.label.trim().is_empty() {
        let existing_label = state
            .state_store
            .list_strategy_routes(None)
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|r| r.id == id)
            .map(|r| r.label);
        match existing_label {
            Some(l) => l,
            None => {
                let name = |aid: &str| -> String {
                    state
                        .state_store
                        .get_account(aid)
                        .ok()
                        .flatten()
                        .map(|a| format!("{}:{}", a.venue, a.label))
                        .unwrap_or_else(|| aid.to_string())
                };
                format!(
                    "{} \u{2194} {}",
                    name(&input.account_a_id),
                    name(&input.account_b_id)
                )
            }
        }
    } else {
        input.label.clone()
    };
    state
        .state_store
        .upsert_strategy_route(
            &id,
            &input.strategy_id,
            &label,
            &input.account_a_id,
            &input.account_b_id,
            input.enabled,
        )
        .map_err(|e| e.to_string())
}

pub fn delete_strategy_route_impl(route_id: &str, state: &AppEngineState) -> Result<bool, String> {
    state
        .state_store
        .delete_strategy_route(route_id)
        .map_err(|e| e.to_string())
}

/// Eligible account routes for a strategy across a venue pair: enabled
/// routes whose two endpoint accounts are enabled and sit on the two
/// venues (in either order).
pub fn eligible_strategy_routes_impl(
    strategy_id: &str,
    venue_a: &str,
    venue_b: &str,
    state: &AppEngineState,
) -> Result<Vec<StrategyAccountRoute>, String> {
    state
        .state_store
        .eligible_strategy_routes(strategy_id, venue_a, venue_b, state.env().as_str())
        .map_err(|e| e.to_string())
}

pub async fn get_venue_balances_impl(
    state: &AppEngineState,
) -> Result<Vec<VenueBalanceInfo>, String> {
    let is_live = state.is_live();
    let env_str = state.env().as_str();
    let env_label = if is_live { "live" } else { "testnet" };
    let monitor = margin_monitor_from_settings(state);
    let mut results = Vec::new();

    // One code path for both environments: `get_venues()` returns the
    // active environment's adapters (production or venue demo endpoints).
    if state.vault.write().is_unlocked() {
        state.sync_vault_credentials_to_adapters();
    }

    for (vid, name, adapter) in state.get_venues() {
        let def_acc_id = state
            .state_store
            .default_account(vid, env_str)
            .ok()
            .flatten()
            .map(|a| a.id);
        if !adapter.has_live_credentials() {
            // Truthful status: credentials stored but vault locked is not
            // the same as nothing configured (an enabled venue with a
            // saved account is configured — it just can't be read now).
            let locked = !state.vault.write().is_unlocked();
            // `configured_venues` needs the decrypted vault, so a locked
            // vault always reports false there — fall back to the durable
            // account table, which knows a credential profile exists even
            // when its secrets are unreadable.
            let configured = state
                .vault
                .write()
                .configured_venues()
                .unwrap_or_default()
                .iter()
                .any(|v| v.eq_ignore_ascii_case(vid))
                || def_acc_id.is_some();
            let (status, action) = if locked && configured {
                (
                    "LOCKED".to_string(),
                    Some(format!(
                        "Vault is locked — unlock to read the {env_label} {name} balance."
                    )),
                )
            } else {
                (
                        "NO_CREDENTIALS".to_string(),
                        Some(format!(
                            "No usable {env_label} API credentials for {name} in the secure vault — add them on the account card."
                        )),
                    )
            };
            results.push(VenueBalanceInfo {
                venue: vid.to_string(),
                equity_usd: 0.0,
                available_margin_usd: 0.0,
                margin_rate_pct: 0.0,
                status,
                recommended_action: action,
                is_live,
                account_id: def_acc_id,
            });
            continue;
        }

        match live_account_or_cached(state, vid, &adapter).await {
            Ok((account, stale_age)) => {
                // USD-quote policy: convert venue-settlement figures to the
                // USD basis so the balance card, equity attribution, and
                // margin evaluation all read USD — a depegged settlement
                // coin shows as a real equity drop, not hidden basis.
                let mut account = account;
                state.normalize_account_usd(vid, &mut account);
                let free_margin_usd = account
                    .free_margin
                    .to_string()
                    .parse::<f64>()
                    .unwrap_or(0.0);
                let equity_usd = account.equity.to_string().parse::<f64>().unwrap_or(0.0);
                let margin_rate_pct = account
                    .margin_usage_pct
                    .to_string()
                    .parse::<f64>()
                    .unwrap_or(0.0);

                if equity_usd.is_finite() {
                    let now_ms = Utc::now().timestamp_millis();
                    // Equity baselines are namespaced `{env}-{venue}` so
                    // a testnet baseline is never compared against live
                    // account equity (it would log a fake
                    // deposit/withdrawal event).
                    match state.state_store.check_account_equity(
                        &format!("{env_str}-{vid}"),
                        equity_usd,
                        &position_signature(&account.positions),
                        now_ms,
                    ) {
                        Ok(report) => {
                            if let Some(event) = report.event {
                                tracing::warn!(
                                        "external equity delta on {vid}: {} expected {expected} actual {actual} ({kind}) — baseline rebased",
                                        event.delta,
                                        expected = event.expected,
                                        actual = event.actual,
                                        kind = event.kind,
                                    );
                            }
                        }
                        Err(e) => {
                            tracing::warn!("equity attribution poll failed for {vid}: {e}")
                        }
                    }
                }

                let (status, action) = match monitor.evaluate_account(&account) {
                        MarginHealthStatus::Healthy => ("HEALTHY".to_string(), None),
                        MarginHealthStatus::Warning => (
                            "LOW_MARGIN".to_string(),
                            Some(format!("Warning: {} margin utilization elevated.", name)),
                        ),
                        MarginHealthStatus::Empty => (
                            "EMPTY".to_string(),
                            Some(format!(
                                "No funds on {} — deposit to enable trading on this venue.",
                                name
                            )),
                        ),
                        MarginHealthStatus::Critical | MarginHealthStatus::Breached => (
                            "DEPLETED".to_string(),
                            Some(format!(
                                "Critical: {} margin utilization breached. Execution halted on this leg.",
                                name
                            )),
                        ),
                        // Account-level eval cannot produce this today, but the
                        // shared enum must never silently map Unknown → safe.
                        MarginHealthStatus::Unknown => (
                            "UNVERIFIED".to_string(),
                            Some(format!(
                                "{} margin data incomplete — safety cannot be verified.",
                                name
                            )),
                        ),
                    };

                // Stale override: figures above are last-known-good. Inside the
                // grace period this stays quiet (STALE); past it, it
                // escalates to ERROR so the operator is warned — but the
                // cached balance is still reported, never zeroed.
                let (status, action) = match stale_age {
                        Some(age) if age > stale_grace_ms(state) => (
                            "ERROR".to_string(),
                            Some(format!(
                                "{env_label} balance unreadable for {}s — last known ${:.2}. Check the connection, then re-check funds on the venue site.",
                                age / 1000,
                                equity_usd
                            )),
                        ),
                        Some(age) => (
                            "STALE".to_string(),
                            Some(format!(
                                "{env_label} read failed — last known ${:.2} from {}s ago.",
                                equity_usd,
                                age / 1000
                            )),
                        ),
                        None => (status, action),
                    };
                results.push(VenueBalanceInfo {
                    venue: vid.to_string(),
                    equity_usd,
                    available_margin_usd: free_margin_usd,
                    margin_rate_pct,
                    status,
                    recommended_action: action,
                    is_live,
                    account_id: def_acc_id,
                });
            }
            Err(e) => {
                tracing::warn!("failed to fetch {env_label} balance for {name} ({vid}): {e}");
                results.push(VenueBalanceInfo {
                    venue: vid.to_string(),
                    equity_usd: 0.0,
                    available_margin_usd: 0.0,
                    margin_rate_pct: 0.0,
                    status: "ERROR".to_string(),
                    recommended_action: Some(format!(
                        "Failed to retrieve {env_label} balance: {e}"
                    )),
                    is_live,
                    account_id: def_acc_id,
                });
            }
        }
    }

    Ok(results)
}

#[tauri::command]
pub async fn get_vault_status(
    state: State<'_, Arc<AppEngineState>>,
) -> Result<VaultStatus, String> {
    Ok(get_vault_status_impl(&state))
}

#[tauri::command]
pub async fn initialize_vault(
    password: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<VaultStatus, String> {
    initialize_vault_impl(&password, &state)
}

#[tauri::command]
pub async fn unlock_vault(
    password: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<VaultStatus, String> {
    unlock_vault_impl(&password, &state)
}

#[tauri::command]
pub async fn lock_vault(state: State<'_, Arc<AppEngineState>>) -> Result<VaultStatus, String> {
    lock_vault_impl(&state)
}

#[tauri::command]
pub async fn delete_exchange_credentials(
    venue: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<bool, String> {
    delete_exchange_credentials_impl(&venue, &state)
}

#[tauri::command]
pub async fn list_accounts(
    venue: Option<String>,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<AccountDto>, String> {
    list_accounts_impl(venue.as_deref(), &state)
}

#[tauri::command]
pub async fn upsert_account(
    input: AccountUpsertInput,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<AccountDto, String> {
    upsert_account_impl(input, &state)
}

#[tauri::command]
pub async fn delete_account(
    account_id: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<bool, String> {
    delete_account_impl(&account_id, &state)
}

#[tauri::command]
pub async fn delete_account_credentials(
    account_id: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<bool, String> {
    delete_account_credentials_impl(&account_id, &state)
}

#[tauri::command]
pub async fn get_account_credential_field(
    account_id: String,
    field: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<String, String> {
    get_account_credential_field_impl(&account_id, &field, &state)
}

/// Derive the EVM signer address from a hex private key — used by the
/// credentials UI to show "signs as 0x…" so a wrong/stale agent key is
/// visible before a live order fails. The key is validated but never
/// stored or logged.
pub fn derive_evm_signer_address_impl(private_key: &str) -> Result<String, String> {
    arbitrageur_exchanges::wallet::EvmWalletSigner::from_hex(private_key)
        .map(|s| s.address_checksum())
        .map_err(|e| format!("not a valid EVM private key: {e}"))
}

/// Vault-gated signer identity for the STORED privateKey: derives the
/// address server-side and returns only it — the secret never crosses IPC.
/// Lets the operator verify the stored key against the venue's agent list.
pub fn get_account_signer_address_impl(
    account_id: &str,
    state: &AppEngineState,
) -> Result<String, String> {
    // Read the stored value directly — the IPC reveal path refuses sentinel
    // records, but this function only ever returns a public address and the
    // sentinel is the routing input that selects stronghold derivation.
    let exists = state
        .state_store
        .get_account(account_id)
        .map_err(|e| e.to_string())?
        .is_some();
    if !exists {
        return Err(format!("Unknown account '{account_id}'"));
    }
    let key = {
        let mut vault = state.vault.write();
        if !vault.is_unlocked() {
            return Err("Vault is locked. Please unlock before viewing credentials.".into());
        }
        let data = vault.get_data().map_err(|e| e.to_string())?;
        data.credentials
            .iter()
            .find(|c| c.profile_id.as_deref() == Some(account_id))
            .and_then(|c| c.private_key.clone())
            .unwrap_or_default()
    };
    if key.is_empty() {
        return Err("no private key stored for this account".into());
    }
    if key
        .trim()
        .starts_with(arbitrageur_exchanges::evm_amm::EVM_EXTERNAL_SIGNER_PREFIX)
    {
        // External custody: the record carries the sentinel, not key
        // bytes. Prefer the live derivation from the stronghold itself;
        // fall back to the stored wallet address (which was validated to
        // equal the signer when the wallet was saved).
        if let Some(sh) = state.evm_stronghold.read().as_ref() {
            if let Ok(addr) = sh.evm_address() {
                return Ok(addr.to_string());
            }
        }
        let wa = get_account_credential_field_impl(account_id, "walletAddress", state)?;
        if wa.trim().is_empty() {
            return Err("external-custody key has no wallet address on record".into());
        }
        return Ok(wa.trim().to_string());
    }
    derive_evm_signer_address_impl(&key)
}

#[tauri::command]
pub async fn derive_evm_signer_address(private_key: String) -> Result<String, String> {
    derive_evm_signer_address_impl(&private_key)
}

#[tauri::command]
pub async fn get_account_signer_address(
    account_id: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<String, String> {
    get_account_signer_address_impl(&account_id, &state)
}

/// The signer address the active-environment adapter would sign with *right
/// now* — derived from the in-memory provisioned key, not the vault record.
/// Divergence from `get_account_signer_address` means provisioning is
/// stale; equality with a rejected-order address confirms which key signed.
pub fn get_active_signer_address_impl(
    venue: &str,
    state: &AppEngineState,
) -> Result<Option<String>, String> {
    let vid = venue.trim().to_lowercase();
    let is_live = state.is_live();
    if vid == "hyperliquid" || vid == "hl" {
        let adapter = if is_live {
            &state.live_hl
        } else {
            &state.tn_hl
        };
        return Ok(adapter.active_signer_address());
    }
    // Long-tail DEX perps and EVM AMMs exist only on the live set — they
    // have no public testnet, so testnet has no signer to introspect.
    if is_live {
        if let Some(a) = state
            .live_dex_perps
            .iter()
            .find(|a| a.venue_id_str().eq_ignore_ascii_case(&vid))
        {
            return Ok(a.active_signer_address());
        }
        if let Some(a) = state.live_evm_amms.iter().find(|a| a.spec_id() == vid) {
            return Ok(a.wallet_address_hex());
        }
    }
    Err(format!("no signer introspection for venue '{venue}'"))
}

#[tauri::command]
pub async fn get_active_signer_address(
    venue: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Option<String>, String> {
    get_active_signer_address_impl(&venue, &state)
}

/// Read the wallet's current ERC-20 allowance to an EVM AMM venue's router
/// (human units). Live-only — EVM AMMs have no testnet adapters.
#[tauri::command]
pub async fn get_evm_amm_allowance(
    venue: String,
    token: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<f64, String> {
    let vid = venue.trim().to_lowercase();
    let adapter = state
        .live_evm_amms
        .iter()
        .find(|a| a.spec_id() == vid)
        .ok_or_else(|| format!("no EVM AMM adapter for venue '{venue}'"))?;
    adapter
        .router_allowance(token.trim())
        .await
        .map(|d| d.to_string().parse::<f64>().unwrap_or(0.0))
        .map_err(|e| e.to_string())
}

/// Operator setup action: submit an exact-amount ERC-20 approval so the
/// venue router may spend up to `amount` of `token` (the new total cap —
/// swaps draw it down). Deliberately off the dispatch path: an onchain
/// approval waits ~30s for mining, which would leave a sibling CEX leg
/// naked (issue #59). Returns the mined tx hash, or null when the existing
/// allowance already covers `amount`.
#[tauri::command]
pub async fn prepare_evm_amm_allowance(
    venue: String,
    token: String,
    amount: f64,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Option<String>, String> {
    if !(amount.is_finite() && amount > 0.0) {
        return Err("amount must be a positive finite number".to_string());
    }
    let vid = venue.trim().to_lowercase();
    let adapter = state
        .live_evm_amms
        .iter()
        .find(|a| a.spec_id() == vid)
        .ok_or_else(|| format!("no EVM AMM adapter for venue '{venue}'"))?;
    let amount_dec: arbitrageur_core::types::Decimal = amount
        .to_string()
        .parse()
        .map_err(|e| format!("invalid amount: {e}"))?;
    adapter
        .approve_router_token(token.trim(), amount_dec)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn set_default_account(
    venue: String,
    account_id: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<(), String> {
    set_default_account_impl(&venue, &account_id, &state)
}

#[tauri::command]
pub async fn list_strategy_routes(
    strategy_id: Option<String>,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<StrategyAccountRoute>, String> {
    list_strategy_routes_impl(strategy_id.as_deref(), &state)
}

#[tauri::command]
pub async fn upsert_strategy_route(
    input: StrategyRouteUpsertInput,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<StrategyAccountRoute, String> {
    upsert_strategy_route_impl(input, &state)
}

#[tauri::command]
pub async fn delete_strategy_route(
    route_id: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<bool, String> {
    delete_strategy_route_impl(&route_id, &state)
}

#[tauri::command]
pub async fn eligible_strategy_routes(
    strategy_id: String,
    venue_a: String,
    venue_b: String,
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<StrategyAccountRoute>, String> {
    eligible_strategy_routes_impl(&strategy_id, &venue_a, &venue_b, &state)
}

#[tauri::command]
pub async fn get_venue_balances(
    state: State<'_, Arc<AppEngineState>>,
) -> Result<Vec<VenueBalanceInfo>, String> {
    get_venue_balances_impl(&state).await
}
