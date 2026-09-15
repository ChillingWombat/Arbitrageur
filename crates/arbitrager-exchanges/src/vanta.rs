use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use arbitrager_core::exchange::ExchangeAdapter;
use arbitrager_core::market::{MarketMetadata, Symbol};
use arbitrager_core::order::{CancelRequest, OrderRequest, OrderResponse};
use arbitrager_core::position::AccountSnapshot;
use arbitrager_core::quote::{BookLevel, FundingRate, OrderBook, Ticker};
use arbitrager_core::types::{dec, Decimal, VenueId};
use crate::rate_limiter::AccountRequestCoordinator;

/// Vanta Exchange (Orderly Network broker protocol) connector.
/// Vanta's perpetual API is hosted on Orderly; public read-only calls use
/// POST {base}/v1/public/query without credentials.
#[allow(dead_code)]
pub struct VantaAdapter {
    venue_id: VenueId,
    client: Client,
    base_url: String,
    account_id: Option<String>,
    orderly_key: Option<String>,
    orderly_secret: Option<String>,
    rate_limiter: AccountRequestCoordinator,
}

fn dec_from_json(v: Option<&serde_json::Value>) -> Option<Decimal> {
    match v? {
        serde_json::Value::String(s) => Decimal::from_str_exact(s).ok(),
        serde_json::Value::Number(n) => Decimal::from_str_exact(&n.to_string()).ok(),
        _ => None,
    }
}

fn ts_from_json(v: Option<&serde_json::Value>) -> Option<DateTime<Utc>> {
    let ms = match v? {
        serde_json::Value::Number(n) => n.as_i64()?,
        serde_json::Value::String(s) => s.parse::<i64>().ok()?,
        _ => return None,
    };
    DateTime::from_timestamp_millis(ms)
}

/// Exact standard Orderly perp symbol `PERP_<BASE>_USDC`; broker-suffixed or
/// otherwise malformed symbols do not match.
fn standard_symbol_base(symbol: &str) -> Option<&str> {
    let mut parts = symbol.split('_');
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some("PERP"), Some(base), Some("USDC"), None) if !base.is_empty() => Some(base),
        _ => None,
    }
}

fn parse_market_summary_entry(venue: &VenueId, m: &serde_json::Value) -> Option<MarketMetadata> {
    let symbol_str = m.get("symbol")?.as_str()?;
    let base = standard_symbol_base(symbol_str)?;
    let quote_tick = dec_from_json(m.get("quote_tick"))?;
    let base_tick = dec_from_json(m.get("base_tick"))?;
    let min_notional = dec_from_json(m.get("min_notional"))?;
    if !(quote_tick > dec!(0) && base_tick > dec!(0) && min_notional >= dec!(0)) {
        return None;
    }
    Some(MarketMetadata {
        symbol: Symbol::perp(base, "USDC"),
        venue: venue.clone(),
        venue_symbol: symbol_str.to_string(),
        price_decimals: quote_tick.normalize().scale(),
        size_decimals: base_tick.normalize().scale(),
        min_order_size: base_tick,
        min_notional,
        maker_fee_bps: dec!(2.0),
        taker_fee_bps: dec!(5.0),
        contract_size: dec!(1.0),
        is_active: true,
    })
}

fn market_entry<'a>(
    data: &'a serde_json::Value,
    vanta_sym: &str,
) -> anyhow::Result<&'a serde_json::Value> {
    data.get("markets")
        .and_then(|m| m.as_array())
        .ok_or_else(|| anyhow::anyhow!("Vanta marketSummary response missing data.markets"))?
        .iter()
        .find(|m| m.get("symbol").and_then(|s| s.as_str()) == Some(vanta_sym))
        .ok_or_else(|| anyhow::anyhow!("Vanta marketSummary has no entry for {vanta_sym}"))
}

fn parse_orderbook_data(
    venue: &VenueId,
    symbol: &str,
    vanta_sym: &str,
    data: &serde_json::Value,
) -> anyhow::Result<OrderBook> {
    let returned = data
        .get("symbol")
        .and_then(|s| s.as_str())
        .ok_or_else(|| anyhow::anyhow!("Vanta orderbook response missing data.symbol"))?;
    if returned != vanta_sym {
        anyhow::bail!(
            "Vanta orderbook returned symbol {returned} for requested {vanta_sym}"
        );
    }
    let mut book = OrderBook::new(venue.clone(), symbol.to_string());
    if let Some(levels) = data.get("bids").and_then(|l| l.as_array()) {
        for lvl in levels {
            if let (Some(p), Some(q)) = (
                dec_from_json(lvl.get("price")),
                dec_from_json(lvl.get("quantity")),
            ) {
                book.bids.push(BookLevel::new(p, q));
            }
        }
    }
    if let Some(levels) = data.get("asks").and_then(|l| l.as_array()) {
        for lvl in levels {
            if let (Some(p), Some(q)) = (
                dec_from_json(lvl.get("price")),
                dec_from_json(lvl.get("quantity")),
            ) {
                book.asks.push(BookLevel::new(p, q));
            }
        }
    }
    book.timestamp = Utc::now();
    Ok(book)
}

