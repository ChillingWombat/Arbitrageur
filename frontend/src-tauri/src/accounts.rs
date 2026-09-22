//! Vault lifecycle, exchange credentials, account profiles, strategy
//! routing, and venue balances. Split out of `commands.rs`; the few shared
//! helpers it needs are reached via `crate::commands`.

use crate::commands::{
    live_account_or_cached, margin_monitor_from_settings, position_signature, stale_grace_ms,
};
use crate::state::{AppEngineState, VenueBalanceInfo};
use crate::state_store::{AccountProfile, StrategyAccountRoute};
use crate::vault::{CredentialPresence, ExchangeCredentials, VaultStatus};
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
    state.sync_vault_credentials_to_adapters();
    Ok(status)
}

pub fn unlock_vault_impl(password: &str, state: &AppEngineState) -> Result<VaultStatus, String> {
    let status = {
        let mut vault = state.vault.write();
        vault.unlock(password).map_err(|e| e.to_string())?;
        vault.get_status()
    };
    ensure_credential_migration(state)?;
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
        }
    }
    Ok(status)
}

pub fn lock_vault_impl(state: &AppEngineState) -> VaultStatus {
    let status = {
        let mut vault = state.vault.write();
        vault.lock();
        vault.get_status()
    };
    state.sync_vault_credentials_to_adapters();
    status
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
            "privateKey" => c.private_key.clone().unwrap_or_default(),
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
    Ok(lock_vault_impl(&state))
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
    let key = get_account_credential_field_impl(account_id, "privateKey", state)?;
    if key.is_empty() {
        return Err("no private key stored for this account".into());
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
    // Long-tail DEX perps exist only on the live set — they have no public
    // testnet, so testnet has no signer to introspect.
    if is_live {
        if let Some(a) = state
            .live_dex_perps
            .iter()
            .find(|a| a.venue_id_str().eq_ignore_ascii_case(&vid))
        {
            return Ok(a.active_signer_address());
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
