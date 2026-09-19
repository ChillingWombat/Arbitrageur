//! PopDEX perpetual CLOB connector (closed-beta credential model).
//!
//! # Connectivity model (researched 2026-09-19)
//!
//! PopDEX is a closed-beta perpetual CLOB on Morph (frontend at
//! `https://app.popdex.xyz/trade`). Probing showed:
//!
//! - `api.popdex.trade` (our old default) is a **parked domain** — it never
//!   was a real endpoint and must not be used.
//! - `app.popdex.xyz/api/*` serves the SPA shell, not JSON; the trading
//!   bundle calls same-origin `/v1/uta/...` paths (Bitget-UTA-shaped order
//!   placement) proxied through the app backend with session auth. There is
//!   **no public, unauthenticated market-data or trading API**.
//!
//! # Secure connection path
//!
//! 1. The operator obtains beta API credentials (base URL + key/secret,
//!    and/or the venue EVM wallet) out of band.
//! 2. Credentials are pasted in System Settings → Exchange Accounts and held
//!    in the encrypted vault; the adapter receives them via
//!    [`PopDexAdapter::update_credentials`] (EVM keys load into a zeroizing
//!    [`crate::wallet::EvmWalletSigner`] — raw strings are never retained).
//! 3. Until credentials are configured, every network path fails closed with
//!    an actionable message. Known-dead placeholder hosts are normalized to
//!    "unconfigured" so a stale setting can never silently point at a parked
//!    domain.
//!
//! When PopDEX publishes its beta REST shape, implement market-data parsing
//! against `base_url` here; the plumbing (client, timeouts, credential
//! binding, fail-closed errors) is already in place.

use std::time::Duration;

use async_trait::async_trait;
use parking_lot::RwLock;
use reqwest::Client;

use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::market::MarketMetadata;
use arbitrageur_core::order::{CancelRequest, OrderRequest, OrderResponse};
use arbitrageur_core::position::AccountSnapshot;
use arbitrageur_core::quote::{FundingRate, OrderBook, Ticker};
use arbitrageur_core::types::VenueId;

use crate::wallet::{wallet_execution_unavailable, EvmWalletSigner};
use zeroize::Zeroizing;

/// Placeholder hosts that must never be treated as configured endpoints.
/// `api.popdex.trade` is a parked domain; `api.popdex.xyz` 404s.
const DEAD_HOSTS: &[&str] = &["api.popdex.trade", "api.popdex.xyz"];

fn host_of(url: &str) -> String {
    url.trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or("")
        .to_lowercase()
}

fn normalize_base_url(raw: Option<String>) -> Option<String> {
    let b = raw
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())?;
    if DEAD_HOSTS.iter().any(|h| host_of(&b) == *h) {
        return None;
    }
    Some(b)
}

fn has_secret(s: Option<&str>) -> bool {
    s.is_some_and(|v| !v.trim().is_empty())
}

/// PopDEX connector — inert until beta credentials are configured.
pub struct PopDexAdapter {
    venue_id: VenueId,
    client: Client,
    base_url: RwLock<Option<String>>,
    api_key: RwLock<Option<Zeroizing<String>>>,
    api_secret: RwLock<Option<Zeroizing<String>>>,
    wallet_address: RwLock<Option<String>>,
    wallet: RwLock<Option<EvmWalletSigner>>,
}

impl PopDexAdapter {
    pub fn new(
        base_url: Option<String>,
        api_key: Option<String>,
        api_secret: Option<String>,
    ) -> Self {
        let key = api_key.map(|k| k.trim().to_string()).filter(|k| !k.is_empty());
        let secret = api_secret
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        Self {
            venue_id: VenueId::new("popdex"),
            client: Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap_or_default(),
            base_url: RwLock::new(normalize_base_url(base_url)),
            api_key: RwLock::new(key.map(Zeroizing::new)),
            api_secret: RwLock::new(secret.map(Zeroizing::new)),
            wallet_address: RwLock::new(None),
            wallet: RwLock::new(None),
        }
    }

    /// Rebind credentials at runtime (Settings save / vault unlock).
    /// Dead placeholder hosts are normalized to unconfigured; an invalid EVM
    /// key fails the whole update so a half-bound state is impossible.
    pub fn update_credentials(
        &self,
        base_url: Option<String>,
        api_key: Option<String>,
        api_secret: Option<String>,
        wallet_address: Option<String>,
        private_key_hex: Option<String>,
    ) -> anyhow::Result<()> {
        if let Some(hex_key) = private_key_hex
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty())
        {
            let signer = EvmWalletSigner::from_hex(&hex_key)?;
            if let Some(a) = wallet_address.as_deref().map(str::trim).filter(|a| !a.is_empty()) {
                if !a.eq_ignore_ascii_case(&signer.address_hex()) {
                    anyhow::bail!("PopDEX wallet address does not match the supplied private key");
                }
            }
            *self.wallet.write() = Some(signer);
            *self.wallet_address.write() = wallet_address
                .map(|a| a.trim().to_string())
                .filter(|a| !a.is_empty());
        } else {
            *self.wallet.write() = None;
            *self.wallet_address.write() = wallet_address
                .map(|a| a.trim().to_string())
                .filter(|a| !a.is_empty());
        }
        *self.base_url.write() = normalize_base_url(base_url);
        *self.api_key.write() = api_key.map(|k| k.trim().to_string()).filter(|k| !k.is_empty()).map(Zeroizing::new);
        *self.api_secret.write() = api_secret
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(Zeroizing::new);
        Ok(())
    }

    /// True only when a real (non-placeholder) base URL **and** an API key
    /// are bound. Wallet-only binding enables future contract execution, not
    /// REST market data.
    pub fn is_configured(&self) -> bool {
        self.base_url.read().is_some() && has_secret(self.api_key.read().as_deref().map(String::as_str))
    }

    fn not_configured_error(&self, what: &str) -> anyhow::Error {
        anyhow::anyhow!(
            "PopDEX {what} unavailable: no beta API endpoint configured. PopDEX is in closed beta \
             with no public market-data API (its app proxies signed requests through its own backend). \
             Paste the beta API base URL + key in System Settings → Exchange Accounts to connect; \
             until then this venue is inert and contributes no signals. Refusing to fabricate {what}."
        )
    }
}