fn parse_ticker_data(
    venue: &VenueId,
    symbol: &str,
    vanta_sym: &str,
    data: &serde_json::Value,
) -> anyhow::Result<Ticker> {
    let m = market_entry(data, vanta_sym)?;
    let bid = dec_from_json(m.get("bid_price"))
        .filter(|d| *d > dec!(0))
        .ok_or_else(|| {
            anyhow::anyhow!("Vanta marketSummary for {vanta_sym} has no positive bid_price")
        })?;
    let ask = dec_from_json(m.get("ask_price"))
        .filter(|d| *d > dec!(0))
        .ok_or_else(|| {
            anyhow::anyhow!("Vanta marketSummary for {vanta_sym} has no positive ask_price")
        })?;
    let last = dec_from_json(m.get("mark_price"))
        .filter(|d| *d > dec!(0))
        .unwrap_or((bid + ask) / dec!(2));
    Ok(Ticker {
        venue: venue.clone(),
        symbol: symbol.to_string(),
        bid,
        ask,
        last,
        timestamp: Utc::now(),
    })
}

fn parse_funding_data(
    venue: &VenueId,
    symbol: &str,
    vanta_sym: &str,
    data: &serde_json::Value,
) -> anyhow::Result<FundingRate> {
    let m = market_entry(data, vanta_sym)?;
    let rate = dec_from_json(m.get("last_funding_rate")).ok_or_else(|| {
        anyhow::anyhow!("Vanta marketSummary for {vanta_sym} has no parseable last_funding_rate")
    })?;
    let predicted_rate = dec_from_json(m.get("est_funding_rate"));
    let next_funding_time = ts_from_json(m.get("next_funding_time")).ok_or_else(|| {
        anyhow::anyhow!("Vanta marketSummary for {vanta_sym} has no valid next_funding_time")
    })?;
    Ok(FundingRate {
        venue: venue.clone(),
        symbol: symbol.to_string(),
        rate,
        predicted_rate,
        interval_hours: 8,
        next_funding_time,
        timestamp: Utc::now(),
    })
}

impl VantaAdapter {
    pub fn new(
        base_url: Option<String>,
        account_id: Option<String>,
        orderly_key: Option<String>,
        orderly_secret: Option<String>,
    ) -> Self {
        let base = base_url.unwrap_or_else(|| "https://api.orderly.org".to_string());
        Self {
            venue_id: VenueId::new("vanta"),
            client: Client::builder().build().unwrap_or_default(),
            base_url: base,
            account_id,
            orderly_key,
            orderly_secret,
            rate_limiter: AccountRequestCoordinator::new("vanta", 150, 2000, 500, 10000),
        }
    }

    /// Futures symbol normalization. Orderly perp ids are `PERP_*`; a spot
    /// request (either `SPOT_*` exchange format or a `-SPOT` market-type
    /// suffix) must fail closed — silently mapping it onto the perp book
    /// would misattribute the market type.
    fn to_vanta_symbol(symbol: &str) -> anyhow::Result<String> {
        let tokens: Vec<&str> = symbol.split(['-', '/', '_']).collect();
        let first = tokens.first().copied().unwrap_or("");
        let last = tokens.last().copied().unwrap_or("");
        if first.eq_ignore_ascii_case("spot") || last.eq_ignore_ascii_case("spot") {
            anyhow::bail!(
                "Vanta spot instrument '{symbol}' is not supported by this adapter's futures API; refusing to query a perp market"
            );
        }
        let clean = symbol.replace('-', "_");
        if clean.starts_with("PERP_") {
            Ok(clean)
        } else {
            Ok(format!(
                "PERP_{}_USDC",
                symbol.split('-').next().unwrap_or(symbol)
            ))
        }
    }

    #[allow(dead_code)]
    fn from_vanta_symbol(vanta_symbol: &str) -> String {
        vanta_symbol
            .strip_prefix("PERP_")
            .unwrap_or(vanta_symbol)
            .replace('_', "-")
    }

