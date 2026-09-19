//! Autonomous convergence executor policy — Spread Radar bot (GitHub issue #5).
//!
//! Pure decision functions for the opt-in paper-only bot that consumes the
//! spread monitor's qualified-opportunity stream. No I/O, no engine state,
//! no time reads: every input arrives as an argument so policy is
//! deterministic and unit-testable in isolation.
//!
//! Contract notes (paper-only, fail-closed):
//! - The monitor remains the sole authority on qualification. These functions
//!   never re-derive P_win, convergence time, or capacity; they consume them
//!   and refuse to trade when any required field is missing.
//! - Sizing prices depth-decay via the row's `optimal_size_usd` and never
//!   walks the book past the marginal `breakeven_size_usd` bound.
//! - The authoritative delta/margin/slippage/depth gates stay inside the
//!   guarded paper execution path. The pre-gate here is structural (an
//!   equal-quantity long/short pair nets to zero by DeltaGuard arithmetic);
//!   any live breach still fails closed inside execution and is audited.
//! - Stop evaluation uses an entry-relative band (`stop_mult × |entry_dev|`,
//!   floored at the friction band) because the monitor row DTO does not carry
//!   the OU σ the stop band was conditioned on. This is the documented
//!   paper-only approximation; it never overlaps the convergence band.

use serde::{Deserialize, Serialize};

/// Plain-data view of one monitor row, mapped 1:1 from `MonitorRowDto`.
/// The executor builds this; policy never touches engine types.
#[derive(Debug, Clone, PartialEq)]
pub struct ConvergenceSignal {
    pub symbol: String,
    pub buy_venue: String,
    pub sell_venue: String,
    /// Monitor lifecycle status: only "ready" is tradable.
    pub status: String,
    /// The monitor's own execute gate (paper path, books, session, …).
    pub executable: bool,
    pub current_edge_bps: Option<f64>,
    pub baseline_bps: Option<f64>,
    pub deviation_bps: Option<f64>,
    pub net_capture_bps: Option<f64>,
    pub p_win: Option<f64>,
    pub expected_profit_bps: Option<f64>,
    pub expected_profit_usd: Option<f64>,
    pub optimal_size_usd: Option<f64>,
    pub breakeven_size_usd: Option<f64>,
    pub funding_unknown: bool,
    pub expected_converge_secs: Option<f64>,
}

impl ConvergenceSignal {
    /// Minimal constructor for tests and the executor mapping layer.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        symbol: &str,
        buy_venue: &str,
        sell_venue: &str,
        status: &str,
        executable: bool,
    ) -> Self {
        Self {
            symbol: symbol.to_string(),
            buy_venue: buy_venue.to_string(),
            sell_venue: sell_venue.to_string(),
            status: status.to_string(),
            executable,
            current_edge_bps: None,
            baseline_bps: None,
            deviation_bps: None,
            net_capture_bps: None,
            p_win: None,
            expected_profit_bps: None,
            expected_profit_usd: None,
            optimal_size_usd: None,
            breakeven_size_usd: None,
            funding_unknown: false,
            expected_converge_secs: None,
        }
    }
}

/// Bot knobs. Persisted as one JSON row (`radar_bot_config`); `enabled` and
/// every `mode` other than full-auto default to inert. Ship conservative.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AutoExecConfig {
    /// Master opt-in. Default OFF: nothing trades until armed explicitly.
    pub enabled: bool,
    /// Executor poll cadence, seconds.
    pub poll_interval_secs: u64,
    /// Minimum win probability for entries (≥ monitor display floor).
    pub min_p_win: f64,
    /// Minimum expected profit (net × P_win) for entries, bps.
    pub min_expected_profit_bps: f64,
    /// Max simultaneously managed convergence positions.
    pub max_concurrent_positions: usize,
    /// Max notional per entry, USD.
    pub max_notional_per_position_usd: f64,
    /// Max open notional summed per venue leg, USD.
    pub max_notional_per_venue_usd: f64,
    /// Max open notional per ticker, USD.
    pub max_notional_per_symbol_usd: f64,
    /// Per-pair re-entry blackout after an exit/stop, seconds.
    pub pair_cooldown_secs: u64,
    /// Max-hold deadline = entry + mult × expected_converge_secs.
    pub hold_timeout_mult: f64,
    /// Stop band = mult × |entry deviation|, floored at the friction band.
    pub stop_mult: f64,
    /// Unwatched (stale/missing row) grace before reduce-only unwind, seconds.
    pub stale_unwind_secs: u64,
}

impl Default for AutoExecConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            poll_interval_secs: 5,
            min_p_win: 0.93,
            min_expected_profit_bps: 2.0,
            max_concurrent_positions: 3,
            max_notional_per_position_usd: 1000.0,
            max_notional_per_venue_usd: 3000.0,
            max_notional_per_symbol_usd: 2000.0,
            pair_cooldown_secs: 900,
            hold_timeout_mult: 3.0,
            stop_mult: 1.5,
            stale_unwind_secs: 600,
        }
    }
}

