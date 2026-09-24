use anyhow::Context;
use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::market::{MarketMetadata, Symbol};
use arbitrageur_core::order::{CancelRequest, OrderRequest, OrderResponse};
use arbitrageur_core::position::AccountSnapshot;
use arbitrageur_core::quote::{BookLevel, FundingRate, OrderBook, Ticker};
use arbitrageur_core::types::{dec, Decimal, VenueId};
use async_trait::async_trait;
use chrono::Utc;
use reqwest::Client;
use serde::Deserialize;
use std::str::FromStr;

/// Polymarket CTF CLOB adapter for binary prediction markets.
///
/// Read-only market data against the public CLOB REST API. `fetch_orderbook`
/// takes a YES-token id as the symbol (token ids are the venue-native asset
/// identifiers); `fetch_markets` lists live sampling markets so callers can
/// discover token ids. No order placement — `place_order` refuses.
pub struct PolymarketAdapter {
    venue_id: VenueId,
    client: Client,
    base_url: String,
    #[allow(dead_code)]
    api_key: Option<String>,
}

#[derive(Deserialize)]
struct PolyBookLevel {
    price: String,
    size: String,
}

#[derive(Deserialize)]
struct PolyBookResponse {
    bids: Vec<PolyBookLevel>,
    asks: Vec<PolyBookLevel>,
}

#[derive(Deserialize)]
struct PolyToken {
    token_id: String,
    outcome: String,
}

#[derive(Deserialize)]
struct PolyMarket {
    question: Option<String>,
    tokens: Vec<PolyToken>,
    active: Option<bool>,
    accepting_orders: Option<bool>,
    minimum_order_size: Option<f64>,
}

#[derive(Deserialize)]
struct PolyMarketsPage {
    data: Vec<PolyMarket>,
    next_cursor: Option<String>,
}

fn parse_poly_level(l: &PolyBookLevel) -> anyhow::Result<BookLevel> {
    let price = Decimal::from_str(&l.price).context("polymarket book level price")?;
    let size = Decimal::from_str(&l.size).context("polymarket book level size")?;
    Ok(BookLevel::new(price, size))
}

/// Sort a parsed book into canonical order (bids desc, asks asc) and drop
/// degenerate levels rather than trusting venue ordering.
fn finish_book(mut book: OrderBook) -> OrderBook {
    book.bids.retain(|l| l.price > dec!(0) && l.size > dec!(0));
    book.asks.retain(|l| l.price > dec!(0) && l.size > dec!(0));
    book.bids.sort_by_key(|l| std::cmp::Reverse(l.price));
    book.asks.sort_by_key(|l| l.price);
    book
}

impl PolymarketAdapter {
    pub fn new(base_url: Option<String>, api_key: Option<String>) -> Self {
        Self {
            venue_id: VenueId::new("polymarket"),
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap_or_default(),
            base_url: base_url.unwrap_or_else(|| "https://clob.polymarket.com".to_string()),
            api_key,
        }
    }
}

#[async_trait]
impl ExchangeAdapter for PolymarketAdapter {
    fn venue_id(&self) -> VenueId {
        self.venue_id.clone()
    }

    fn name(&self) -> &str {
        "Polymarket"
    }