#[async_trait]
impl ExchangeAdapter for PopDexAdapter {
    fn venue_id(&self) -> VenueId {
        self.venue_id.clone()
    }

    fn name(&self) -> &str {
        "PopDEX"
    }

    async fn ping(&self) -> anyhow::Result<()> {
        if !self.is_configured() {
            return Err(self.not_configured_error("connectivity probe"));
        }
        // Beta shape is unpublished; a HEAD/GET against the configured root
        // proves reachability without assuming any JSON contract.
        let base = self.base_url.read().clone().unwrap_or_default();
        let resp = self.client.get(&base).send().await?;
        if resp.status().is_success() {
            Ok(())
        } else {
            anyhow::bail!("PopDEX beta endpoint returned status {}", resp.status());
        }
    }

    async fn fetch_markets(&self) -> anyhow::Result<Vec<MarketMetadata>> {
        // No public market list exists and the beta REST shape is unpublished;
        // returning a hardcoded list would fabricate venue state.
        Err(self.not_configured_error("market list"))
    }

    async fn fetch_orderbook(&self, symbol: &str) -> anyhow::Result<OrderBook> {
        Err(self.not_configured_error(&format!("orderbook for {symbol}")))
    }

    async fn fetch_ticker(&self, symbol: &str) -> anyhow::Result<Ticker> {
        Err(self.not_configured_error(&format!("ticker for {symbol}")))
    }

    async fn fetch_funding_rate(&self, symbol: &str) -> anyhow::Result<FundingRate> {
        Err(self.not_configured_error(&format!("funding rate for {symbol}")))
    }

    async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
        Err(self.not_configured_error("account snapshot"))
    }

    async fn place_order(&self, _order: OrderRequest) -> anyhow::Result<OrderResponse> {
        Err(wallet_execution_unavailable(
            "PopDEX",
            "Morph L2",
            "beta-API order placement or wallet-signed CLOB order via the venue backend",
        ))
    }

    async fn cancel_order(&self, _req: CancelRequest) -> anyhow::Result<()> {
        Err(wallet_execution_unavailable(
            "PopDEX",
            "Morph L2",
            "beta-API order cancellation",
        ))
    }

    fn supports_live_execution(&self) -> bool {
        false
    }

    fn has_live_credentials(&self) -> bool {
        self.is_configured() || self.wallet.read().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arbitrageur_core::types::{dec, Side};

    #[test]
    fn unconfigured_by_default_and_dead_hosts_normalized() {
        let a = PopDexAdapter::new(None, None, None);
        assert!(!a.is_configured());
        assert!(!a.has_live_credentials());

        for dead in [
            "https://api.popdex.trade",
            "https://api.popdex.trade/",
            "https://api.popdex.xyz",
        ] {
            let b = PopDexAdapter::new(Some(dead.into()), Some("key".into()), None);
            assert!(!b.is_configured(), "{dead} must normalize to unconfigured");
        }

        let c = PopDexAdapter::new(Some("https://beta.example".into()), Some("key".into()), None);
        assert!(c.is_configured());
        assert!(c.has_live_credentials());
    }

    #[tokio::test]
    async fn market_data_fails_closed_with_guidance() {
        let a = PopDexAdapter::new(None, None, None);
        assert!(!a.supports_live_execution());
        for err in [
            a.fetch_markets().await.unwrap_err(),
            a.fetch_orderbook("BTC-PERP").await.unwrap_err(),
            a.fetch_ticker("BTC-PERP").await.unwrap_err(),
            a.fetch_funding_rate("BTC-PERP").await.unwrap_err(),
            a.fetch_account().await.unwrap_err(),
        ] {
            let msg = format!("{err:?}");
            assert!(msg.contains("closed beta"), "{msg}");
            assert!(msg.contains("Refusing to fabricate"), "{msg}");
        }
        let exec_err = a
            .place_order(OrderRequest::new_limit(a.venue_id(), "BTC-PERP", Side::Buy, dec!(1), dec!(1)))
            .await
            .unwrap_err();
        assert!(format!("{exec_err:?}").contains("no API-key trading"));
    }

    #[test]
    fn credential_update_validates_wallet_binding() {
        let a = PopDexAdapter::new(None, None, None);
        let key = "0000000000000000000000000000000000000000000000000000000000000001";
        a.update_credentials(
            None,
            None,
            None,
            Some("0x7e5f4552091a69125d5dfcb7b8c2659029395bdf".into()),
            Some(key.into()),
        )
        .unwrap();
        assert!(a.has_live_credentials());
        assert!(!a.is_configured());
        // Bad key must fail the update, not half-bind.
        assert!(a
            .update_credentials(None, None, None, None, Some("zz".into()))
            .is_err());
    }
}