    /// POST /v1/public/query — the documented public read-only endpoint.
    /// Returns the `data` payload; requires HTTP success and `success: true`.
    async fn public_query(&self, body: serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let url = format!("{}/v1/public/query", self.base_url);
        let resp = self.client.post(&url).json(&body).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("Vanta public query HTTP status: {}", resp.status());
        }
        let json_val = resp.json::<serde_json::Value>().await?;
        if json_val.get("success").and_then(|s| s.as_bool()) != Some(true) {
            anyhow::bail!("Vanta public query did not report success");
        }
        json_val
            .get("data")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Vanta public query response missing data"))
    }
}

#[async_trait]
impl ExchangeAdapter for VantaAdapter {
    fn venue_id(&self) -> VenueId {
        self.venue_id.clone()
    }

    fn name(&self) -> &str {
        "Vanta.exchange"
    }

    async fn fetch_markets(&self) -> anyhow::Result<Vec<MarketMetadata>> {
        let data = self
            .public_query(serde_json::json!({"type": "marketSummary"}))
            .await?;
        let markets = data
            .get("markets")
            .and_then(|m| m.as_array())
            .ok_or_else(|| {
                anyhow::anyhow!("Vanta marketSummary response missing data.markets")
            })?;
        Ok(markets
            .iter()
            .filter_map(|m| parse_market_summary_entry(&self.venue_id, m))
            .collect())
    }

    async fn fetch_orderbook(&self, symbol: &str) -> anyhow::Result<OrderBook> {
        let vanta_sym = Self::to_vanta_symbol(symbol)?;
        let data = self
            .public_query(serde_json::json!({
                "type": "orderbook",
                "symbol": vanta_sym,
                "max_level": 100,
            }))
            .await?;
        parse_orderbook_data(&self.venue_id, symbol, &vanta_sym, &data)
    }

    async fn fetch_ticker(&self, symbol: &str) -> anyhow::Result<Ticker> {
        let vanta_sym = Self::to_vanta_symbol(symbol)?;
        let data = self
            .public_query(serde_json::json!({"type": "marketSummary", "symbols": [vanta_sym.clone()]}))
            .await?;
        parse_ticker_data(&self.venue_id, symbol, &vanta_sym, &data)
    }

    async fn fetch_funding_rate(&self, symbol: &str) -> anyhow::Result<FundingRate> {
        let vanta_sym = Self::to_vanta_symbol(symbol)?;
        let data = self
            .public_query(serde_json::json!({"type": "marketSummary", "symbols": [vanta_sym.clone()]}))
            .await?;
        parse_funding_data(&self.venue_id, symbol, &vanta_sym, &data)
    }