    async fn fetch_markets(&self) -> anyhow::Result<Vec<MarketMetadata>> {
        // Live sampling markets only; cap pages so a hung/large cursor set
        // cannot loop forever.
        let mut out = Vec::new();
        let mut cursor = String::new();
        for _ in 0..3 {
            let url = if cursor.is_empty() {
                format!("{}/sampling-markets", self.base_url)
            } else {
                format!("{}/sampling-markets?next_cursor={cursor}", self.base_url)
            };
            let resp = self
                .client
                .get(&url)
                .send()
                .await
                .context("polymarket sampling-markets request")?;
            let status = resp.status();
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                anyhow::bail!("polymarket sampling-markets HTTP {status}: {body}");
            }
            let page: PolyMarketsPage = resp
                .json()
                .await
                .context("polymarket sampling-markets decode")?;
            for m in page.data {
                if m.active == Some(false) || m.accepting_orders == Some(false) {
                    continue;
                }
                for tok in m.tokens {
                    if !tok.outcome.eq_ignore_ascii_case("yes") {
                        continue;
                    }
                    let label = m.question.clone().unwrap_or_else(|| tok.token_id.clone());
                    let symbol = Symbol::prediction_binary(
                        format!("{}-YES", label.to_uppercase().replace(' ', "-")),
                        "USDC",
                    );
                    out.push(MarketMetadata {
                        symbol,
                        venue: self.venue_id.clone(),
                        venue_symbol: tok.token_id.clone(),
                        price_decimals: 4,
                        size_decimals: 2,
                        min_order_size: Decimal::from_str(
                            &m.minimum_order_size.unwrap_or(5.0).to_string(),
                        )
                        .unwrap_or(dec!(5)),
                        min_notional: dec!(1.0),
                        maker_fee_bps: dec!(0.0),
                        taker_fee_bps: dec!(0.0),
                        contract_size: dec!(1.0),
                        is_active: true,
                    });
                }
            }
            match page.next_cursor.filter(|c| !c.is_empty() && c != "LTE=") {
                Some(c) => cursor = c,
                None => break,
            }
        }
        Ok(out)
    }

    async fn fetch_orderbook(&self, symbol: &str) -> anyhow::Result<OrderBook> {
        // `symbol` is the YES token id — the CLOB's asset identifier.
        let url = format!("{}/book?token_id={}", self.base_url, symbol);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .context("polymarket book request")?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("polymarket book HTTP {status} for token {symbol}: {body}");
        }
        let parsed: PolyBookResponse = resp.json().await.context("polymarket book decode")?;
        let mut book = OrderBook::new(self.venue_id.clone(), symbol.to_string());
        for l in &parsed.bids {
            book.bids.push(parse_poly_level(l)?);
        }
        for l in &parsed.asks {
            book.asks.push(parse_poly_level(l)?);
        }
        book.timestamp = Utc::now();
        Ok(finish_book(book))
    }

    async fn fetch_ticker(&self, symbol: &str) -> anyhow::Result<Ticker> {
        let book = self.fetch_orderbook(symbol).await?;
        let bid = book
            .best_bid_price()
            .ok_or_else(|| anyhow::anyhow!("polymarket book for {symbol} has no bids"))?;
        let ask = book
            .best_ask_price()
            .ok_or_else(|| anyhow::anyhow!("polymarket book for {symbol} has no asks"))?;

        Ok(Ticker {
            venue: self.venue_id.clone(),
            symbol: symbol.to_string(),
            bid,
            ask,
            last: (bid + ask) / dec!(2),
            timestamp: Utc::now(),
        })
    }

    async fn fetch_funding_rate(&self, _symbol: &str) -> anyhow::Result<FundingRate> {
        anyhow::bail!("Prediction markets do not have funding rates")
    }

    async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
        anyhow::bail!("Live account fetch is not implemented for {}; refusing to fabricate an account snapshot", self.name())
    }

    async fn place_order(&self, _order: OrderRequest) -> anyhow::Result<OrderResponse> {
        anyhow::bail!(
            "Live order execution is not implemented for {}; refusing to fabricate a fill",
            self.name()
        )
    }

    async fn cancel_order(&self, _req: CancelRequest) -> anyhow::Result<()> {
        anyhow::bail!(
            "Live order cancellation is not implemented for {}",
            self.name()
        )
    }
}

/// Kalshi CFTC-regulated prediction market exchange adapter.
///
/// Read-only market data against the public v2 REST API. `fetch_orderbook`
/// takes `{ticker}` for the YES-side book or `{ticker}:NO` for the NO-side
/// book (asks are derived from the opposite side's bids, since a YES bid is
/// a NO ask at `1 − price`). No order placement.
pub struct KalshiAdapter {
    venue_id: VenueId,
    client: Client,
    base_url: String,
    #[allow(dead_code)]
    api_key: Option<String>,
}

#[derive(Deserialize)]
struct KalshiOrderbookResponse {
    orderbook: Option<KalshiOrderbook>,
}

#[derive(Deserialize)]
struct KalshiOrderbook {
    /// YES bids as `[price_cents, contract_count]` (deprecated shape) and/or
    /// dollar-denominated `[price_str, count_str]` arrays.
    yes: Option<Vec<Vec<serde_json::Value>>>,
    no: Option<Vec<Vec<serde_json::Value>>>,
    yes_dollars: Option<Vec<Vec<serde_json::Value>>>,
    no_dollars: Option<Vec<Vec<serde_json::Value>>>,
}

