use crate::rate_limiter::AccountRequestCoordinator;
use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::market::{MarketMetadata, Symbol};
use arbitrageur_core::order::{CancelRequest, OrderRequest, OrderResponse};
use arbitrageur_core::position::AccountSnapshot;
use arbitrageur_core::quote::{BookLevel, FundingRate, OrderBook, Ticker};
use arbitrageur_core::symbol::canonical_base;
use arbitrageur_core::types::{dec, Decimal, VenueId};
use async_trait::async_trait;
use chrono::Utc;
use parking_lot::RwLock;
use reqwest::Client;
use zeroize::Zeroizing;

/// Arcus perpetuals connector — a real CLOB on its own appchain.
///
/// Endpoints (documented at docs.arcus.xyz): live `https://api.arcus.xyz`,
/// testnet `https://api.testnet.arcus.xyz` — identical paths on both hosts,
/// environment selects by base URL. Market data is unauthenticated
/// (`/v1/markets`, `/v1/l2OrderBook/{market}`, `/v1/mids`, `/v1/feetiers`);
/// order routing requires per-request Ed25519 signatures which are not
/// implemented here, so order and account paths fail closed rather than
/// fabricate. The venue is a market-data peer: admitted in both
/// environments, excluded from execution by `supports_live_execution`.
pub struct ArcusAdapter {
    venue_id: VenueId,
    client: Client,
    base_url: String,
    api_key: RwLock<Option<Zeroizing<String>>>,
    api_secret: RwLock<Option<Zeroizing<String>>>,
    rate_limiter: AccountRequestCoordinator,
}

impl ArcusAdapter {
    pub fn new(
        base_url: Option<String>,
        api_key: Option<String>,
        api_secret: Option<String>,
    ) -> Self {
        // api.arcus.exchange does not resolve; the live host is
        // api.arcus.xyz (verified via /health, 2026-09). Persisted settings
        // may still carry the dead hostname — normalize rather than pin a
        // broken endpoint.
        let mut base = base_url.unwrap_or_else(|| "https://api.arcus.xyz".to_string());
        if base.contains("api.arcus.exchange") {
            base = "https://api.arcus.xyz".to_string();
        }
        Self {
            venue_id: VenueId::new("arcus"),
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap_or_default(),
            base_url: base,
            api_key: RwLock::new(api_key.map(Zeroizing::new)),
            api_secret: RwLock::new(api_secret.map(Zeroizing::new)),
            rate_limiter: AccountRequestCoordinator::new("arcus", 100, 2000, 500, 10000),
        }
    }

    /// Rotate or clear credentials — Arcus reads are public today; the
    /// stored key pair is what order signing will use once Ed25519 request
    /// signing lands.
    pub fn update_credentials(&self, api_key: Option<String>, api_secret: Option<String>) {
        *self.api_key.write() = api_key.map(Zeroizing::new);
        *self.api_secret.write() = api_secret.map(Zeroizing::new);
    }

    async fn get_json(&self, path: &str) -> anyhow::Result<serde_json::Value> {
        if let Some(cached_str) = self.rate_limiter.acquire_or_cache_path(path, true).await? {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&cached_str) {
                return Ok(val);
            }
        }

        let url = format!("{}{}", self.base_url, path);
        let resp = match self.client.get(&url).send().await {
            Ok(r) => r,
            Err(e) => {
                self.rate_limiter.record_failure(false);
                return Err(e.into());
            }
        };

        if !resp.status().is_success() {
            let is_429 = resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS;
            self.rate_limiter.record_failure(is_429);
            anyhow::bail!("Arcus API error: {} {}", resp.status(), path);
        }

        let text = resp.text().await?;
        self.rate_limiter.record_success_path(path, text.clone());
        let data = serde_json::from_str::<serde_json::Value>(&text)?;
        Ok(data)
    }
}

/// Engine symbol → Arcus market name. Arcus perps quote USD exclusively
/// and spell markets `BASE-USD`; the engine's canonical forms (`BTC`,
/// `BTC-USD`, `BTC-USDC`, `BTC-PERP`) all reduce through the shared
/// canonical-base normalizer.
fn arcus_market_name(symbol: &str) -> String {
    format!("{}-USD", canonical_base(symbol))
}

/// Decimal places implied by a string step/tick size ("0.1" → 1,
/// "0.00000001" → 8, "5" → 0). The venue publishes grid sizes as strings;
/// scale is the wire-truthful precision.
fn decimals_of(value: &str) -> u32 {
    Decimal::from_str_exact(value.trim())
        .map(|d| d.scale())
        .unwrap_or(0)
}

#[async_trait]
impl ExchangeAdapter for ArcusAdapter {
    fn venue_id(&self) -> VenueId {
        self.venue_id.clone()
    }

    fn name(&self) -> &str {
        "Arcus"
    }

