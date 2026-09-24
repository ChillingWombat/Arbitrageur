//! Out-of-band operator alerting for events that must not stay UI-only:
//! circuit-breaker trips and signals escalating to `unresolved`
//! (review §M-9 — an unattended escalation previously reached nobody
//! outside the app window).
//!
//! Channel: `ARBITRAGEUR_ALERT_WEBHOOK=<http(s) URL>` — a POST of a small
//! JSON body per event, compatible with generic webhook receivers
//! (Slack/Discord incoming webhooks, ntfy, Alertmanager webhook_configs,
//! PagerDuty Events endpoints that accept arbitrary JSON). An unset or
//! unparsable URL disables the channel — logged once at startup so the
//! gap is visible in the journal, never per event.
//!
//! Dispatch is fire-and-forget on the tokio runtime: alerting must never
//! block, reorder, or crash a trading path. Delivery failures are logged
//! at warn level — the durable signal ledger stays the source of truth.

use crate::state::TradingEnvironment;
use parking_lot::RwLock;
use std::sync::{Arc, OnceLock};

/// Validated webhook endpoint; `None` = channel disabled.
fn webhook_url() -> Option<&'static str> {
    static URL: OnceLock<Option<String>> = OnceLock::new();
    URL.get_or_init(|| {
        let raw = std::env::var("ARBITRAGEUR_ALERT_WEBHOOK").unwrap_or_default();
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }
        match reqwest::Url::parse(trimmed) {
            Ok(u) if u.scheme() == "https" || u.scheme() == "http" => Some(trimmed.to_string()),
            _ => {
                tracing::warn!(
                    "ARBITRAGEUR_ALERT_WEBHOOK is not a valid http(s) URL — \
                     alert channel disabled"
                );
                None
            }
        }
    })
    .as_deref()
}

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(8))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    })
}

/// Install the circuit-breaker trip hook and report the channel state
/// once. Called during `AppEngineState` construction.
pub fn init(
    cb: &arbitrageur_risk::circuit_breaker::CircuitBreaker,
    environment: Arc<RwLock<TradingEnvironment>>,
) {
    match webhook_url() {
        Some(url) => tracing::info!(
            target: "alerting",
            "operator alert channel configured ({}); breaker trips and unresolved \
             escalations will POST out-of-band",
            url
        ),
        None => tracing::warn!(
            target: "alerting",
            "ARBITRAGEUR_ALERT_WEBHOOK unset — circuit-breaker trips and \
             unresolved escalations stay UI/journal-only. Set it to reach \
             the operator out-of-band (see ops/deploy/README.md)."
        ),
    }
    cb.set_trip_hook(Arc::new(move |reason| {
        let env = environment.read().as_str();
        dispatch(env, "circuit_breaker_trip", reason.to_string());
    }));
}

/// POST one alert event. Never blocks the caller; a missing channel or
/// missing runtime degrades to a log line.
pub fn dispatch(environment: &'static str, event: &'static str, detail: String) {
    let Some(url) = webhook_url() else {
        tracing::warn!(
            target: "alerting",
            "alert suppressed (no webhook configured): {event} [{environment}] {detail}"
        );
        return;
    };
    let url = url.to_string();
    // Keep payloads small for receivers with body limits. truncate() on a
    // non-char-boundary panics — clamp to a boundary first.
    let mut detail = detail;
    if detail.len() > 500 {
        let mut end = 500;
        while !detail.is_char_boundary(end) {
            end -= 1;
        }
        detail.truncate(end);
        detail.push('…');
    }
    let body = serde_json::json!({
        "source": "arbitrageur",
        "event": event,
        "environment": environment,
        "detail": detail,
        "ts": chrono::Utc::now().to_rfc3339(),
    });
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        // No runtime (unit tests, CLI paths) — the alert still reaches
        // the journal, which is the documented fallback channel.
        tracing::warn!(
            target: "alerting",
            "alert (no async runtime — journal only): {event} [{environment}] {detail}"
        );
        return;
    };
    handle.spawn(async move {
        match client().post(&url).json(&body).send().await {
            Ok(resp) if resp.status().is_success() => {}
            Ok(resp) => tracing::warn!(
                target: "alerting",
                "alert webhook rejected ({}) for {event}",
                resp.status()
            ),
            Err(e) => tracing::warn!(
                target: "alerting",
                "alert webhook delivery failed for {event}: {e}"
            ),
        }
    });
}