#[derive(Deserialize)]
struct KalshiMarket {
    ticker: String,
    title: Option<String>,
    status: Option<String>,
}

#[derive(Deserialize)]
struct KalshiMarketsPage {
    markets: Vec<KalshiMarket>,
    cursor: Option<String>,
}

fn kalshi_level_price(v: &serde_json::Value, dollar_mode: bool) -> Option<Decimal> {
    match v {
        serde_json::Value::Number(n) => {
            let raw = n.as_f64()?;
            let px = if dollar_mode { raw } else { raw / 100.0 };
            Decimal::from_str(&px.to_string()).ok()
        }
        serde_json::Value::String(s) => {
            let d = Decimal::from_str(s).ok()?;
            if dollar_mode {
                Some(d)
            } else {
                Some(d / dec!(100))
            }
        }
        _ => None,
    }
}

fn kalshi_level_qty(v: &serde_json::Value) -> Option<Decimal> {
    match v {
        serde_json::Value::Number(n) => Decimal::from_str(&n.as_f64()?.to_string()).ok(),
        serde_json::Value::String(s) => Decimal::from_str(s).ok(),
        _ => None,
    }
}

fn parse_kalshi_levels(
    levels: &[Vec<serde_json::Value>],
    dollar_mode: bool,
) -> Vec<(Decimal, Decimal)> {
    levels
        .iter()
        .filter_map(|lv| {
            let px = kalshi_level_price(lv.first()?, dollar_mode)?;
            let qty = kalshi_level_qty(lv.get(1)?)?;
            Some((px, qty))
        })
        .collect()
}

impl KalshiAdapter {
    pub fn new(base_url: Option<String>, api_key: Option<String>) -> Self {
        Self {
            venue_id: VenueId::new("kalshi"),
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap_or_default(),
            base_url: base_url.unwrap_or_else(|| "https://api.elections.kalshi.com".to_string()),
            api_key,
        }
    }
}

#[async_trait]
impl ExchangeAdapter for KalshiAdapter {
    fn venue_id(&self) -> VenueId {
        self.venue_id.clone()
    }

    fn name(&self) -> &str {
        "Kalshi"
    }