    fn rest_base_url(&self) -> Option<String> {
        Some(self.base_url.clone())
    }

    /// `/health` is a dedicated liveness endpoint — cheaper than a markets
    /// pull and bypasses the response cache the way a probe must.
    async fn ping(&self) -> anyhow::Result<()> {
        let url = format!("{}/health", self.base_url);
        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("Arcus health probe: HTTP {}", resp.status());
        }
        Ok(())
    }

    async fn fetch_markets(&self) -> anyhow::Result<Vec<MarketMetadata>> {
        let res = self.get_json("/v1/markets").await?;
        let rows = res
            .get("markets")
            .and_then(|m| m.as_array())
            .ok_or_else(|| anyhow::anyhow!("Arcus /v1/markets response missing markets array"))?;
        let mut markets = Vec::new();
        for m in rows {
            // Only perpetuals ride this adapter — the spot router is a
            // separate API surface (router.spot.arcus.xyz) not wired here.
            if m.get("type").and_then(|t| t.as_str()) != Some("PERPETUAL") {
                continue;
            }
            let display = m
                .get("marketDisplayName")
                .and_then(|d| d.as_str())
                .unwrap_or("");
            let Some((base, quote)) = display.rsplit_once('-') else {
                continue;
            };
            if base.is_empty() || quote.is_empty() {
                continue;
            }
            let str_dec = |key: &str| -> Option<Decimal> {
                m.get(key)
                    .and_then(|v| v.as_str())
                    .and_then(|s| Decimal::from_str_exact(s).ok())
            };
            markets.push(MarketMetadata {
                symbol: Symbol::perp(base, quote),
                venue: self.venue_id.clone(),
                venue_symbol: display.to_string(),
                price_decimals: m
                    .get("tickSize")
                    .and_then(|t| t.as_str())
                    .map(decimals_of)
                    .unwrap_or(2),
                size_decimals: m
                    .get("stepSize")
                    .and_then(|s| s.as_str())
                    .map(decimals_of)
                    .unwrap_or(3),
                min_order_size: str_dec("minOrderSize").unwrap_or(dec!(0.0001)),
                min_notional: str_dec("minOrderNotional").unwrap_or(dec!(5.0)),
                // Base fee tier, verified against GET /v1/feetiers (level 0:
                // 150ppm maker / 450ppm taker). Higher-volume tiers improve
                // on this — using the base tier keeps route economics
                // conservative rather than flattering.
                maker_fee_bps: dec!(1.5),
                taker_fee_bps: dec!(4.5),
                contract_size: dec!(1.0),
                is_active: m.get("status").and_then(|s| s.as_str()) == Some("ONLINE"),
            });
        }
        Ok(markets)
    }

    /// `/v1/mids` is the venue's bulk mid-price map keyed by market name —
    /// exactly what the ticker universe seeds from.
    async fn fetch_market_prices(
        &self,
    ) -> anyhow::Result<std::collections::HashMap<String, Decimal>> {
        let res = self.get_json("/v1/mids").await?;
        let mut out = std::collections::HashMap::new();
        if let Some(mids) = res.get("mids").and_then(|m| m.as_object()) {
            for (market, price) in mids {
                if let Some(p) = price.as_str().and_then(|s| Decimal::from_str_exact(s).ok()) {
                    if p > dec!(0) {
                        out.insert(market.clone(), p);
                    }
                }
            }
        }
        Ok(out)
    }

    async fn fetch_orderbook(&self, symbol: &str) -> anyhow::Result<OrderBook> {
        let market = arcus_market_name(symbol);
        let path = format!("/v1/l2OrderBook/{}?nLevels=50", market);
        let res = self.get_json(&path).await?;

        let mut book = OrderBook::new(self.venue_id.clone(), symbol.to_string());
        book.timestamp = Utc::now();

        let parse_side = |side: Option<&serde_json::Value>, dst: &mut Vec<BookLevel>| {
            if let Some(levels) = side.and_then(|s| s.as_array()) {
                for lvl in levels {
                    let Some(pair) = lvl.as_array() else { continue };
                    let p = pair
                        .first()
                        .and_then(|v| v.as_str())
                        .and_then(|s| Decimal::from_str_exact(s).ok());
                    let s = pair
                        .get(1)
                        .and_then(|v| v.as_str())
                        .and_then(|s| Decimal::from_str_exact(s).ok());
                    if let (Some(p), Some(s)) = (p, s) {
                        if p > dec!(0) && s > dec!(0) {
                            dst.push(BookLevel::new(p, s));
                        }
                    }
                }
            }
        };
        parse_side(res.get("bids"), &mut book.bids);
        parse_side(res.get("asks"), &mut book.asks);

        if book.bids.is_empty() && book.asks.is_empty() {
            anyhow::bail!(
                "No live orderbook data returned from Arcus API for {}",
                symbol
            );
        }

        Ok(book)
    }

    async fn fetch_ticker(&self, symbol: &str) -> anyhow::Result<Ticker> {
        let book = self.fetch_orderbook(symbol).await?;
        let bid = book.best_bid_price().unwrap_or(dec!(0));
        let ask = book.best_ask_price().unwrap_or(dec!(0));
        Ok(Ticker {
            venue: self.venue_id.clone(),
            symbol: symbol.to_string(),
            bid,
            ask,
            last: (bid + ask) / dec!(2),
            timestamp: Utc::now(),
        })
    }

    /// Arcus funding is hourly; `GET /v1/markets` carries the current
    /// `fundingRate`, the projected `nextFundingRate`, and the epoch-second
    /// `nextFundingAt` per market — the venue's own print, not a default.
    async fn fetch_funding_rate(&self, symbol: &str) -> anyhow::Result<FundingRate> {
        let market = arcus_market_name(symbol);
        let res = self.get_json("/v1/markets").await?;
        let rows = res
            .get("markets")
            .and_then(|m| m.as_array())
            .ok_or_else(|| anyhow::anyhow!("Arcus /v1/markets response missing markets array"))?;
        let m = rows
            .iter()
            .find(|r| r.get("marketDisplayName").and_then(|d| d.as_str()) == Some(market.as_str()))
            .ok_or_else(|| anyhow::anyhow!("Arcus market {} not listed", market))?;
        // Fail-closed: a missing print is an outage, not a 0% print — a
        // silent zero would mint phantom carry against any nonzero leg.
        let rate = m
            .get("fundingRate")
            .and_then(|r| r.as_str())
            .and_then(|s| Decimal::from_str_exact(s).ok())
            .ok_or_else(|| {
                anyhow::anyhow!("Arcus funding response missing fundingRate for {symbol}")
            })?;
        let predicted = m
            .get("nextFundingRate")
            .and_then(|r| r.as_str())
            .and_then(|s| Decimal::from_str_exact(s).ok());
        let next_at = m
            .get("nextFundingAt")
            .and_then(|t| t.as_i64())
            .and_then(|secs| chrono::DateTime::from_timestamp(secs, 0))
            .unwrap_or_else(|| Utc::now() + chrono::Duration::hours(1));
        Ok(FundingRate {
            venue: self.venue_id.clone(),
            symbol: symbol.to_string(),
            rate,
            predicted_rate: predicted,
            interval_hours: 1,
            next_funding_time: next_at,
            timestamp: Utc::now(),
        })
    }

    async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
        anyhow::bail!(
            "Account fetch is not implemented for {}; refusing to fabricate an account snapshot",
            self.name()
        )
    }

    async fn place_order(&self, _order: OrderRequest) -> anyhow::Result<OrderResponse> {
        anyhow::bail!(
            "Order execution requires Ed25519 request signing, not implemented for {}; refusing to fabricate a fill",
            self.name()
        )
    }

    async fn cancel_order(&self, _req: CancelRequest) -> anyhow::Result<()> {
        anyhow::bail!(
            "Order cancellation requires Ed25519 request signing, not implemented for {}",
            self.name()
        )
    }

    fn has_live_credentials(&self) -> bool {
        let k = self.api_key.read();
        let s = self.api_secret.read();
        matches!(
            (k.as_deref(), s.as_deref()),
            (Some(k), Some(s)) if !k.trim().is_empty() && !s.trim().is_empty()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn market_name_reduces_engine_symbols_to_venue_spelling() {
        // Arcus quotes USD on every perp; all engine spellings reduce to
        // the BASE-USD venue name.
        assert_eq!(arcus_market_name("BTC-USD"), "BTC-USD");
        assert_eq!(arcus_market_name("BTC-USDC"), "BTC-USD");
        assert_eq!(arcus_market_name("BTC"), "BTC-USD");
        assert_eq!(arcus_market_name("amd"), "AMD-USD");
    }

    #[test]
    fn decimals_of_parses_string_grids() {
        assert_eq!(decimals_of("0.1"), 1);
        assert_eq!(decimals_of("0.00000001"), 8);
        assert_eq!(decimals_of("5"), 0);
        assert_eq!(decimals_of("junk"), 0);
    }

    #[tokio::test]
    async fn order_paths_fail_closed_without_signing() {
        let a = ArcusAdapter::new(
            Some("https://api.testnet.arcus.xyz".to_string()),
            None,
            None,
        );
        let order = OrderRequest::new_limit(
            VenueId::new("arcus"),
            "BTC-USD",
            arbitrageur_core::types::Side::Buy,
            dec!(80000.0),
            dec!(0.001),
        );
        assert!(a.place_order(order).await.is_err());
        assert!(!a.has_live_credentials());
    }
}