impl AutoExecConfig {
    /// Fail-closed validation: any out-of-range knob rejects the whole config
    /// so a corrupt payload can never widen risk.
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=300).contains(&self.poll_interval_secs) {
            return Err(format!(
                "poll_interval_secs {} out of range 1..=300",
                self.poll_interval_secs
            ));
        }
        if !(0.5..=0.999).contains(&self.min_p_win) || !self.min_p_win.is_finite() {
            return Err(format!(
                "min_p_win {} out of range 0.5..=0.999",
                self.min_p_win
            ));
        }
        if !self.min_expected_profit_bps.is_finite()
            || !(0.0..=10_000.0).contains(&self.min_expected_profit_bps)
        {
            return Err(format!(
                "min_expected_profit_bps {} out of range 0..=10000",
                self.min_expected_profit_bps
            ));
        }
        if self.max_concurrent_positions == 0 || self.max_concurrent_positions > 32 {
            return Err(format!(
                "max_concurrent_positions {} out of range 1..=32",
                self.max_concurrent_positions
            ));
        }
        if !self.max_notional_per_position_usd.is_finite()
            || !(10.0..=1_000_000.0).contains(&self.max_notional_per_position_usd)
        {
            return Err(format!(
                "max_notional_per_position_usd {} out of range 10..=1000000",
                self.max_notional_per_position_usd
            ));
        }
        if !self.max_notional_per_venue_usd.is_finite()
            || self.max_notional_per_venue_usd < self.max_notional_per_position_usd
            || self.max_notional_per_venue_usd > 100_000_000.0
        {
            return Err(format!(
                "max_notional_per_venue_usd {} must cover one position and stay <= 1e8",
                self.max_notional_per_venue_usd
            ));
        }
        if !self.max_notional_per_symbol_usd.is_finite()
            || self.max_notional_per_symbol_usd < self.max_notional_per_position_usd
            || self.max_notional_per_symbol_usd > 100_000_000.0
        {
            return Err(format!(
                "max_notional_per_symbol_usd {} must cover one position and stay <= 1e8",
                self.max_notional_per_symbol_usd
            ));
        }
        if self.pair_cooldown_secs > 86_400 {
            return Err(format!(
                "pair_cooldown_secs {} out of range 0..=86400",
                self.pair_cooldown_secs
            ));
        }
        if !self.hold_timeout_mult.is_finite()
            || !(1.0..=24.0).contains(&self.hold_timeout_mult)
        {
            return Err(format!(
                "hold_timeout_mult {} out of range 1..=24",
                self.hold_timeout_mult
            ));
        }
        if !self.stop_mult.is_finite() || !(1.0..=10.0).contains(&self.stop_mult) {
            return Err(format!(
                "stop_mult {} out of range 1..=10",
                self.stop_mult
            ));
        }
        if !(60..=86_400).contains(&self.stale_unwind_secs) {
            return Err(format!(
                "stale_unwind_secs {} out of range 60..=86400",
                self.stale_unwind_secs
            ));
        }
        Ok(())
    }
}

/// Bot operating mode (persisted alongside the config).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotMode {
    /// Fully inert: no entries, no exits driven (existing ledger untouched).
    Off,
    /// Entries + exits.
    Auto,
    /// Exits only; entries frozen, watchers live.
    Paused,
    /// Exits only; like Paused but signals wind-down intent in the UI.
    ReduceOnly,
}

impl BotMode {
    pub fn parse_mode(raw: &str) -> Option<Self> {
        match raw {
            "off" => Some(Self::Off),
            "auto" => Some(Self::Auto),
            "paused" => Some(Self::Paused),
            "reduce_only" => Some(Self::ReduceOnly),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Auto => "auto",
            Self::Paused => "paused",
            Self::ReduceOnly => "reduce_only",
        }
    }

    /// New entries only in full-auto.
    pub fn entries_allowed(&self) -> bool {
        matches!(self, Self::Auto)
    }

    /// Exits/watchers stay live whenever the bot is armed at all.
    pub fn exits_allowed(&self) -> bool {
        matches!(self, Self::Auto | Self::Paused | Self::ReduceOnly)
    }
}

/// Snapshot of everything one entry decision depends on. The executor fills
/// this from engine state; policy only reads it.
#[derive(Debug, Clone)]
pub struct GateContext {
    pub live_mode: bool,
    pub engine_paused: bool,
    pub monitor_enabled: bool,
    pub snapshot_present: bool,
    pub circuit_ok: bool,
    pub circuit_reason: Option<String>,
    /// Currently managed open convergence positions.
    pub open_positions: usize,
    /// Open bot notional already resting on the candidate's buy venue, USD.
    pub buy_venue_notional_usd: f64,
    /// Open bot notional already resting on the candidate's sell venue, USD.
    pub sell_venue_notional_usd: f64,
    /// Open bot notional already resting on the candidate's symbol, USD.
    pub symbol_notional_usd: f64,
    /// Candidate entry size, USD.
    pub size_usd: f64,
    /// Remaining per-pair cooldown, seconds. `Some(>0)` blocks re-entry.
    pub cooldown_remaining_secs: Option<f64>,
    /// An in-flight/imbalanced/unresolved ledger signal blocks the symbol.
    pub blocking_signal: bool,
}

