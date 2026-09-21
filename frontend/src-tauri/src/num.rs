//! Decimal ↔ f64 conversions shared across the desktop crate. One copy so
//! every module converts identically (string-parse, never lossy float math).

use rust_decimal::Decimal;
use std::str::FromStr;

/// Decimal → finite f64, `None` when unparseable or non-finite.
pub(crate) fn dec_opt_f64(v: Decimal) -> Option<f64> {
    let f: f64 = v.to_string().parse().ok()?;
    if f.is_finite() {
        Some(f)
    } else {
        None
    }
}

/// Decimal → f64, `0.0` on failure (display/aggregate contexts only —
/// never use where a bad parse must fail closed).
pub(crate) fn dec_f64(d: Decimal) -> f64 {
    d.to_string().parse::<f64>().unwrap_or(0.0)
}

/// Positive finite f64 → Decimal, `None` otherwise.
pub(crate) fn f64_dec(v: f64) -> Option<Decimal> {
    if !v.is_finite() || v <= 0.0 {
        return None;
    }
    Decimal::from_str(&v.to_string()).ok()
}