    async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
        anyhow::bail!("Live account fetch is not implemented for {}; refusing to fabricate an account snapshot", self.name())
    }

    async fn place_order(&self, _order: OrderRequest) -> anyhow::Result<OrderResponse> {
        anyhow::bail!("Live order execution is not implemented for {}; refusing to fabricate a fill", self.name())
    }

    async fn cancel_order(&self, _req: CancelRequest) -> anyhow::Result<()> {
        anyhow::bail!("Live order cancellation is not implemented for {}", self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn market_row(symbol: &str) -> serde_json::Value {
        serde_json::json!({
            "symbol": symbol,
            "bid_price": "64199.5",
            "ask_price": "64205.0",
            "mark_price": "64202.25",
            "last_funding_rate": "0.00012",
            "est_funding_rate": "0.00015",
            "next_funding_time": 1757779200000i64,
            "min_notional": "10",
            "quote_tick": "0.5",
            "base_tick": "0.0001"
        })
    }

    #[test]
    fn parse_market_summary_entry_accepts_standard_perp() {
        let venue = VenueId::new("vanta");
        let m = market_row("PERP_BTC_USDC");
        let meta = parse_market_summary_entry(&venue, &m).expect("valid standard market");
        assert_eq!(meta.symbol.raw, "BTC-USDC");
        assert_eq!(meta.venue_symbol, "PERP_BTC_USDC");
        assert_eq!(meta.price_decimals, 1);
        assert_eq!(meta.size_decimals, 4);
        assert_eq!(meta.min_order_size, dec!(0.0001));
        assert_eq!(meta.min_notional, dec!(10));
        assert!(meta.is_active);
    }

    #[test]
    fn parse_market_summary_entry_rejects_broker_suffixed_and_malformed() {
        let venue = VenueId::new("vanta");
        assert!(parse_market_summary_entry(&venue, &market_row("PERP_BTC_USDC_BRK")).is_none());
        assert!(parse_market_summary_entry(&venue, &market_row("PERP__USDC")).is_none());
        assert!(parse_market_summary_entry(&venue, &market_row("SPOT_BTC_USDC")).is_none());

        let mut missing_tick = market_row("PERP_ETH_USDC");
        missing_tick.as_object_mut().unwrap().remove("base_tick");
        assert!(parse_market_summary_entry(&venue, &missing_tick).is_none());

        let mut missing_notional = market_row("PERP_ETH_USDC");
        missing_notional.as_object_mut().unwrap().remove("min_notional");
        assert!(parse_market_summary_entry(&venue, &missing_notional).is_none());
    }

    #[test]
    fn parse_orderbook_data_reads_string_levels_and_allows_empty_sides() {
        let venue = VenueId::new("vanta");
        let data = serde_json::json!({
            "symbol": "PERP_BTC_USDC",
            "bids": [{"price": "64200.0", "quantity": "0.5"}],
            "asks": [],
            "ts": 1757779200000i64
        });
        let book = parse_orderbook_data(&venue, "BTC-USDC", "PERP_BTC_USDC", &data)
            .expect("orderbook parses");
        assert_eq!(book.bids.len(), 1);
        assert_eq!(book.bids[0].price, dec!(64200.0));
        assert_eq!(book.bids[0].size, dec!(0.5));
        assert!(book.asks.is_empty());
    }

    #[test]
    fn parse_orderbook_data_fails_closed_on_symbol_mismatch() {
        let venue = VenueId::new("vanta");
        let data = serde_json::json!({
            "symbol": "PERP_ETH_USDC",
            "bids": [{"price": "3000.0", "quantity": "1.0"}],
            "asks": [{"price": "3001.0", "quantity": "1.0"}]
        });
        assert!(parse_orderbook_data(&venue, "BTC-USDC", "PERP_BTC_USDC", &data).is_err());
    }

    #[test]
    fn ticker_and_funding_fail_closed_on_missing_fields() {
        let venue = VenueId::new("vanta");
        let wrap = |row: serde_json::Value| serde_json::json!({ "markets": [row] });

        let good = wrap(market_row("PERP_BTC_USDC"));
        let ticker = parse_ticker_data(&venue, "BTC-USDC", "PERP_BTC_USDC", &good).expect("ticker");
        assert_eq!(ticker.bid, dec!(64199.5));
        assert_eq!(ticker.ask, dec!(64205.0));
        assert_eq!(ticker.last, dec!(64202.25));
        let funding =
            parse_funding_data(&venue, "BTC-USDC", "PERP_BTC_USDC", &good).expect("funding");
        assert_eq!(funding.rate, dec!(0.00012));
        assert_eq!(funding.predicted_rate, Some(dec!(0.00015)));
        assert_eq!(
            funding.next_funding_time,
            DateTime::from_timestamp_millis(1757779200000).unwrap()
        );

        let mut no_bid = market_row("PERP_BTC_USDC");
        no_bid.as_object_mut().unwrap().remove("bid_price");
        assert!(parse_ticker_data(&venue, "BTC-USDC", "PERP_BTC_USDC", &wrap(no_bid)).is_err());

        let mut no_ask = market_row("PERP_BTC_USDC");
        no_ask.as_object_mut().unwrap().remove("ask_price");
        assert!(parse_ticker_data(&venue, "BTC-USDC", "PERP_BTC_USDC", &wrap(no_ask)).is_err());

        let mut no_funding = market_row("PERP_BTC_USDC");
        no_funding.as_object_mut().unwrap().remove("last_funding_rate");
        assert!(
            parse_funding_data(&venue, "BTC-USDC", "PERP_BTC_USDC", &wrap(no_funding)).is_err()
        );

        let mut no_next = market_row("PERP_BTC_USDC");
        no_next.as_object_mut().unwrap().remove("next_funding_time");
        assert!(
            parse_funding_data(&venue, "BTC-USDC", "PERP_BTC_USDC", &wrap(no_next)).is_err()
        );

        let absent = wrap(serde_json::json!({"symbol": "PERP_ETH_USDC"}));
        assert!(parse_ticker_data(&venue, "BTC-USDC", "PERP_BTC_USDC", &absent).is_err());
    }

    #[test]
    fn vanta_symbol_mapping_normalizes_perp_and_rejects_spot() {
        assert_eq!(
            VantaAdapter::to_vanta_symbol("BTC-USDC").unwrap(),
            "PERP_BTC_USDC"
        );
        assert_eq!(
            VantaAdapter::to_vanta_symbol("PERP_ETH_USDC").unwrap(),
            "PERP_ETH_USDC"
        );
        assert!(VantaAdapter::to_vanta_symbol("BTC-SPOT").is_err());
        assert!(VantaAdapter::to_vanta_symbol("eth/spot").is_err());
        assert!(VantaAdapter::to_vanta_symbol("SPOT_BTC_USDC").is_err());
    }
}