impl GateContext {
    /// Test/idle context: everything clear, zero exposure.
    pub fn open(size_usd: f64) -> Self {
        Self {
            live_mode: false,
            engine_paused: false,
            monitor_enabled: true,
            snapshot_present: true,
            circuit_ok: true,
            circuit_reason: None,
            open_positions: 0,
            buy_venue_notional_usd: 0.0,
            sell_venue_notional_usd: 0.0,
            symbol_notional_usd: 0.0,
            size_usd,
            cooldown_remaining_secs: None,
            blocking_signal: false,
        }
    }
}

/// One gate verdict. `reason` is always populated — "ok" when allowed — so
/// the audit trail never carries an empty decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateDecision {
    pub allow: bool,
    pub reason: String,
}

fn deny(reason: impl Into<String>) -> GateDecision {
    GateDecision {
        allow: false,
        reason: reason.into(),
    }
}

fn allow() -> GateDecision {
    GateDecision {
        allow: true,
        reason: "ok".to_string(),
    }
}

/// Ordered entry gate pipeline. First denial wins so the audit reason names
/// the binding constraint. Checks run cheapest-first; risk gates that need
/// no market data precede notional accounting.
pub fn evaluate_entry_gates(
    signal: &ConvergenceSignal,
    cfg: &AutoExecConfig,
    ctx: &GateContext,
) -> GateDecision {
    if !cfg.enabled {
        return deny("bot disabled");
    }
    // Live-mode hard block: the bot is paper-only, unconditionally.
    if ctx.live_mode {
        return deny("live mode: bot is paper-only");
    }
    if ctx.engine_paused {
        return deny("engine paused");
    }
    if !ctx.monitor_enabled {
        return deny("monitor disabled");
    }
    if !ctx.snapshot_present {
        return deny("monitor snapshot unavailable");
    }
    // Single consumption seam: only ready + executable rows.
    if signal.status != "ready" || !signal.executable {
        return deny("signal not ready");
    }
    match signal.p_win {
        Some(p) if p.is_finite() && p >= cfg.min_p_win => {}
        Some(_) => {
            return deny(format!(
                "win probability below {:.0}% floor",
                cfg.min_p_win * 100.0
            ))
        }
        None => return deny("win probability unknown"),
    }
    match signal.expected_profit_bps {
        Some(e) if e.is_finite() && e >= cfg.min_expected_profit_bps => {}
        Some(_) => {
            return deny(format!(
                "expected profit below {:.1} bps hurdle",
                cfg.min_expected_profit_bps
            ))
        }
        None => return deny("expected profit unknown"),
    }
    match signal.expected_converge_secs {
        Some(t) if t.is_finite() && t > 0.0 => {}
        _ => return deny("expected convergence time unknown"),
    }
    match signal.optimal_size_usd {
        Some(s) if s.is_finite() && s > 0.0 => {}
        _ => return deny("no profitable capacity"),
    }
    if signal.funding_unknown {
        return deny("funding adjustment unknown");
    }
    if !ctx.circuit_ok {
        return deny(format!(
            "circuit breaker: {}",
            ctx.circuit_reason.as_deref().unwrap_or("tripped")
        ));
    }
    if ctx.open_positions >= cfg.max_concurrent_positions {
        return deny(format!(
            "max concurrent positions ({}) reached",
            cfg.max_concurrent_positions
        ));
    }
    if !ctx.size_usd.is_finite() || ctx.size_usd <= 0.0 {
        return deny("candidate size not positive");
    }
    if ctx.buy_venue_notional_usd + ctx.size_usd > cfg.max_notional_per_venue_usd {
        return deny(format!(
            "venue cap: {} exposure would exceed ${:.0}",
            signal.buy_venue, cfg.max_notional_per_venue_usd
        ));
    }
    if ctx.sell_venue_notional_usd + ctx.size_usd > cfg.max_notional_per_venue_usd {
        return deny(format!(
            "venue cap: {} exposure would exceed ${:.0}",
            signal.sell_venue, cfg.max_notional_per_venue_usd
        ));
    }
    if ctx.symbol_notional_usd + ctx.size_usd > cfg.max_notional_per_symbol_usd {
        return deny(format!(
            "symbol cap: {} exposure would exceed ${:.0}",
            signal.symbol, cfg.max_notional_per_symbol_usd
        ));
    }
    if let Some(rem) = ctx.cooldown_remaining_secs {
        if rem.is_finite() && rem > 0.0 {
            return deny(format!("pair cooling down ({:.0}s left)", rem));
        }
    }
    if ctx.blocking_signal {
        return deny("unresolved signal on this symbol");
    }
    allow()
}

/// Entry sizing from the capacity ladder: the row's `optimal_size_usd`
/// (depth-decayed, participation- and session-sized by the monitor), capped
/// by the marginal breakeven bound and the per-position risk cap. Returns
/// `None` (fail-closed) when the row carries no usable capacity.
pub fn size_position(signal: &ConvergenceSignal, cfg: &AutoExecConfig) -> Option<f64> {
    let optimal = signal.optimal_size_usd?;
    if !optimal.is_finite() || optimal <= 0.0 {
        return None;
    }
    let mut size = optimal;
    if let Some(be) = signal.breakeven_size_usd {
        if be.is_finite() && be > 0.0 && be < size {
            size = be;
        }
    }
    if size > cfg.max_notional_per_position_usd {
        size = cfg.max_notional_per_position_usd;
    }
    if !size.is_finite() || size <= 0.0 {
        return None;
    }
    Some(size)
}