    async fn fetch_markets(&self) -> anyhow::Result<Vec<MarketMetadata>> {
        let mut out = Vec::new();
        let mut cursor = String::new();
        for _ in 0..3 {
            let url = if cursor.is_empty() {
                format!(
                    "{}/trade-api/v2/markets?status=open&limit=200",
                    self.base_url
                )
            } else {
                format!(
                    "{}/trade-api/v2/markets?status=open&limit=200&cursor={cursor}",
                    self.base_url
                )
            };
            let resp = self
                .client
                .get(&url)
                .send()
                .await
                .context("kalshi markets request")?;
            let status = resp.status();
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                anyhow::bail!("kalshi markets HTTP {status}: {body}");
            }
            let page: KalshiMarketsPage = resp.json().await.context("kalshi markets decode")?;
            for m in page.markets {
                if m.status.as_deref() == Some("closed") || m.status.as_deref() == Some("settled") {
                    continue;
                }
                let label = m.title.clone().unwrap_or_else(|| m.ticker.clone());
                let symbol = Symbol::prediction_binary(
                    format!("{}-YES", label.to_uppercase().replace(' ', "-")),
                    "USD",
                );
                out.push(MarketMetadata {
                    symbol,
                    venue: self.venue_id.clone(),
                    venue_symbol: m.ticker.clone(),
                    price_decimals: 4,
                    size_decimals: 0,
                    min_order_size: dec!(1.0),
                    min_notional: dec!(1.0),
                    maker_fee_bps: dec!(0.0),
                    taker_fee_bps: dec!(7.0),
                    contract_size: dec!(1.0),
                    is_active: true,
                });
            }
            match page.cursor.filter(|c| !c.is_empty()) {
                Some(c) => cursor = c,
                None => break,
            }
        }
        Ok(out)
    }

    async fn fetch_orderbook(&self, symbol: &str) -> anyhow::Result<OrderBook> {
        // `{ticker}` → YES book; `{ticker}:NO` → NO book.
        let (ticker, want_no) = match symbol.rsplit_once(':') {
            Some((t, side)) if side.eq_ignore_ascii_case("no") => (t, true),
            Some((t, side)) if side.eq_ignore_ascii_case("yes") => (t, false),
            _ => (symbol, false),
        };
        if ticker.trim().is_empty() {
            anyhow::bail!("kalshi orderbook requires a market ticker, got {symbol:?}");
        }
        let url = format!(
            "{}/trade-api/v2/markets/{}/orderbook",
            self.base_url, ticker
        );
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .context("kalshi orderbook request")?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("kalshi orderbook HTTP {status} for {ticker}: {body}");
        }
        let parsed: KalshiOrderbookResponse =
            resp.json().await.context("kalshi orderbook decode")?;
        let ob = parsed
            .orderbook
            .ok_or_else(|| anyhow::anyhow!("kalshi orderbook for {ticker} returned no book"))?;

        // Prefer dollar-denominated arrays; fall back to cent-integer arrays.
        let (dollar_mode, yes_raw, no_raw) = match (&ob.yes_dollars, &ob.no_dollars) {
            (Some(y), Some(n)) => (true, y.clone(), n.clone()),
            _ => (
                false,
                ob.yes.clone().unwrap_or_default(),
                ob.no.clone().unwrap_or_default(),
            ),
        };
        let yes_bids = parse_kalshi_levels(&yes_raw, dollar_mode);
        let no_bids = parse_kalshi_levels(&no_raw, dollar_mode);

        let mut book = OrderBook::new(self.venue_id.clone(), symbol.to_string());
        if want_no {
            // NO book: bids are the venue's NO bids; NO asks are YES bids
            // complemented (a YES bid at p is a NO ask at 1 − p).
            for (px, qty) in no_bids {
                book.bids.push(BookLevel::new(px, qty));
            }
            for (px, qty) in yes_bids {
                let ask = dec!(1) - px;
                if ask > dec!(0) {
                    book.asks.push(BookLevel::new(ask, qty));
                }
            }
        } else {
            for (px, qty) in yes_bids {
                book.bids.push(BookLevel::new(px, qty));
            }
            for (px, qty) in no_bids {
                let ask = dec!(1) - px;
                if ask > dec!(0) {
                    book.asks.push(BookLevel::new(ask, qty));
                }
            }
        }
        book.timestamp = Utc::now();
        Ok(finish_book(book))
    }

    async fn fetch_ticker(&self, symbol: &str) -> anyhow::Result<Ticker> {
        let book = self.fetch_orderbook(symbol).await?;
        let bid = book
            .best_bid_price()
            .ok_or_else(|| anyhow::anyhow!("kalshi book for {symbol} has no bids"))?;
        let ask = book
            .best_ask_price()
            .ok_or_else(|| anyhow::anyhow!("kalshi book for {symbol} has no asks"))?;

        Ok(Ticker {
            venue: self.venue_id.clone(),
            symbol: symbol.to_string(),
            bid,
            ask,
            last: (bid + ask) / dec!(2),
            timestamp: Utc::now(),
        })
    }

    async fn fetch_funding_rate(&self, _symbol: &str) -> anyhow::Result<FundingRate> {
        anyhow::bail!("Kalshi does not have perpetual funding rates")
    }

    async fn fetch_account(&self) -> anyhow::Result<AccountSnapshot> {
        anyhow::bail!("Live account fetch is not implemented for {}; refusing to fabricate an account snapshot", self.name())
    }

    async fn place_order(&self, _order: OrderRequest) -> anyhow::Result<OrderResponse> {
        anyhow::bail!(
            "Live order execution is not implemented for {}; refusing to fabricate a fill",
            self.name()
        )
    }

    async fn cancel_order(&self, _req: CancelRequest) -> anyhow::Result<()> {
        anyhow::bail!(
            "Live order cancellation is not implemented for {}",
            self.name()
        )
    }
}

#[cfg(test)]
mod tests {
    /// CI guard (issue #38): no production `fetch_orderbook` in this module may
    /// return a book without an HTTP call — forbid hardcoded decimal levels.
    #[test]
    fn no_fabricated_book_levels_in_production() {
        let src = include_str!("prediction.rs");
        let production = src.split("#[cfg(test)]").next().unwrap_or(src);
        assert!(
            !production.contains("BookLevel::new(dec!"),
            "prediction adapters must fetch real books, never fabricate levels"
        );
    }
}