/// Friction band implied by the entry row: deviation minus net capture.
/// The win condition is the spread reverting inside this band around
/// baseline. `None` when the row cannot price its own friction.
pub fn friction_band_bps(deviation_bps: Option<f64>, net_capture_bps: Option<f64>) -> Option<f64> {
    let (d, n) = (deviation_bps?, net_capture_bps?);
    if !d.is_finite() || !n.is_finite() {
        return None;
    }
    let band = d.abs() - n.abs();
    if !band.is_finite() || band < 0.0 {
        return None;
    }
    Some(band)
}

/// Max-hold deadline for a position entered now.
pub fn hold_deadline_ms(
    entry_ts_ms: i64,
    expected_converge_secs: Option<f64>,
    cfg: &AutoExecConfig,
) -> Option<i64> {
    let t = expected_converge_secs?;
    if !t.is_finite() || t <= 0.0 {
        return None;
    }
    let hold_ms = (t * cfg.hold_timeout_mult * 1000.0).round();
    if !hold_ms.is_finite() || hold_ms <= 0.0 || hold_ms > i64::MAX as f64 {
        return None;
    }
    entry_ts_ms.checked_add(hold_ms as i64)
}

/// Everything an entry needs, derived together so a row that cannot price
/// size, friction, or deadline is refused with one reason.
#[derive(Debug, Clone)]
pub struct EntryPlan {
    pub size_usd: f64,
    pub friction_band_bps: f64,
    pub deadline_ms: i64,
}

pub fn plan_entry(
    signal: &ConvergenceSignal,
    cfg: &AutoExecConfig,
    entry_ts_ms: i64,
) -> Result<EntryPlan, String> {
    let size_usd = size_position(signal, cfg).ok_or_else(|| "no usable capacity".to_string())?;
    let friction_band_bps =
        friction_band_bps(signal.deviation_bps, signal.net_capture_bps)
            .ok_or_else(|| "cannot derive friction band".to_string())?;
    let deadline_ms = hold_deadline_ms(entry_ts_ms, signal.expected_converge_secs, cfg)
        .ok_or_else(|| "cannot derive hold deadline".to_string())?;
    Ok(EntryPlan {
        size_usd,
        friction_band_bps,
        deadline_ms,
    })
}

/// Live spread observed for an open position's pair on this tick.
#[derive(Debug, Clone)]
pub struct LiveSpreadView {
    /// Monitor lifecycle status of the pair ("ready", "no-data", …).
    pub status: String,
    pub deviation_bps: Option<f64>,
}

/// Exit verdict for one open position on one tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExitEval {
    /// Watched, no action.
    Hold,
    /// Row missing/stale/deviation unknown: hold but flag unwatched. Past
    /// the grace window the executor unwinds reduce-only instead.
    Unwatched { reason: String },
    /// Unwind now through the existing flatten path.
    Exit { trigger: ExitTrigger, reason: String },
}

/// Why a position is being unwound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitTrigger {
    /// Spread reverted into the friction band around baseline (the win).
    Converged,
    /// Deviation crossed the stop band (the modeled loss bound).
    Stopped,
    /// Max-hold deadline elapsed with neither convergence nor stop.
    TimedOut,
    /// Row stale/missing past the grace window: reduce-only unwind.
    StaleUnwind,
}

impl ExitTrigger {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Converged => "converged",
            Self::Stopped => "stopped",
            Self::TimedOut => "timed_out",
            Self::StaleUnwind => "stale_unwind",
        }
    }
}

/// Exit-condition evaluation over the entry snapshot and the live row.
/// Band edges are inclusive and disjoint by construction (the stop threshold
/// is floored at the friction band), so converged and stopped can never fire
/// on the same tick. Order: convergence, stop, timeout.
pub fn evaluate_exit(
    entry_dev_bps: f64,
    friction_band_bps: f64,
    deadline_ms: i64,
    unwatched_for_secs: Option<f64>,
    live: Option<&LiveSpreadView>,
    now_ms: i64,
    cfg: &AutoExecConfig,
) -> ExitEval {
    let stale_grace = cfg.stale_unwind_secs as f64;
    let stale_exit = |why: &str| {
        if unwatched_for_secs.unwrap_or(0.0) >= stale_grace {
            ExitEval::Exit {
                trigger: ExitTrigger::StaleUnwind,
                reason: why.to_string(),
            }
        } else {
            ExitEval::Unwatched {
                reason: why.to_string(),
            }
        }
    };
    let Some(row) = live else {
        return stale_exit("pair no longer tracked");
    };
    if row.status == "no-data" {
        return stale_exit("pair has no data");
    }
    let Some(dev) = row.deviation_bps.filter(|d| d.is_finite()) else {
        return stale_exit("live deviation unknown");
    };
    if dev.abs() <= friction_band_bps {
        return ExitEval::Exit {
            trigger: ExitTrigger::Converged,
            reason: format!(
                "spread reverted into friction band (|dev| {dev:.1} <= {friction_band_bps:.1} bps)"
            ),
        };
    }
    let stop_at = (cfg.stop_mult * entry_dev_bps.abs()).max(friction_band_bps);
    if dev.abs() >= stop_at {
        return ExitEval::Exit {
            trigger: ExitTrigger::Stopped,
            reason: format!(
                "deviation crossed stop band (|dev| {dev:.1} >= {stop_at:.1} bps)"
            ),
        };
    }
    if now_ms >= deadline_ms {
        return ExitEval::Exit {
            trigger: ExitTrigger::TimedOut,
            reason: "max-hold deadline elapsed".to_string(),
        };
    }
    ExitEval::Hold
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready_signal() -> ConvergenceSignal {
        let mut s = ConvergenceSignal::new("BTC", "hyperliquid", "lighter", "ready", true);
        s.current_edge_bps = Some(12.0);
        s.baseline_bps = Some(2.0);
        s.deviation_bps = Some(10.0);
        s.net_capture_bps = Some(6.0);
        s.p_win = Some(0.95);
        s.expected_profit_bps = Some(5.7);
        s.expected_profit_usd = Some(57.0);
        s.optimal_size_usd = Some(10_000.0);
        s.breakeven_size_usd = Some(8_000.0);
        s.expected_converge_secs = Some(120.0);
        s
    }

    fn open_ctx(size: f64) -> (AutoExecConfig, GateContext) {
        let cfg = AutoExecConfig {
            enabled: true,
            ..AutoExecConfig::default()
        };
        (cfg.clone(), GateContext::open(size))
    }

    #[test]
    fn entry_on_qualified_row_is_allowed() {
        let s = ready_signal();
        let (cfg, ctx) = open_ctx(1000.0);
        let d = evaluate_entry_gates(&s, &cfg, &ctx);
        assert!(d.allow, "expected allow, got {}", d.reason);
        assert_eq!(d.reason, "ok");
    }

    #[test]
    fn gate_bot_disabled_first() {
        let s = ready_signal();
        let (mut cfg, ctx) = open_ctx(1000.0);
        cfg.enabled = false;
        let d = evaluate_entry_gates(&s, &cfg, &ctx);
        assert!(!d.allow);
        assert_eq!(d.reason, "bot disabled");
    }

    #[test]
    fn gate_live_mode_hard_blocks() {
        let s = ready_signal();
        let (cfg, mut ctx) = open_ctx(1000.0);
        ctx.live_mode = true;
        let d = evaluate_entry_gates(&s, &cfg, &ctx);
        assert!(!d.allow);
        assert!(d.reason.contains("paper-only"));
    }

    #[test]
    fn gate_engine_pause_blocks() {
        let s = ready_signal();
        let (cfg, mut ctx) = open_ctx(1000.0);
        ctx.engine_paused = true;
        assert_eq!(
            evaluate_entry_gates(&s, &cfg, &ctx),
            GateDecision {
                allow: false,
                reason: "engine paused".into()
            }
        );
    }

    #[test]
    fn gate_monitor_disabled_and_missing_snapshot() {
        let s = ready_signal();
        let (cfg, mut ctx) = open_ctx(1000.0);
        ctx.monitor_enabled = false;
        assert!(!evaluate_entry_gates(&s, &cfg, &ctx).allow);
        ctx.monitor_enabled = true;
        ctx.snapshot_present = false;
        let d = evaluate_entry_gates(&s, &cfg, &ctx);
        assert!(!d.allow);
        assert_eq!(d.reason, "monitor snapshot unavailable");
    }

    #[test]
    fn gate_rejects_non_ready_statuses() {
        let (cfg, ctx) = open_ctx(1000.0);
        for status in ["preparing", "tracking", "triggered", "no-data"] {
            let mut s = ready_signal();
            s.status = status.into();
            let d = evaluate_entry_gates(&s, &cfg, &ctx);
            assert!(!d.allow, "status {status} must not trade");
            assert_eq!(d.reason, "signal not ready");
        }
        let mut s = ready_signal();
        s.executable = false;
        assert!(!evaluate_entry_gates(&s, &cfg, &ctx).allow);
    }

    #[test]
    fn gate_p_win_floor_and_unknown() {
        let (mut cfg, ctx) = open_ctx(1000.0);
        cfg.min_p_win = 0.93;
        let mut low = ready_signal();
        low.p_win = Some(0.90);
        let d = evaluate_entry_gates(&low, &cfg, &ctx);
        assert!(!d.allow);
        assert!(d.reason.contains("93% floor"));
        let mut missing = ready_signal();
        missing.p_win = None;
        assert_eq!(
            evaluate_entry_gates(&missing, &cfg, &ctx).reason,
            "win probability unknown"
        );
        // NaN never trades.
        let mut nan = ready_signal();
        nan.p_win = Some(f64::NAN);
        assert!(!evaluate_entry_gates(&nan, &cfg, &ctx).allow);
    }

    #[test]
    fn gate_expected_profit_hurdle_and_unknown() {
        let (mut cfg, ctx) = open_ctx(1000.0);
        cfg.min_expected_profit_bps = 6.0;
        let mut s = ready_signal();
        s.expected_profit_bps = Some(5.7);
        assert!(!evaluate_entry_gates(&s, &cfg, &ctx).allow);
        s.expected_profit_bps = None;
        assert_eq!(
            evaluate_entry_gates(&s, &cfg, &ctx).reason,
            "expected profit unknown"
        );
    }

    #[test]
    fn gate_requires_converge_time_and_capacity() {
        let (cfg, ctx) = open_ctx(1000.0);
        let mut s = ready_signal();
        s.expected_converge_secs = None;
        assert_eq!(
            evaluate_entry_gates(&s, &cfg, &ctx).reason,
            "expected convergence time unknown"
        );
        let mut s = ready_signal();
        s.expected_converge_secs = Some(0.0);
        assert!(!evaluate_entry_gates(&s, &cfg, &ctx).allow);
        let mut s = ready_signal();
        s.optimal_size_usd = None;
        assert_eq!(
            evaluate_entry_gates(&s, &cfg, &ctx).reason,
            "no profitable capacity"
        );
    }

    #[test]
    fn gate_rejects_funding_unknown() {
        let (cfg, ctx) = open_ctx(1000.0);
        let mut s = ready_signal();
        s.funding_unknown = true;
        assert_eq!(
            evaluate_entry_gates(&s, &cfg, &ctx).reason,
            "funding adjustment unknown"
        );
    }

    #[test]
    fn gate_circuit_breaker_names_reason() {
        let (cfg, mut ctx) = open_ctx(1000.0);
        ctx.circuit_ok = false;
        ctx.circuit_reason = Some("drawdown".into());
        let d = evaluate_entry_gates(&ready_signal(), &cfg, &ctx);
        assert!(!d.allow);
        assert!(d.reason.contains("drawdown"));
    }

    #[test]
    fn gate_concurrent_position_cap() {
        let (mut cfg, ctx_base) = open_ctx(100.0);
        cfg.max_concurrent_positions = 2;
        let mut ctx = ctx_base;
        ctx.open_positions = 2;
        let d = evaluate_entry_gates(&ready_signal(), &cfg, &ctx);
        assert!(!d.allow);
        assert!(d.reason.contains("max concurrent positions (2)"));
        ctx.open_positions = 1;
        assert!(evaluate_entry_gates(&ready_signal(), &cfg, &ctx).allow);
    }

    #[test]
    fn gate_per_venue_notional_caps() {
        let (mut cfg, _) = open_ctx(0.0);
        cfg.max_notional_per_venue_usd = 3000.0;
        let mut ctx = GateContext::open(500.0);
        ctx.buy_venue_notional_usd = 2600.0;
        let d = evaluate_entry_gates(&ready_signal(), &cfg, &ctx);
        assert!(!d.allow);
        assert!(d.reason.contains("hyperliquid"));
        let mut ctx = GateContext::open(500.0);
        ctx.sell_venue_notional_usd = 2600.0;
        let d = evaluate_entry_gates(&ready_signal(), &cfg, &ctx);
        assert!(!d.allow);
        assert!(d.reason.contains("lighter"));
        // Exact fit at the cap is allowed.
        let mut ctx = GateContext::open(400.0);
        ctx.buy_venue_notional_usd = 2600.0;
        assert!(evaluate_entry_gates(&ready_signal(), &cfg, &ctx).allow);
    }

    #[test]
    fn gate_per_symbol_cap() {
        let (mut cfg, _) = open_ctx(0.0);
        cfg.max_notional_per_symbol_usd = 2000.0;
        let mut ctx = GateContext::open(500.0);
        ctx.symbol_notional_usd = 1600.0;
        let d = evaluate_entry_gates(&ready_signal(), &cfg, &ctx);
        assert!(!d.allow);
        assert!(d.reason.contains("BTC"));
    }

    #[test]
    fn gate_pair_cooldown_blocks_until_expiry() {
        let (cfg, mut ctx) = open_ctx(100.0);
        ctx.cooldown_remaining_secs = Some(12.0);
        let d = evaluate_entry_gates(&ready_signal(), &cfg, &ctx);
        assert!(!d.allow);
        assert!(d.reason.contains("cooling down"));
        ctx.cooldown_remaining_secs = Some(0.0);
        assert!(evaluate_entry_gates(&ready_signal(), &cfg, &ctx).allow);
        ctx.cooldown_remaining_secs = None;
        assert!(evaluate_entry_gates(&ready_signal(), &cfg, &ctx).allow);
    }

    #[test]
    fn gate_blocking_signal_collides() {
        let (cfg, mut ctx) = open_ctx(100.0);
        ctx.blocking_signal = true;
        assert_eq!(
            evaluate_entry_gates(&ready_signal(), &cfg, &ctx).reason,
            "unresolved signal on this symbol"
        );
    }

    #[test]
    fn gate_is_deterministic() {
        let s = ready_signal();
        let (cfg, ctx) = open_ctx(777.0);
        let a = evaluate_entry_gates(&s, &cfg, &ctx);
        let b = evaluate_entry_gates(&s, &cfg, &ctx);
        assert_eq!(a, b);
    }

    #[test]
    fn sizing_caps_by_breakeven_and_position_max() {
        let cfg = AutoExecConfig {
            max_notional_per_position_usd: 1000.0,
            ..AutoExecConfig::default()
        };
        let mut s = ready_signal();
        // optimal 10k, breakeven 8k, cap 1k -> 1k.
        assert_eq!(size_position(&s, &cfg), Some(1000.0));
        let wide = AutoExecConfig {
            max_notional_per_position_usd: 1_000_000.0,
            ..AutoExecConfig::default()
        };
        // breakeven binds below optimal.
        assert_eq!(size_position(&s, &wide), Some(8000.0));
        s.breakeven_size_usd = None;
        assert_eq!(size_position(&s, &wide), Some(10_000.0));
        // Breakeven above optimal never inflates size.
        s.breakeven_size_usd = Some(99_000.0);
        assert_eq!(size_position(&s, &wide), Some(10_000.0));
        // No capacity -> None.
        s.optimal_size_usd = None;
        assert_eq!(size_position(&s, &cfg), None);
        s.optimal_size_usd = Some(0.0);
        assert_eq!(size_position(&s, &cfg), None);
        s.optimal_size_usd = Some(f64::NAN);
        assert_eq!(size_position(&s, &cfg), None);
    }

    #[test]
    fn friction_band_derives_from_entry_row() {
        assert_eq!(friction_band_bps(Some(10.0), Some(6.0)), Some(4.0));
        // Sign-insensitive: a negative-deviation ordered pair still works.
        assert_eq!(friction_band_bps(Some(-10.0), Some(-6.0)), Some(4.0));
        assert_eq!(friction_band_bps(None, Some(6.0)), None);
        assert_eq!(friction_band_bps(Some(10.0), None), None);
        // Net wider than dev is unpriceable -> None (fail closed).
        assert_eq!(friction_band_bps(Some(3.0), Some(6.0)), None);
    }

    #[test]
    fn hold_deadline_scales_with_converge_time() {
        let cfg = AutoExecConfig::default(); // mult 3.0
        assert_eq!(
            hold_deadline_ms(1_000_000, Some(120.0), &cfg),
            Some(1_360_000)
        );
        assert_eq!(hold_deadline_ms(0, None, &cfg), None);
        assert_eq!(hold_deadline_ms(0, Some(0.0), &cfg), None);
        assert_eq!(hold_deadline_ms(0, Some(f64::INFINITY), &cfg), None);
    }

    #[test]
    fn plan_entry_bundles_size_friction_deadline() {
        let cfg = AutoExecConfig {
            enabled: true,
            ..AutoExecConfig::default()
        };
        let plan = plan_entry(&ready_signal(), &cfg, 0).expect("plan");
        assert_eq!(plan.size_usd, 1000.0);
        assert_eq!(plan.friction_band_bps, 4.0);
        assert_eq!(plan.deadline_ms, 360_000);
    }

    fn exit_cfg() -> AutoExecConfig {
        AutoExecConfig {
            enabled: true,
            ..AutoExecConfig::default()
        }
    }

    fn live(status: &str, dev: Option<f64>) -> LiveSpreadView {
        LiveSpreadView {
            status: status.into(),
            deviation_bps: dev,
        }
    }

    #[test]
    fn exit_on_convergence_into_friction_band() {
        let cfg = exit_cfg();
        // Entry dev 10, friction 4: |dev| 3.9 converges; exactly 4.0 converges (inclusive).
        for dev in [0.0, 3.9, 4.0, -4.0] {
            let e = evaluate_exit(10.0, 4.0, 999_999, None, Some(&live("ready", Some(dev))), 0, &cfg);
            assert!(
                matches!(e, ExitEval::Exit { trigger: ExitTrigger::Converged, .. }),
                "dev {dev} must converge, got {e:?}"
            );
        }
        // Just outside the band holds.
        assert_eq!(
            evaluate_exit(10.0, 4.0, 999_999, None, Some(&live("ready", Some(4.1))), 0, &cfg),
            ExitEval::Hold
        );
    }

    #[test]
    fn exit_on_stop_band_breach() {
        let cfg = exit_cfg(); // stop 1.5 x |10| = 15
        let e = evaluate_exit(10.0, 4.0, 999_999, None, Some(&live("ready", Some(15.0))), 0, &cfg);
        assert!(
            matches!(e, ExitEval::Exit { trigger: ExitTrigger::Stopped, .. }),
            "got {e:?}"
        );
        let e = evaluate_exit(10.0, 4.0, 999_999, None, Some(&live("ready", Some(-20.0))), 0, &cfg);
        assert!(matches!(
            e,
            ExitEval::Exit {
                trigger: ExitTrigger::Stopped,
                ..
            }
        ));
        // 14.9 holds (deadline far away).
        assert_eq!(
            evaluate_exit(10.0, 4.0, 999_999, None, Some(&live("ready", Some(14.9))), 0, &cfg),
            ExitEval::Hold
        );
    }

    #[test]
    fn stop_band_never_overlaps_convergence_band() {
        let cfg = exit_cfg();
        // Tiny entry dev: stop floors at the friction band, so the converged
        // region always wins ties.
        let e = evaluate_exit(1.0, 4.0, 999_999, None, Some(&live("ready", Some(4.0))), 0, &cfg);
        assert!(matches!(
            e,
            ExitEval::Exit {
                trigger: ExitTrigger::Converged,
                ..
            }
        ));
    }

    #[test]
    fn exit_on_max_hold_timeout() {
        let cfg = exit_cfg();
        // Mid-band dev, deadline reached -> timeout.
        let e = evaluate_exit(10.0, 4.0, 1000, None, Some(&live("ready", Some(8.0))), 1000, &cfg);
        assert!(matches!(
            e,
            ExitEval::Exit {
                trigger: ExitTrigger::TimedOut,
                ..
            }
        ));
        // One ms early holds.
        assert_eq!(
            evaluate_exit(10.0, 4.0, 1000, None, Some(&live("ready", Some(8.0))), 999, &cfg),
            ExitEval::Hold
        );
    }

    #[test]
    fn missing_or_stale_rows_hold_then_unwind() {
        let cfg = exit_cfg(); // stale grace 600s
        // Gone entirely: unwatched inside grace…
        let e = evaluate_exit(10.0, 4.0, 9_999_999, Some(60.0), None, 0, &cfg);
        assert!(matches!(e, ExitEval::Unwatched { .. }), "got {e:?}");
        // …reduce-only unwind past it.
        let e = evaluate_exit(10.0, 4.0, 9_999_999, Some(600.0), None, 0, &cfg);
        assert!(matches!(
            e,
            ExitEval::Exit {
                trigger: ExitTrigger::StaleUnwind,
                ..
            }
        ));
        // Explicit stale status behaves the same.
        let e = evaluate_exit(
            10.0,
            4.0,
            9_999_999,
            Some(601.0),
            Some(&live("no-data", Some(8.0))),
            0,
            &cfg,
        );
        assert!(matches!(
            e,
            ExitEval::Exit {
                trigger: ExitTrigger::StaleUnwind,
                ..
            }
        ));
        // Unknown live deviation (but fresh row) holds unwatched.
        let e = evaluate_exit(
            10.0,
            4.0,
            9_999_999,
            Some(1.0),
            Some(&live("ready", None)),
            0,
            &cfg,
        );
        assert!(matches!(e, ExitEval::Unwatched { .. }));
    }

    #[test]
    fn config_validation_rejects_unsafe_ranges() {
        let mut cfg = AutoExecConfig::default();
        assert!(cfg.validate().is_ok());
        cfg.min_p_win = 0.4;
        assert!(cfg.validate().is_err());
        cfg = AutoExecConfig::default();
        cfg.max_notional_per_venue_usd = 5.0; // below one position
        assert!(cfg.validate().is_err());
        cfg = AutoExecConfig::default();
        cfg.stop_mult = 0.5;
        assert!(cfg.validate().is_err());
        cfg = AutoExecConfig::default();
        cfg.poll_interval_secs = 0;
        assert!(cfg.validate().is_err());
        cfg = AutoExecConfig::default();
        cfg.max_concurrent_positions = 0;
        assert!(cfg.validate().is_err());
        cfg = AutoExecConfig::default();
        cfg.stale_unwind_secs = 10;
        assert!(cfg.validate().is_err());
        cfg = AutoExecConfig::default();
        cfg.hold_timeout_mult = 99.0;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn bot_mode_gates_entries_not_exits() {
        assert!(BotMode::Auto.entries_allowed());
        assert!(!BotMode::Paused.entries_allowed());
        assert!(!BotMode::ReduceOnly.entries_allowed());
        assert!(!BotMode::Off.entries_allowed());
        assert!(BotMode::Auto.exits_allowed());
        assert!(BotMode::Paused.exits_allowed());
        assert!(BotMode::ReduceOnly.exits_allowed());
        assert!(!BotMode::Off.exits_allowed());
        assert_eq!(BotMode::parse_mode("reduce_only"), Some(BotMode::ReduceOnly));
        assert_eq!(BotMode::parse_mode("bogus"), None);
        assert_eq!(BotMode::Off.as_str(), "off");
    }

    #[test]
    fn exit_on_dynamic_baseline_shift_convergence() {
        let cfg = exit_cfg();
        // Entry: spread 35 bps, baseline 20 bps -> entry_dev = 15 bps. Friction band = 4 bps.
        let entry_dev = 15.0;
        let friction_band = 4.0;
        let deadline = 100_000;

        // Days later: baseline shifted permanently to 25 bps.
        // Live spread is now 25.5 bps -> live_dev = 25.5 - 25.0 = 0.5 bps.
        // Live dev is well within the friction band (0.5 <= 4.0).
        let live_row = live("ready", Some(0.5));
        let e = evaluate_exit(entry_dev, friction_band, deadline, None, Some(&live_row), 10_000, &cfg);
        assert!(
            matches!(e, ExitEval::Exit { trigger: ExitTrigger::Converged, .. }),
            "expected converged, got {e:?}"
        );

        // Conversely, if spread never converges (e.g. stays at 32 bps on a 25 bps baseline -> live_dev = 7.0 bps),
        // it holds until deadline expires, then exits on timeout:
        let live_unconverged = live("ready", Some(7.0));
        let e_hold = evaluate_exit(entry_dev, friction_band, deadline, None, Some(&live_unconverged), 50_000, &cfg);
        assert_eq!(e_hold, ExitEval::Hold);

        let e_timeout = evaluate_exit(entry_dev, friction_band, deadline, None, Some(&live_unconverged), 100_000, &cfg);
        assert!(matches!(e_timeout, ExitEval::Exit { trigger: ExitTrigger::TimedOut, .. }));
    }
}
