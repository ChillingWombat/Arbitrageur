//! StateStore persistence tests — grouped by domain; see sibling modules
//! for the implementations under test.

use super::*;

fn temp_store_path(tag: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "arbitrageur_state_store_test_{}_{}",
        tag,
        rand::random::<u64>()
    ));
    let file = dir.join("state.sqlite3");
    (dir, file)
}

#[test]
fn canonical_base_merges_equivalent_index_underlyings() {
    // The reported case: Lighter US100 == PopDEX TECH100 (Nasdaq-100).
    assert_eq!(canonical_base("US100"), "NAS100");
    assert_eq!(canonical_base("TECH100"), "NAS100");
    assert_eq!(canonical_base("US100-PERP"), "NAS100");
    assert_eq!(canonical_base("TECH100USDT"), "NAS100");
    assert_eq!(canonical_base("tech100/usdc"), "NAS100");
    assert_eq!(canonical_base("NAS100"), "NAS100");
    assert_eq!(canonical_base("USTEC"), "NAS100");
    // Siblings in the same class.
    assert_eq!(canonical_base("US500"), "SPX500");
    assert_eq!(canonical_base("US30"), "US30");
    assert_eq!(canonical_base("DJ30"), "US30");
    // Ambiguous short forms do NOT merge: "SPX" is the SPX6900 memecoin on
    // crypto venues (~$0.5), not the S&P index — fusing them corrupts every
    // downstream pairing and chart.
    assert_eq!(canonical_base("SPX"), "SPX");
    assert_eq!(canonical_base("SPX-USDC"), "SPX");
    assert_eq!(canonical_base("NQ"), "NQ");
    assert_eq!(canonical_base("DJI"), "DJI");
    // Non-indices untouched (incl. quote-strip behavior).
    assert_eq!(canonical_base("BTC-USDC"), "BTC");
    assert_eq!(canonical_base("BTCUSDT"), "BTC");
    assert_eq!(canonical_base("TUSD"), "TUSD");
    assert_eq!(canonical_base("BUSD"), "BUSD");
    assert_eq!(canonical_base("ETH-PERP"), "ETH");
}

#[test]
fn test_file_store_config_and_permissions() {
    let (dir, file) = temp_store_path("config");
    let store = StateStore::open(&file).unwrap();
    {
        let conn = store.conn.lock();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 29);
        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
        let fk: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fk, 1);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let dir_mode = fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700);
        let file_mode = fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(file_mode, 0o600);
    }
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn test_strategy_params_json_roundtrip_and_reopen() {
    let (dir, file) = temp_store_path("strategy_params");
    {
        let store = StateStore::open(&file).unwrap();
        store
            .upsert_strategy_params_json("futures", "{\"strategyId\":\"futures\"}")
            .unwrap();
        store
            .upsert_strategy_params_json("futures", "{\"strategyId\":\"futures\",\"v\":2}")
            .unwrap();
        store
            .upsert_strategy_params_json("spot", "{\"strategyId\":\"spot\"}")
            .unwrap();
        assert!(
            store.upsert_strategy_params_json("bogus", "{}").is_err(),
            "unknown strategy id must fail the CHECK constraint"
        );
    }
    {
        let reopened = StateStore::open(&file).unwrap();
        let rows = reopened.strategy_params_json().unwrap();
        assert_eq!(
            rows,
            vec![
                (
                    "futures".to_string(),
                    "{\"strategyId\":\"futures\",\"v\":2}".to_string()
                ),
                ("spot".to_string(), "{\"strategyId\":\"spot\"}".to_string()),
            ]
        );
    }
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn test_funding_rate_samples_roundtrip_and_minute_bucket() {
    let store = StateStore::open_in_memory().unwrap();
    // Two writes inside the same minute collapse to one row (REPLACE).
    store
        .record_funding_rate_samples(1_700_000_000_012, &[("lighter", "BTC-USDC", 0.0008, 8)])
        .unwrap();
    store
        .record_funding_rate_samples(1_700_000_000_045, &[("lighter", "BTC-USDC", 0.0009, 8)])
        .unwrap();
    // A later minute appends.
    store
        .record_funding_rate_samples(1_700_000_060_000, &[("lighter", "BTC-USDC", 0.001, 8)])
        .unwrap();
    let series = store
        .funding_rate_series("lighter", "BTC-USDC", 0, i64::MAX)
        .unwrap();
    assert_eq!(series.len(), 2, "got {series:?}");
    assert_eq!(series[0].0, 1_700_000_000_012 / 60_000 * 60_000);
    assert!((series[0].1 - 0.0009).abs() < 1e-12);
    assert_eq!(series[0].2, 8);
    // Bad rows never abort the batch.
    store
        .record_funding_rate_samples(
            1_700_000_120_000,
            &[
                ("!!", "BTC-USDC", 0.001, 8),
                ("lighter", "BTC-USDC", f64::NAN, 8),
            ],
        )
        .unwrap();
    let after = store
        .funding_rate_series("lighter", "BTC-USDC", 0, i64::MAX)
        .unwrap();
    assert_eq!(after.len(), 2);
    // Retention prune keeps only the fresh minute bucket (1_700_000_040_000).
    let removed = store.prune_funding_rate_samples(1_700_000_040_000).unwrap();
    assert_eq!(removed, 1);
    let kept = store
        .funding_rate_series("lighter", "BTC-USDC", 0, i64::MAX)
        .unwrap();
    assert_eq!(kept.len(), 1);
}

#[test]
fn test_default_account_lifecycle() {
    let store = StateStore::open_in_memory().unwrap();
    let first = store
        .upsert_account(
            "hl_main",
            "HyperLiquid",
            "Main",
            true,
            false,
            "standard",
            None,
            None,
        )
        .unwrap();
    assert!(first.is_default);
    assert!(store.has_enabled_account("hyperliquid", "live").unwrap());
    assert!(!store.has_enabled_account("lighter", "live").unwrap());

    let second = store
        .upsert_account(
            "hl_alt",
            "hyperliquid",
            "Alt",
            true,
            false,
            "standard",
            None,
            None,
        )
        .unwrap();
    assert!(!second.is_default);
    assert_eq!(
        store
            .default_account("hyperliquid", "live")
            .unwrap()
            .unwrap()
            .id,
        "hl_main"
    );

    store
        .upsert_account(
            "hl_zulu",
            "hyperliquid",
            "Zulu",
            true,
            false,
            "standard",
            None,
            None,
        )
        .unwrap();
    store.set_default_account("hyperliquid", "hl_zulu").unwrap();
    assert!(store.get_account("hl_zulu").unwrap().unwrap().is_default);
    assert!(!store.get_account("hl_main").unwrap().unwrap().is_default);

    store
        .upsert_account(
            "hl_zulu",
            "hyperliquid",
            "Zulu",
            false,
            false,
            "standard",
            None,
            None,
        )
        .unwrap();
    let promoted = store
        .default_account("hyperliquid", "live")
        .unwrap()
        .unwrap();
    assert_eq!(promoted.id, "hl_alt");

    store
        .upsert_account(
            "hl_alt",
            "hyperliquid",
            "Alt",
            false,
            false,
            "standard",
            None,
            None,
        )
        .unwrap();
    store
        .upsert_account(
            "hl_main",
            "hyperliquid",
            "Main",
            false,
            false,
            "standard",
            None,
            None,
        )
        .unwrap();
    assert!(store
        .default_account("hyperliquid", "live")
        .unwrap()
        .is_none());
    assert!(!store.has_enabled_account("hyperliquid", "live").unwrap());

    let reenabled = store
        .upsert_account(
            "hl_alt",
            "hyperliquid",
            "Alt",
            true,
            false,
            "standard",
            None,
            None,
        )
        .unwrap();
    assert!(reenabled.is_default);
}

#[test]
fn test_delete_default_promotes_alphabetically_first_enabled() {
    let store = StateStore::open_in_memory().unwrap();
    store
        .upsert_account(
            "a_main",
            "hyperliquid",
            "Main",
            true,
            true,
            "standard",
            None,
            None,
        )
        .unwrap();
    store
        .upsert_account(
            "a_zulu",
            "hyperliquid",
            "Zulu",
            true,
            false,
            "standard",
            None,
            None,
        )
        .unwrap();
    store
        .upsert_account(
            "a_beta",
            "hyperliquid",
            "Beta",
            true,
            false,
            "standard",
            None,
            None,
        )
        .unwrap();
    assert!(store.delete_account("a_main").unwrap());
    let promoted = store
        .default_account("hyperliquid", "live")
        .unwrap()
        .unwrap();
    assert_eq!(promoted.id, "a_beta");
}

#[test]
fn test_validation_and_immutable_venue() {
    let store = StateStore::open_in_memory().unwrap();
    assert!(store
        .upsert_account(
            "",
            "hyperliquid",
            "Main",
            true,
            false,
            "standard",
            None,
            None
        )
        .is_err());
    assert!(store
        .upsert_account(
            "bad id",
            "hyperliquid",
            "Main",
            true,
            false,
            "standard",
            None,
            None
        )
        .is_err());
    assert!(store
        .upsert_account(
            &"x".repeat(129),
            "hyperliquid",
            "Main",
            true,
            false,
            "standard",
            None,
            None
        )
        .is_err());
    assert!(store
        .upsert_account("a", "", "Main", true, false, "standard", None, None)
        .is_err());
    assert!(store
        .upsert_account(
            "a",
            "bad venue",
            "Main",
            true,
            false,
            "standard",
            None,
            None
        )
        .is_err());
    assert!(store
        .upsert_account("a", "hyperliquid", "", true, false, "standard", None, None)
        .is_err());
    assert!(store
        .upsert_account(
            "a",
            "hyperliquid",
            &"y".repeat(81),
            true,
            false,
            "standard",
            None,
            None
        )
        .is_err());
    assert!(store
        .upsert_account(
            "a",
            "hyperliquid",
            "bad\u{0007}label",
            true,
            false,
            "standard",
            None,
            None
        )
        .is_err());

    store
        .upsert_account(
            "acct1",
            "hyperliquid",
            "Main",
            true,
            false,
            "standard",
            None,
            None,
        )
        .unwrap();
    assert!(store
        .upsert_account("acct1", "lighter", "Moved", true, false, "standard", None, None)
        .is_err());
    let renamed = store
        .upsert_account(
            "acct1",
            "HyperLiquid",
            "Renamed",
            true,
            false,
            "standard",
            None,
            None,
        )
        .unwrap();
    assert_eq!(renamed.venue, "hyperliquid");
    assert_eq!(renamed.label, "Renamed");
}

#[test]
fn test_label_uniqueness_scoped_to_venue() {
    let store = StateStore::open_in_memory().unwrap();
    store
        .upsert_account(
            "a1",
            "hyperliquid",
            "Main",
            true,
            false,
            "standard",
            None,
            None,
        )
        .unwrap();
    assert!(store
        .upsert_account(
            "a2",
            "hyperliquid",
            "MAIN",
            true,
            false,
            "standard",
            None,
            None
        )
        .is_err());
    store
        .upsert_account("a2", "lighter", "main", true, false, "standard", None, None)
        .unwrap();
    assert_eq!(store.list_accounts(None).unwrap().len(), 2);
}

#[test]
fn test_route_rules_and_canonicalization() {
    let store = StateStore::open_in_memory().unwrap();
    store
        .upsert_account(
            "hl_a",
            "hyperliquid",
            "HL A",
            true,
            false,
            "standard",
            None,
            None,
        )
        .unwrap();
    store
        .upsert_account(
            "hl_b",
            "hyperliquid",
            "HL B",
            true,
            false,
            "standard",
            None,
            None,
        )
        .unwrap();
    store
        .upsert_account(
            "lt_a", "lighter", "LT A", true, false, "standard", None, None,
        )
        .unwrap();
    store
        .upsert_account(
            "lt_off", "lighter", "LT Off", false, false, "standard", None, None,
        )
        .unwrap();

    assert!(store
        .upsert_strategy_route("r1", "unknown", "R", "hl_a", "lt_a", true)
        .is_err());
    assert!(store
        .upsert_strategy_route("r1", "futures", "R", "hl_a", "hl_a", true)
        .is_err());
    assert!(store
        .upsert_strategy_route("r1", "futures", "R", "hl_a", "hl_b", true)
        .is_err());
    assert!(store
        .upsert_strategy_route("r1", "futures", "R", "hl_a", "lt_off", true)
        .is_err());
    assert!(store
        .upsert_strategy_route("r1", "futures", "R", "missing", "lt_a", true)
        .is_err());

    let disabled_route = store
        .upsert_strategy_route("r_off", "futures", "Off", "hl_a", "lt_off", false)
        .unwrap();
    assert!(!disabled_route.enabled);

    let route = store
        .upsert_strategy_route("r1", "Futures", "Main Route", "lt_a", "hl_a", true)
        .unwrap();
    assert_eq!(route.account_a_id, "hl_a");
    assert_eq!(route.account_a_venue, "hyperliquid");
    assert_eq!(route.account_b_id, "lt_a");
    assert_eq!(route.account_b_venue, "lighter");
    assert_eq!(route.strategy_id, "futures");

    assert!(store
        .upsert_strategy_route("r2", "futures", "Dup", "hl_a", "lt_a", true)
        .is_err());
    assert!(store
        .upsert_strategy_route("r2", "futures", "Dup", "lt_a", "hl_a", true)
        .is_err());
    store
        .upsert_strategy_route("r2", "spot", "Other", "hl_a", "lt_a", true)
        .unwrap();
}

#[test]
fn test_referenced_account_delete_requires_route_removal() {
    let store = StateStore::open_in_memory().unwrap();
    store
        .upsert_account(
            "hl_a",
            "hyperliquid",
            "HL A",
            true,
            false,
            "standard",
            None,
            None,
        )
        .unwrap();
    store
        .upsert_account(
            "lt_a", "lighter", "LT A", true, false, "standard", None, None,
        )
        .unwrap();
    store
        .upsert_strategy_route("r1", "futures", "Route", "hl_a", "lt_a", true)
        .unwrap();

    let err = store.delete_account("hl_a").unwrap_err();
    assert!(err.to_string().contains("strategy routes"));
    assert!(!store.delete_account("missing").unwrap());

    assert!(store.delete_strategy_route("r1").unwrap());
    assert!(!store.delete_strategy_route("r1").unwrap());
    assert!(store.delete_account("hl_a").unwrap());
}

#[test]
fn test_eligible_strategy_routes_filters() {
    let store = StateStore::open_in_memory().unwrap();
    store
        .upsert_account(
            "hl_a",
            "hyperliquid",
            "HL A",
            true,
            false,
            "standard",
            None,
            None,
        )
        .unwrap();
    store
        .upsert_account(
            "hl_b",
            "hyperliquid",
            "HL B",
            true,
            false,
            "standard",
            None,
            None,
        )
        .unwrap();
    store
        .upsert_account(
            "lt_a", "lighter", "LT A", true, false, "standard", None, None,
        )
        .unwrap();
    store
        .upsert_account(
            "lt_b", "lighter", "LT B", true, false, "standard", None, None,
        )
        .unwrap();

    store
        .upsert_strategy_route("r1", "futures", "Active", "hl_a", "lt_a", true)
        .unwrap();
    store
        .upsert_strategy_route("r2", "futures", "Inactive", "hl_b", "lt_a", false)
        .unwrap();
    store
        .upsert_strategy_route("r3", "futures", "Stale", "hl_a", "lt_b", true)
        .unwrap();
    store
        .upsert_account(
            "lt_b", "lighter", "LT B", false, false, "standard", None, None,
        )
        .unwrap();

    let eligible = store
        .eligible_strategy_routes("futures", "hyperliquid", "lighter", "live")
        .unwrap();
    assert_eq!(eligible.len(), 1);
    assert_eq!(eligible[0].id, "r1");

    let reversed = store
        .eligible_strategy_routes("futures", "lighter", "hyperliquid", "live")
        .unwrap();
    assert_eq!(reversed.len(), 1);
    assert_eq!(reversed[0].id, "r1");

    assert!(store
        .eligible_strategy_routes("futures", "hyperliquid", "kraken", "live")
        .unwrap()
        .is_empty());
    assert!(store
        .eligible_strategy_routes("spot", "hyperliquid", "lighter", "live")
        .unwrap()
        .is_empty());
}

#[test]
fn test_file_store_persists_across_reopen() {
    let (dir, file) = temp_store_path("persist");
    {
        let store = StateStore::open(&file).unwrap();
        store
            .upsert_account(
                "hl_a",
                "hyperliquid",
                "Main",
                true,
                false,
                "standard",
                None,
                None,
            )
            .unwrap();
        store
            .upsert_account(
                "lt_a", "lighter", "Alt", true, false, "standard", None, None,
            )
            .unwrap();
        store
            .upsert_strategy_route("r1", "futures", "Route", "hl_a", "lt_a", true)
            .unwrap();
    }

    let store = StateStore::open(&file).unwrap();
    let accounts = store.list_accounts(None).unwrap();
    assert_eq!(accounts.len(), 2);
    assert!(store.get_account("hl_a").unwrap().unwrap().is_default);
    let routes = store.list_strategy_routes(Some("futures")).unwrap();
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].account_a_id, "hl_a");
    assert_eq!(store.list_strategy_routes(None).unwrap().len(), 1);
    let _ = fs::remove_dir_all(dir);
}

fn two_leg_signal(
    id: &str,
    symbol: &str,
    acct_a: Option<&str>,
    acct_b: Option<&str>,
) -> NewSignalExecution {
    NewSignalExecution {
        id: id.to_string(),
        strategy_id: "futures".into(),
        symbol: symbol.to_string(),
        route_id: None,
        expected_net_usd: Some(12.5),
        note: None,
        legs: vec![
            NewSignalLeg {
                account_id: acct_a.map(|s| s.to_string()),
                venue: "hyperliquid".into(),
                side: LegSide::Long,
                symbol: None,
                intended_qty: 1.0,
                intended_price: None,
            },
            NewSignalLeg {
                account_id: acct_b.map(|s| s.to_string()),
                venue: "lighter".into(),
                side: LegSide::Short,
                symbol: None,
                intended_qty: 1.0,
                intended_price: None,
            },
        ],
    }
}

#[test]
fn test_leg_symbol_none_resolves_to_root_and_some_is_exact() {
    let store = StateStore::open_in_memory().unwrap();
    let version: i64 = store
        .conn
        .lock()
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 29);
    store
        .record_signal_execution(&NewSignalExecution {
            id: "fund-1".to_string(),
            strategy_id: "funding".into(),
            symbol: "BTC-USDC".to_string(),
            route_id: None,
            expected_net_usd: None,
            note: None,
            legs: vec![
                NewSignalLeg {
                    account_id: None,
                    venue: "lighter".into(),
                    side: LegSide::Long,
                    symbol: Some("BTC-SPOT".to_string()),
                    intended_qty: 0.5,
                    intended_price: None,
                },
                NewSignalLeg {
                    account_id: None,
                    venue: "lighter".into(),
                    side: LegSide::Short,
                    symbol: None,
                    intended_qty: 0.5,
                    intended_price: None,
                },
            ],
        })
        .unwrap();
    let sig = store.get_signal("fund-1").unwrap().expect("signal");
    let long = sig
        .legs
        .iter()
        .find(|l| l.side == LegSide::Long)
        .expect("long");
    let short = sig
        .legs
        .iter()
        .find(|l| l.side == LegSide::Short)
        .expect("short");
    assert_eq!(long.symbol, "BTC-SPOT");
    assert_eq!(short.symbol, "BTC-USDC");
    store
        .apply_leg_fill("fund-1", LegSide::Long, 0.5, 64990.0, 0.1, None)
        .unwrap();
    store
        .apply_leg_fill("fund-1", LegSide::Short, 0.5, 65000.0, 0.1, None)
        .unwrap();
    assert!((store.venue_symbol_net("lighter", "BTC-SPOT").unwrap() - 0.5).abs() < 1e-9);
    assert!((store.venue_symbol_net("lighter", "BTC-USDC").unwrap() + 0.5).abs() < 1e-9);
}

#[test]
fn test_v22_backfill_copies_parent_root_symbol() {
    let store = StateStore::open_in_memory().unwrap();
    store
        .record_signal_execution(&two_leg_signal("s-v21", "ETH-USDC", None, None))
        .unwrap();
    // Simulate a pre-v22 row, then replay the v22 backfill verbatim.
    store
        .conn
        .lock()
        .execute(
            "UPDATE signal_legs SET symbol = '' WHERE signal_id = 's-v21'",
            [],
        )
        .unwrap();
    store
            .conn
            .lock()
            .execute(
                "UPDATE signal_legs SET symbol = (SELECT symbol FROM signal_executions WHERE signal_executions.id = signal_legs.signal_id) WHERE symbol = ''",
                [],
            )
            .unwrap();
    let sig = store.get_signal("s-v21").unwrap().expect("signal");
    assert!(sig.legs.iter().all(|l| l.symbol == "ETH-USDC"));
    assert_eq!(store.ticker_net_position("ETH-USDC").unwrap(), 0.0);
}

#[test]
fn test_signal_lifecycle_blocks_then_frees_ticker() {
    let store = StateStore::open_in_memory().unwrap();
    store
        .upsert_account(
            "hl_a",
            "hyperliquid",
            "Main",
            true,
            false,
            "standard",
            None,
            None,
        )
        .unwrap();
    store
        .upsert_account(
            "lt_a", "lighter", "Main", true, false, "standard", None, None,
        )
        .unwrap();

    let sig = store
        .record_signal_execution(&two_leg_signal(
            "s1",
            "BTC-USDC",
            Some("hl_a"),
            Some("lt_a"),
        ))
        .unwrap();
    assert_eq!(sig.status, SignalStatus::InFlight);
    assert_eq!(sig.account_long_id.as_deref(), Some("hl_a"));
    assert_eq!(sig.account_short_id.as_deref(), Some("lt_a"));
    assert_eq!(sig.legs.len(), 2);
    assert!(store.ticker_blocked("btc-usdc").unwrap());

    // Full fills on both legs → per-signal delta reconciles to zero.
    store
        .apply_leg_fill("s1", LegSide::Long, 1.0, 60000.0, 1.2, Some("o-long"))
        .unwrap();
    store
        .apply_leg_fill("s1", LegSide::Short, 1.0, 60120.0, 1.3, Some("o-short"))
        .unwrap();
    assert!((store.signal_delta("s1").unwrap()).abs() < 1e-9);
    store
        .update_signal_status("s1", SignalStatus::Balanced, None)
        .unwrap();
    assert!(!store.ticker_blocked("BTC-USDC").unwrap());
    // The open position still contributes legs to per-venue/account nets.
    assert!((store.venue_symbol_net("hyperliquid", "BTC-USDC").unwrap() - 1.0).abs() < 1e-9);
    assert!((store.venue_symbol_net("lighter", "BTC-USDC").unwrap() + 1.0).abs() < 1e-9);
    assert!((store.account_symbol_net("hl_a", "BTC-USDC").unwrap() - 1.0).abs() < 1e-9);
    assert!((store.account_symbol_net("lt_a", "BTC-USDC").unwrap() + 1.0).abs() < 1e-9);
    // Ticker net is zero only because the signal hedged itself.
    assert!((store.ticker_net_position("BTC-USDC").unwrap()).abs() < 1e-9);

    let recorded = store.get_signal("s1").unwrap().unwrap();
    assert_eq!(recorded.status, SignalStatus::Balanced);
    assert!((recorded.fees_usd - 2.5).abs() < 1e-9);
}

#[test]
fn test_signal_predictions_record_resolve_list() {
    let store = StateStore::open_in_memory().unwrap();
    store
        .record_signal_execution(&two_leg_signal("s-pred", "BTC-USDC", None, None))
        .unwrap();

    // Two kinds can attach to one signal (composite PK).
    store
        .record_signal_prediction(
            "s-pred",
            "net_win",
            0.72,
            Some(4.5),
            "futures",
            "BTC-USDC",
            "hyperliquid/lighter",
            false,
        )
        .unwrap();
    store
        .record_signal_prediction(
            "s-pred",
            "reversion",
            0.91,
            None,
            "futures",
            "BTC-USDC",
            "hyperliquid/lighter",
            false,
        )
        .unwrap();
    // Out-of-range probability fails closed.
    assert!(store
        .record_signal_prediction(
            "s-pred", "net_win", 1.5, None, "futures", "BTC-USDC", "x/y", false,
        )
        .is_err());

    let rows = store
        .list_signal_predictions(None, None, None, false)
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r.realized_net_bps.is_none()));

    // Unresolved-only listing is empty until the outcome lands.
    assert!(store
        .list_signal_predictions(None, None, None, true)
        .unwrap()
        .is_empty());

    // Resolution stamps realized net bps on every kind of the signal.
    let resolved = store.resolve_signal_predictions("s-pred", 6.25).unwrap();
    assert_eq!(resolved, 2);
    let scored = store
        .list_signal_predictions(None, None, None, true)
        .unwrap();
    assert_eq!(scored.len(), 2);
    assert!(scored.iter().all(|r| r.realized_net_bps == Some(6.25)));

    // First outcome wins: re-resolution must not rewrite history.
    let again = store.resolve_signal_predictions("s-pred", -99.0).unwrap();
    assert_eq!(again, 0);
    let still = store
        .list_signal_predictions(None, None, None, true)
        .unwrap();
    assert!(still.iter().all(|r| r.realized_net_bps == Some(6.25)));

    // Filters work.
    assert_eq!(
        store
            .list_signal_predictions(Some("net_win"), None, None, false)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store
            .list_signal_predictions(None, Some("futures"), Some(false), false)
            .unwrap()
            .len(),
        2
    );
    // Cascade: deleting the parent signal removes its predictions.
    store
        .conn
        .lock()
        .execute("DELETE FROM signal_executions WHERE id = 's-pred'", [])
        .unwrap();
    assert!(store
        .list_signal_predictions(None, None, None, false)
        .unwrap()
        .is_empty());
}

#[test]
fn record_leg_order_id_before_fill() {
    let store = StateStore::open_in_memory().unwrap();
    store
        .record_signal_execution(&two_leg_signal("s-ack", "BTC-USDC", None, None))
        .unwrap();
    store
        .record_leg_order_id("s-ack", LegSide::Short, "venue-order-42", Some("cid-ack-1"))
        .unwrap();
    let sig = store.get_signal("s-ack").unwrap().unwrap();
    let short = sig
        .legs
        .iter()
        .find(|l| l.side == LegSide::Short)
        .expect("short leg");
    assert_eq!(short.venue_order_id.as_deref(), Some("venue-order-42"));
    assert_eq!(short.client_order_id.as_deref(), Some("cid-ack-1"));
    assert_eq!(short.filled_qty, 0.0);
    assert!(store
        .record_leg_order_id("s-ack", LegSide::Short, "", None)
        .is_err());
}

#[test]
fn test_v27_intended_price_roundtrip() {
    // Dispatch-time quotes persist on the leg and read back verbatim.
    let store = StateStore::open_in_memory().unwrap();
    let mut sig = two_leg_signal("s-intended", "BTC-USDC", None, None);
    sig.legs[0].intended_price = Some(100.0);
    sig.legs[1].intended_price = Some(100.5);
    store.record_signal_execution(&sig).unwrap();
    let got = store.get_signal("s-intended").unwrap().unwrap();
    let long = got.legs.iter().find(|l| l.side == LegSide::Long).unwrap();
    let short = got.legs.iter().find(|l| l.side == LegSide::Short).unwrap();
    assert_eq!(long.intended_price, Some(100.0));
    assert_eq!(short.intended_price, Some(100.5));
}

#[test]
fn test_v27_slippage_direction_aware_and_missing_data() {
    let store = StateStore::open_in_memory().unwrap();

    // Long paying above quote is adverse (+10 bps); short filling above
    // quote is favorable (negative bps).
    let mut sig = two_leg_signal("s-slip", "BTC-USDC", None, None);
    sig.legs[0].intended_price = Some(100.0);
    sig.legs[1].intended_price = Some(100.5);
    store.record_signal_execution(&sig).unwrap();
    store
        .apply_leg_fill("s-slip", LegSide::Long, 1.0, 100.1, 0.0, None)
        .unwrap();
    store
        .apply_leg_fill("s-slip", LegSide::Short, 1.0, 100.6, 0.0, None)
        .unwrap();
    let hl = store.leg_slippage_samples_bps("hyperliquid", 10).unwrap();
    assert_eq!(hl.len(), 1);
    assert!((hl[0].0 - 10.0).abs() < 1e-9);
    assert_eq!(hl[0].1, 1.0);
    let li = store.leg_slippage_samples_bps("lighter", 10).unwrap();
    assert_eq!(li.len(), 1);
    assert!((li[0].0 - (100.5 - 100.6) / 100.5 * 10_000.0).abs() < 1e-9);

    // No dispatch quote → excluded even after a fill lands.
    store
        .record_signal_execution(&two_leg_signal("s-none", "BTC-USDC", None, None))
        .unwrap();
    store
        .apply_leg_fill("s-none", LegSide::Long, 1.0, 100.0, 0.0, None)
        .unwrap();
    // Intended price without any fill → excluded.
    let mut pending = two_leg_signal("s-pending", "BTC-USDC", None, None);
    pending.legs[0].intended_price = Some(100.0);
    store.record_signal_execution(&pending).unwrap();

    let hl = store.leg_slippage_samples_bps("hyperliquid", 10).unwrap();
    assert_eq!(hl.len(), 1, "only the fully-priced fill may sample");
    assert!((hl[0].0 - 10.0).abs() < 1e-9);
}

#[test]
fn test_migration_v27_intended_price_defaults_null() {
    // Replay to v26, seed a signal + filled leg under the pre-v27
    // schema, then migrate: the leg survives with intended_price NULL —
    // excluded from slippage samples rather than backfilled from the
    // fill price.
    let conn = Connection::open_in_memory().unwrap();
    for batch in [
        MIGRATION_V1_SQL,
        MIGRATION_V2_SQL,
        MIGRATION_V3_SQL,
        MIGRATION_V4_SQL,
        MIGRATION_V5_SQL,
        MIGRATION_V6_SQL,
        MIGRATION_V7_SQL,
        MIGRATION_V8_SQL,
        MIGRATION_V9_SQL,
        MIGRATION_V10_SQL,
        MIGRATION_V11_SQL,
        MIGRATION_V12_SQL,
        MIGRATION_V13_SQL,
        MIGRATION_V14_SQL,
        MIGRATION_V15_SQL,
        MIGRATION_V16_SQL,
        MIGRATION_V17_SQL,
        MIGRATION_V18_SQL,
        MIGRATION_V19_SQL,
        MIGRATION_V20_SQL,
        MIGRATION_V21_SQL,
        MIGRATION_V22_SQL,
        MIGRATION_V23_SQL,
        MIGRATION_V24_SQL,
        MIGRATION_V25_SQL,
        MIGRATION_V26_SQL,
    ] {
        conn.execute_batch(batch).unwrap();
    }
    let v: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, 26);
    conn.execute(
        "INSERT INTO signal_executions
                (id, strategy_id, symbol, status, created_at, updated_at)
             VALUES ('sig-pre', 'futures', 'BTC-USDC', 'closed', 't0', 't0')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO signal_legs
                (id, signal_id, venue, side, intended_qty, filled_qty,
                 fill_price, created_at, updated_at)
             VALUES ('leg-pre', 'sig-pre', 'hyperliquid', 'long', 1.0, 1.0,
                     100.0, 't0', 't0')",
        [],
    )
    .unwrap();
    StateStore::migrate(&conn).unwrap();
    let v: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, 29);
    let ip: Option<f64> = conn
        .query_row(
            "SELECT intended_price FROM signal_legs WHERE id = 'leg-pre'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(ip, None);
}

#[test]
fn pending_order_legs_and_monotonic_fill_state() {
    let store = StateStore::open_in_memory().unwrap();
    store
        .record_signal_execution(&two_leg_signal("s-recon", "BTC-USDC", None, None))
        .unwrap();
    // No order ids yet — nothing to reconcile.
    assert!(store.list_pending_order_legs().unwrap().is_empty());
    store
        .record_leg_order_id("s-recon", LegSide::Short, "ord-77", Some("cid-recon-1"))
        .unwrap();
    let pending = store.list_pending_order_legs().unwrap();
    assert_eq!(pending.len(), 1);
    let (exec, leg) = &pending[0];
    assert_eq!(exec.id, "s-recon");
    assert_eq!(leg.side, LegSide::Short);
    assert_eq!(leg.venue_order_id.as_deref(), Some("ord-77"));

    // Venue reports cumulative fills: apply 0.4, then 0.7 — monotone up.
    store
        .apply_leg_order_state("s-recon", LegSide::Short, 0.4, Some(100.0), 0.01)
        .unwrap();
    store
        .apply_leg_order_state("s-recon", LegSide::Short, 0.7, Some(101.0), 0.02)
        .unwrap();
    // A stale venue snapshot reporting less must never regress the ledger.
    store
        .apply_leg_order_state("s-recon", LegSide::Short, 0.2, None, 0.005)
        .unwrap();
    let sig = store.get_signal("s-recon").unwrap().unwrap();
    let short = sig.legs.iter().find(|l| l.side == LegSide::Short).unwrap();
    assert_eq!(short.filled_qty, 0.7);
    assert_eq!(short.fill_price, Some(101.0));
    assert_eq!(short.fee_usd, 0.02);
    assert_eq!(sig.fees_usd, 0.02);

    // Terminal signals drop out of the poll set.
    store
        .update_signal_status("s-recon", SignalStatus::Closed, None)
        .unwrap();
    assert!(store.list_pending_order_legs().unwrap().is_empty());
}

#[test]
fn test_imbalanced_signal_blocks_ticker_despite_net_zero() {
    let store = StateStore::open_in_memory().unwrap();
    // The user's exact scenario: the ticker's aggregate position reads
    // flat, but only because signal A carries −1 while signal B carries
    // +1. Neither signal hedged itself, so both must block.
    store
        .record_signal_execution(&two_leg_signal("sa", "BTC-USDC", None, None))
        .unwrap();
    store
        .apply_leg_fill("sa", LegSide::Short, 1.0, 60120.0, 0.0, Some("o-a"))
        .unwrap();
    store
        .update_signal_status("sa", SignalStatus::Imbalanced, Some("long leg failed"))
        .unwrap();

    let mut b = two_leg_signal("sb", "BTC-USDC", None, None);
    b.legs[0].venue = "lighter".into();
    b.legs[1].venue = "hyperliquid".into();
    store.record_signal_execution(&b).unwrap();
    store
        .apply_leg_fill("sb", LegSide::Long, 1.0, 60050.0, 0.0, Some("o-b"))
        .unwrap();
    store
        .update_signal_status("sb", SignalStatus::Imbalanced, Some("short leg failed"))
        .unwrap();

    // Aggregate ticker position is flat (−1 + 1 = 0)...
    assert!((store.ticker_net_position("BTC-USDC").unwrap()).abs() < 1e-9);
    // ...yet each signal individually holds residual exposure.
    assert!((store.signal_delta("sa").unwrap() + 1.0).abs() < 1e-9);
    assert!((store.signal_delta("sb").unwrap() - 1.0).abs() < 1e-9);
    // The ticker stays blocked until BOTH imbalanced signals reconcile.
    assert!(store.ticker_blocked("BTC-USDC").unwrap());
    let blockers = store.blocking_signals("BTC-USDC").unwrap();
    assert_eq!(blockers.len(), 2);

    // Closing B's exposure alone is not enough — A still blocks.
    store
        .update_signal_status("sb", SignalStatus::Closed, None)
        .unwrap();
    assert!(store.ticker_blocked("BTC-USDC").unwrap());
    let blockers = store.blocking_signals("BTC-USDC").unwrap();
    assert_eq!(blockers.len(), 1);
    assert_eq!(blockers[0].id, "sa");
}

#[test]
fn test_in_flight_signal_survives_reopen() {
    let (dir, file) = temp_store_path("signal_persist");
    {
        let store = StateStore::open(&file).unwrap();
        store
            .record_signal_execution(&two_leg_signal("s-live", "ETH-USDC", None, None))
            .unwrap();
        assert!(store.ticker_blocked("ETH-USDC").unwrap());
    }
    // Restart: the interrupted in-flight signal still blocks the ticker.
    let store = StateStore::open(&file).unwrap();
    assert!(store.ticker_blocked("eth-usdc").unwrap());
    let blockers = store.blocking_signals("ETH-USDC").unwrap();
    assert_eq!(blockers.len(), 1);
    assert_eq!(blockers[0].status, SignalStatus::InFlight);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn test_signal_validation_and_account_fk() {
    let store = StateStore::open_in_memory().unwrap();
    // Unknown strategy IDs and empty symbols are rejected.
    let mut bad = two_leg_signal("bad", "BTC-USDC", None, None);
    bad.strategy_id = "nonsense".into();
    assert!(store.record_signal_execution(&bad).is_err());
    let mut bad2 = two_leg_signal("bad2", "  ", None, None);
    bad2.symbol = " ".into();
    assert!(store.record_signal_execution(&bad2).is_err());
    // Legs referencing a nonexistent account violate the FK.
    assert!(store
        .record_signal_execution(&two_leg_signal("bad3", "BTC-USDC", Some("ghost"), None))
        .is_err());
    // Zero/negative intended quantities are rejected.
    let mut bad4 = two_leg_signal("bad4", "BTC-USDC", None, None);
    bad4.legs[0].intended_qty = 0.0;
    assert!(store.record_signal_execution(&bad4).is_err());

    // account_has_signal_legs gates deletion.
    store
        .upsert_account(
            "hl_a",
            "hyperliquid",
            "Main",
            true,
            false,
            "standard",
            None,
            None,
        )
        .unwrap();
    assert!(!store.account_has_signal_legs("hl_a").unwrap());
    store
        .record_signal_execution(&two_leg_signal("s9", "BTC-USDC", Some("hl_a"), None))
        .unwrap();
    assert!(store.account_has_signal_legs("hl_a").unwrap());
    assert!(store.delete_account("hl_a").is_err());
}

#[test]
fn test_spatial_baseline_roundtrip_and_validation() {
    let store = StateStore::open_in_memory().unwrap();
    assert_eq!(normalize_strategy_id("triangle").unwrap(), "triangle");
    store
        .record_signal_execution(&two_leg_signal("sig-base-1", "BTC-USDC", None, None))
        .unwrap();
    let row = SpatialEntryBaseline {
        signal_id: "sig-base-1".into(),
        symbol: "BTC-USDC".into(),
        buy_venue: "hyperliquid".into(),
        sell_venue: "lighter".into(),
        entry_net_bps: 20.5,
        baseline_bps: Some(12.0),
        entry_ts_ms: 1_000_000,
    };
    store.record_spatial_baseline(&row).unwrap();
    let got = store
        .spatial_baseline_for_signal("sig-base-1")
        .unwrap()
        .expect("baseline present");
    assert_eq!(got, row);
    assert!(store
        .spatial_baseline_for_signal("sig-missing")
        .unwrap()
        .is_none());
    let mut bad = row.clone();
    bad.signal_id = "sig-bad".into();
    bad.entry_net_bps = f64::NAN;
    assert!(store.record_spatial_baseline(&bad).is_err());
    let mut bad = row.clone();
    bad.signal_id = "sig-bad2".into();
    bad.entry_ts_ms = -1;
    assert!(store.record_spatial_baseline(&bad).is_err());
    // Re-recording the same signal replaces the baseline (re-entry).
    let mut updated = row.clone();
    updated.entry_net_bps = 18.0;
    store.record_spatial_baseline(&updated).unwrap();
    let got = store
        .spatial_baseline_for_signal("sig-base-1")
        .unwrap()
        .expect("baseline present");
    assert_eq!(got.entry_net_bps, 18.0);
}

fn sample(venue_id: &str, mid: f64) -> VenueSampleEntry {
    VenueSampleEntry {
        venue_id: venue_id.into(),
        mid,
        best_bid: mid - 0.5,
        best_ask: mid + 0.5,
        bid_depth: None,
        ask_depth: None,
    }
}

#[test]
fn test_spread_samples_record_order_and_filter() {
    let store = StateStore::open_in_memory().unwrap();
    store
        .record_spread_samples(
            "BTC-USDC",
            1_000,
            &[sample("hyperliquid", 100.0), sample("lighter", 101.0)],
        )
        .unwrap();
    store
        .record_spread_samples("BTC-USDC", 2_000, &[sample("hyperliquid", 100.5)])
        .unwrap();
    // Other symbol is isolated by the query.
    store
        .record_spread_samples("ETH-USDC", 1_500, &[sample("lighter", 50.0)])
        .unwrap();

    let all = store.spread_samples("BTC-USDC", 0, i64::MAX).unwrap();
    assert_eq!(all.len(), 3);
    assert_eq!(all[0].ts_ms, 1_000);
    assert_eq!(all[0].venue_id, "hyperliquid");
    assert_eq!(all[1].venue_id, "lighter");
    assert_eq!(all[2].ts_ms, 2_000);

    let windowed = store.spread_samples("BTC-USDC", 1_500, 2_500).unwrap();
    assert_eq!(windowed.len(), 1);
    assert_eq!(windowed[0].mid, 100.5);

    assert_eq!(
        store.earliest_spread_sample_ts("BTC-USDC").unwrap(),
        Some(1_000)
    );
    assert_eq!(store.earliest_spread_sample_ts("SOL-USDC").unwrap(), None);
}

#[test]
fn test_v28_spread_samples_depth_roundtrip() {
    // Depth persists per venue and stays NULL when absent — the WFO
    // weight is real book data or honest nothing, never 0.0-sham.
    let store = StateStore::open_in_memory().unwrap();
    store
        .record_spread_samples(
            "BTC-USDC",
            1_000,
            &[
                VenueSampleEntry {
                    venue_id: "aaa".into(),
                    mid: 100.0,
                    best_bid: 99.9,
                    best_ask: 100.1,
                    bid_depth: Some(5.0),
                    ask_depth: Some(7.5),
                },
                VenueSampleEntry {
                    venue_id: "bbb".into(),
                    mid: 100.2,
                    best_bid: 100.1,
                    best_ask: 100.3,
                    bid_depth: None,
                    ask_depth: None,
                },
            ],
        )
        .unwrap();
    let rows = store.spread_samples("BTC-USDC", 0, 2_000).unwrap();
    assert_eq!(rows.len(), 2);
    let aaa = rows.iter().find(|r| r.venue_id == "aaa").unwrap();
    assert_eq!(aaa.bid_depth, Some(5.0));
    assert_eq!(aaa.ask_depth, Some(7.5));
    let bbb = rows.iter().find(|r| r.venue_id == "bbb").unwrap();
    assert_eq!(bbb.bid_depth, None);
    assert_eq!(bbb.ask_depth, None);
    // Nonpositive/NaN depths normalize to NULL at write time.
    store
        .record_spread_samples(
            "BTC-USDC",
            2_000,
            &[VenueSampleEntry {
                venue_id: "aaa".into(),
                mid: 100.0,
                best_bid: 99.9,
                best_ask: 100.1,
                bid_depth: Some(0.0),
                ask_depth: Some(f64::NAN),
            }],
        )
        .unwrap();
    let rows = store.spread_samples("BTC-USDC", 1_500, 2_500).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].bid_depth, None);
    assert_eq!(rows[0].ask_depth, None);
}

#[test]
fn test_spread_samples_drop_invalid_and_normalize() {
    let store = StateStore::open_in_memory().unwrap();
    store
        .record_spread_samples(
            "BTC-USDC",
            1_000,
            &[
                sample("HyperLiquid", 100.0),
                sample("lighter", f64::NAN),
                sample("lighter", -5.0),
                sample("lighter", 101.0),
            ],
        )
        .unwrap();
    let rows = store.spread_samples("BTC-USDC", 0, i64::MAX).unwrap();
    assert_eq!(rows.len(), 2, "non-finite/non-positive mids are dropped");
    assert_eq!(rows[0].venue_id, "hyperliquid", "venue ids normalize");
    // Empty batches and negative timestamps are no-ops.
    store.record_spread_samples("BTC-USDC", 5_000, &[]).unwrap();
    store
        .record_spread_samples("BTC-USDC", -1, &[sample("lighter", 1.0)])
        .unwrap();
    assert_eq!(
        store.spread_samples("BTC-USDC", 0, i64::MAX).unwrap().len(),
        2
    );
}

#[test]
fn test_spread_samples_merge_background_and_ui_keys() {
    // Regression: the monitor + WS feed record under the canonical base
    // ("BTC") while UI polls record under the full symbol ("BTC-USDC").
    // Reads for either key must see both so unselected tickers keep a
    // fresh trailing window from background recording alone.
    let store = StateStore::open_in_memory().unwrap();
    let now = 1_700_000_000_000i64;
    store
        .record_spread_samples("BTC", now - 1_000, &[sample("hyperliquid", 100.0)])
        .unwrap();
    store
        .record_spread_samples("BTC-USDC", now - 60_000, &[sample("lighter", 101.0)])
        .unwrap();

    let via_ui = store.spread_samples("BTC-USDC", 0, i64::MAX).unwrap();
    assert_eq!(
        via_ui.len(),
        2,
        "UI key must see background base-keyed rows"
    );
    let via_base = store.spread_samples("BTC", 0, i64::MAX).unwrap();
    assert_eq!(via_base.len(), 2, "base key must see UI full-symbol rows");
    // Trailing-window slice sees the fresh background row.
    let windowed = store.spread_samples("BTC-USDC", now - 5_000, now).unwrap();
    assert_eq!(windowed.len(), 1);
    assert_eq!(windowed[0].venue_id, "hyperliquid");
    assert_eq!(
        store.earliest_spread_sample_ts("BTC-USDC").unwrap(),
        Some(now - 60_000)
    );
    assert_eq!(
        store.earliest_spread_sample_ts("BTC").unwrap(),
        Some(now - 60_000)
    );
}

#[test]
fn test_spread_samples_aliases_keep_symbols_isolated() {
    // Alias merging must never leak one asset's rows into another's.
    let store = StateStore::open_in_memory().unwrap();
    store
        .record_spread_samples("ETH", 1_000, &[sample("lighter", 50.0)])
        .unwrap();
    store
        .record_spread_samples("ETH-USDC", 2_000, &[sample("lighter", 51.0)])
        .unwrap();
    store
        .record_spread_samples("BTC", 1_500, &[sample("lighter", 100.0)])
        .unwrap();
    let rows = store.spread_samples("BTC-USDC", 0, i64::MAX).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].mid, 100.0);
    assert_eq!(
        store.earliest_spread_sample_ts("BTC-USDC").unwrap(),
        Some(1_500)
    );
}

#[test]
fn test_kv_roundtrip_and_retention_keys() {
    let store = StateStore::open_in_memory().unwrap();
    assert_eq!(store.kv_get("retention.max_days").unwrap(), None);
    store.kv_set("retention.max_days", "30").unwrap();
    store.kv_set("retention.max_gb", "2.5").unwrap();
    assert_eq!(
        store.kv_get("retention.max_days").unwrap(),
        Some("30".to_string())
    );
    assert_eq!(
        store.kv_get("retention.max_gb").unwrap(),
        Some("2.5".to_string())
    );
    store.kv_set("retention.max_days", "7").unwrap();
    assert_eq!(
        store.kv_get("retention.max_days").unwrap(),
        Some("7".to_string())
    );
    store.kv_delete("retention.max_days").unwrap();
    assert_eq!(store.kv_get("retention.max_days").unwrap(), None);
    assert!(store.kv_set("", "x").is_err());
}

#[test]
fn test_kv_survives_reopen() {
    let (dir, file) = temp_store_path("kv_reopen");
    {
        let store = StateStore::open(&file).unwrap();
        store.kv_set("retention.max_days", "14").unwrap();
        store.kv_set("retention.max_gb", "1.5").unwrap();
    }
    {
        let reopened = StateStore::open(&file).unwrap();
        assert_eq!(
            reopened.kv_get("retention.max_days").unwrap(),
            Some("14".to_string())
        );
        assert_eq!(
            reopened.kv_get("retention.max_gb").unwrap(),
            Some("1.5".to_string())
        );
        let version: i64 = reopened
            .conn
            .lock()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 29);
    }
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn test_retention_time_cutoff_deletes_oldest_first() {
    let store = StateStore::open_in_memory().unwrap();
    for i in 0..10 {
        store
            .record_spread_samples(
                "BTC-USDC",
                i * 1_000,
                &[sample("hyperliquid", 100.0 + i as f64)],
            )
            .unwrap();
    }
    let removed = store.delete_spread_samples_before(5_000).unwrap();
    assert_eq!(removed, 5);
    let rows = store.spread_samples("BTC-USDC", 0, i64::MAX).unwrap();
    assert_eq!(rows.len(), 5);
    assert_eq!(rows[0].ts_ms, 5_000);
}

#[test]
fn test_retention_cutoff_keeps_fresh_rows_across_alias_keys() {
    // Retention is purely time-based: fresh background ("BTC") and UI
    // ("BTC-USDC") rows must both survive a cutoff that drops old rows.
    let store = StateStore::open_in_memory().unwrap();
    store
        .record_spread_samples("BTC", 1_000, &[sample("hyperliquid", 100.0)])
        .unwrap();
    store
        .record_spread_samples("BTC-USDC", 2_000, &[sample("lighter", 101.0)])
        .unwrap();
    store
        .record_spread_samples("BTC", 9_000, &[sample("hyperliquid", 102.0)])
        .unwrap();
    store
        .record_spread_samples("BTC-USDC", 10_000, &[sample("lighter", 103.0)])
        .unwrap();
    let removed = store.delete_spread_samples_before(5_000).unwrap();
    assert_eq!(removed, 2);
    let rows = store.spread_samples("BTC-USDC", 0, i64::MAX).unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r.ts_ms >= 5_000));
}

#[test]
fn test_retention_oldest_first_batch_delete() {
    let store = StateStore::open_in_memory().unwrap();
    for i in 0..10 {
        store
            .record_spread_samples(
                "BTC-USDC",
                i * 1_000,
                &[sample("hyperliquid", 100.0 + i as f64)],
            )
            .unwrap();
    }
    let removed = store.delete_oldest_spread_samples(3).unwrap();
    assert_eq!(removed, 3);
    let rows = store.spread_samples("BTC-USDC", 0, i64::MAX).unwrap();
    assert_eq!(rows.len(), 7);
    assert_eq!(rows[0].ts_ms, 3_000);
    assert_eq!(store.spread_sample_count().unwrap(), 7);
    assert!(store.db_size_bytes().unwrap() > 0);
}

#[test]
fn test_spread_samples_prune_retention_and_cap() {
    let store = StateStore::open_in_memory().unwrap();
    for i in 0..10 {
        store
            .record_spread_samples(
                "BTC-USDC",
                i * 1_000,
                &[sample("hyperliquid", 100.0 + i as f64)],
            )
            .unwrap();
    }
    // Retention: drop everything before ts 5_000.
    store.prune_spread_samples(5_000, 1_000_000).unwrap();
    let rows = store.spread_samples("BTC-USDC", 0, i64::MAX).unwrap();
    assert_eq!(rows.len(), 5);
    assert_eq!(rows[0].ts_ms, 5_000);
    // Cap: keep only the newest 2 rows.
    store.prune_spread_samples(0, 2).unwrap();
    let rows = store.spread_samples("BTC-USDC", 0, i64::MAX).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].ts_ms, 8_000);
    assert_eq!(rows[1].ts_ms, 9_000);
}

/* --------- symbol-collision consistency filter (spread_samples) -------- */

/// Insert a sample row straight into the table, bypassing the write-time
/// incumbent gate — reproduces pollution persisted before the gate (and
/// read-side filter) existed, which is exactly what production DBs carry.
fn raw_sample(store: &StateStore, symbol: &str, venue: &str, ts_ms: i64, mid: f64) {
    store
        .conn
        .lock()
        .execute(
            "INSERT INTO spread_samples
                (symbol, venue_id, ts_ms, mid, best_bid, best_ask)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![symbol, venue, ts_ms, mid, mid - 0.01, mid + 0.01],
        )
        .unwrap();
}

#[test]
fn test_spread_samples_incumbent_survives_late_intruder() {
    // Regression for the observed SPX500 corruption: the key was born as an
    // index instrument (~7600) on vanta, then a same-ticker memecoin
    // (~0.5) flooded it from several venues at a far higher rate — rows
    // persisted before the write gate existed. The incumbent scale must
    // survive: quote frequency never steals identity.
    let store = StateStore::open_in_memory().unwrap();
    let b = 600_000i64; // one consistency bucket
                        // Incumbent history: six index-scale buckets from one venue.
    for i in 0..6 {
        raw_sample(&store, "SPX500", "vanta", i * b, 7_600.0 + i as f64);
    }
    // Intruder flood: memecoin-scale rows, three venues, many buckets —
    // already out-numbers the incumbent inside the read window.
    for i in 6..30 {
        raw_sample(&store, "SPX500", "vanta", i * b, 0.5);
        raw_sample(&store, "SPX500", "hyperliquid", i * b, 0.5);
        raw_sample(&store, "SPX500", "lighter", i * b, 0.5);
    }
    let rows = store.spread_samples("SPX500", 0, i64::MAX).unwrap();
    assert!(!rows.is_empty(), "incumbent rows must survive");
    assert!(
        rows.iter().all(|r| r.mid > 1_000.0),
        "only the index scale remains: {rows:?}"
    );
    // A window containing ONLY intruder data is an honest gap, not a
    // coherent-but-wrong memecoin series.
    let intruder_only = store.spread_samples("SPX500", 10 * b, 20 * b).unwrap();
    assert!(
        intruder_only.is_empty(),
        "window of pure off-scale data collapses to a gap"
    );
}

#[test]
fn test_spread_samples_majority_of_peers_beats_hot_venue() {
    // New key born with a hot corrupt feed in its first bucket: the three
    // corroborating index venues out-vote the memecoin feed inside the
    // incumbent cells even though it wrote the most rows — one venue, one
    // bucket, one vote.
    let store = StateStore::open_in_memory().unwrap();
    let b = 600_000i64;
    for i in 0..4 {
        raw_sample(&store, "SPX500", "vanta", i * b, 7_600.0);
        raw_sample(&store, "SPX500", "lighter", i * b, 7_601.0);
        raw_sample(&store, "SPX500", "okx", i * b, 7_599.0);
        // The hot corrupt venue wrote far more rows per bucket.
        for k in 0..20 {
            raw_sample(&store, "SPX500", "hyperliquid", i * b + k, 0.5);
        }
    }
    let rows = store.spread_samples("SPX500", 0, i64::MAX).unwrap();
    assert_eq!(rows.len(), 12, "only the three index venues survive");
    assert!(rows.iter().all(|r| r.mid > 1_000.0));
    assert!(rows.iter().all(|r| r.venue_id != "hyperliquid"));
}

#[test]
fn test_spread_samples_write_gate_rejects_off_scale() {
    // Once a key has an incumbent scale, the write path itself refuses
    // irreconcilable mids — pollution stops at the boundary, not at read.
    let store = StateStore::open_in_memory().unwrap();
    let b = 600_000i64;
    for i in 0..3 {
        store
            .record_spread_samples("IDX", i * b, &[sample("vanta", 7_600.0)])
            .unwrap();
    }
    store
        .record_spread_samples("IDX", 3 * b, &[sample("binance", 0.5)])
        .unwrap();
    let rows = store.spread_samples("IDX", 0, i64::MAX).unwrap();
    assert_eq!(rows.len(), 3, "off-scale write was rejected");
    assert!(rows.iter().all(|r| r.mid > 1_000.0));
    // On-scale writes still land.
    store
        .record_spread_samples("IDX", 4 * b, &[sample("binance", 7_610.0)])
        .unwrap();
    assert_eq!(store.spread_samples("IDX", 0, i64::MAX).unwrap().len(), 4);
}

#[test]
fn test_spread_samples_single_venue_mixed_splits_per_bucket() {
    // One venue quoting both scales (vanta's SPX500 flip): on-scale cells
    // survive, collided rows in the same or other buckets drop — the venue
    // is not smeared into one window median and not excluded wholesale.
    let store = StateStore::open_in_memory().unwrap();
    let b = 600_000i64;
    for i in 0..4 {
        store
            .record_spread_samples("IDX2", i * b, &[sample("vanta", 7_600.0)])
            .unwrap();
    }
    // Same venue flips to memecoin scale for two buckets, then back
    // (raw inserts — the write gate would rightly refuse these today).
    for i in [4, 5] {
        raw_sample(&store, "IDX2", "vanta", i * b, 0.5);
    }
    for i in 6..8 {
        store
            .record_spread_samples("IDX2", i * b, &[sample("vanta", 7_605.0)])
            .unwrap();
    }
    let rows = store.spread_samples("IDX2", 0, i64::MAX).unwrap();
    assert_eq!(rows.len(), 6);
    assert!(rows.iter().all(|r| r.mid > 1_000.0));
    assert!(rows.iter().all(|r| r.ts_ms / b != 4 && r.ts_ms / b != 5));
}

#[test]
fn test_spread_samples_repriced_together_is_not_corruption() {
    // Consensus is relative: every venue repricing 2x together stays
    // consistent — the filter arbitrates SCALE SPLITS, not volatility.
    let store = StateStore::open_in_memory().unwrap();
    let b = 600_000i64;
    for i in 0..3 {
        store
            .record_spread_samples("VOL", i * b, &[sample("aaa", 100.0), sample("bbb", 100.5)])
            .unwrap();
    }
    // Hmm — a genuine 2x reprice inside the incumbent window: cells at
    // 100 and 200 are within 3x of each other, so both corroborate.
    for i in 3..6 {
        store
            .record_spread_samples("VOL", i * b, &[sample("aaa", 210.0), sample("bbb", 209.0)])
            .unwrap();
    }
    let rows = store.spread_samples("VOL", 0, i64::MAX).unwrap();
    assert_eq!(rows.len(), 12, "mutual reprice within 3x is kept");
}

/* ------------------ signal-event audit log (V9) ------------------ */

#[test]
fn test_signal_events_append_readback_and_validation() {
    let store = StateStore::open_in_memory().unwrap();
    store
        .record_monitor_signal_event(
            "BTC",
            "HyperLiquid",
            "lighter",
            60_000,
            None,
            "preparing",
            Some(12.5),
            Some(3.0),
            Some(0.9),
            Some(r#"{"friction_bps":5.0}"#),
            "transition",
        )
        .unwrap();
    store
        .record_monitor_signal_event(
            "BTC",
            "hyperliquid",
            "lighter",
            120_000,
            Some("preparing"),
            "triggered",
            None,
            None,
            None,
            None,
            "transition",
        )
        .unwrap();
    store
        .record_monitor_signal_event(
            "BTC",
            "lighter",
            "hyperliquid",
            120_000,
            None,
            "tracking",
            Some(1.0),
            None,
            None,
            Some("{}"),
            "transition",
        )
        .unwrap();

    // Pair-filtered readback, newest first; venue ids normalize.
    let rows = store
        .recent_monitor_signal_events(Some("BTC"), Some("hyperliquid"), Some("lighter"), 10)
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].ts_ms, 120_000);
    assert_eq!(rows[0].old_status.as_deref(), Some("preparing"));
    assert_eq!(rows[0].new_status, "triggered");
    assert_eq!(rows[1].old_status, None);
    assert_eq!(
        rows[1].economics_json.as_deref(),
        Some(r#"{"friction_bps":5.0}"#)
    );
    assert_eq!(rows[1].buy_venue, "hyperliquid");
    // Full UI symbols resolve to the canonical-base storage key: the monitor
    // records under "BTC" while pages query "BTC-USDC" — both must match.
    let alias_rows = store
        .recent_monitor_signal_events(Some("BTC-USDC"), None, None, 10)
        .unwrap();
    assert_eq!(alias_rows.len(), 3);

    // Unfiltered sees all three; limit bounds the read.
    assert_eq!(
        store
            .recent_monitor_signal_events(None, None, None, 10)
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        store
            .recent_monitor_signal_events(None, None, None, 2)
            .unwrap()
            .len(),
        2
    );
    assert_eq!(store.monitor_signal_event_count().unwrap(), 3);

    // Every invalid append is rejected and stores nothing new.
    assert!(store
        .record_monitor_signal_event(
            "BTC",
            "hyperliquid",
            "lighter",
            -1,
            None,
            "triggered",
            None,
            None,
            None,
            None,
            "transition",
        )
        .is_err());
    assert!(store
        .record_monitor_signal_event(
            "BTC",
            "hyperliquid",
            "lighter",
            1,
            None,
            "bogus",
            None,
            None,
            None,
            None,
            "transition",
        )
        .is_err());
    assert!(store
        .record_monitor_signal_event(
            "BTC",
            "hyperliquid",
            "lighter",
            1,
            Some("bogus"),
            "triggered",
            None,
            None,
            None,
            None,
            "transition",
        )
        .is_err());
    assert!(store
        .record_monitor_signal_event(
            "BTC",
            "hyperliquid",
            "lighter",
            1,
            None,
            "triggered",
            None,
            None,
            None,
            None,
            "pulse",
        )
        .is_err());
    assert!(store
        .record_monitor_signal_event(
            "BTC",
            "hyperliquid",
            "lighter",
            1,
            None,
            "triggered",
            Some(f64::NAN),
            None,
            None,
            None,
            "transition",
        )
        .is_err());
    assert!(store
        .record_monitor_signal_event(
            "BTC",
            "hyperliquid",
            "lighter",
            1,
            None,
            "triggered",
            None,
            None,
            Some(1.5),
            None,
            "transition",
        )
        .is_err());
    assert!(store
        .record_monitor_signal_event(
            "BTC",
            "hyperliquid",
            "lighter",
            1,
            None,
            "triggered",
            None,
            None,
            None,
            Some("{nope"),
            "transition",
        )
        .is_err());
    assert_eq!(store.monitor_signal_event_count().unwrap(), 3);
    assert!(store
        .recent_monitor_signal_events(None, None, None, 0)
        .is_err());
}

#[test]
fn test_signal_events_prune_retention_and_cap() {
    let store = StateStore::open_in_memory().unwrap();
    for i in 0..6 {
        store
            .record_monitor_signal_event(
                "BTC",
                "hyperliquid",
                "lighter",
                i * 1_000,
                None,
                "tracking",
                None,
                None,
                None,
                None,
                "transition",
            )
            .unwrap();
    }
    // Same-days time cutoff drops old audit rows first.
    store
        .prune_monitor_signal_events(3_000, MONITOR_SIGNAL_EVENTS_MAX_ROWS)
        .unwrap();
    let rows = store
        .recent_monitor_signal_events(None, None, None, 100)
        .unwrap();
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().all(|r| r.ts_ms >= 3_000));
    // Separate cap keeps only the newest rows.
    store.prune_monitor_signal_events(0, 2).unwrap();
    let rows = store
        .recent_monitor_signal_events(None, None, None, 100)
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].ts_ms, 5_000);
    assert_eq!(rows[1].ts_ms, 4_000);
}

/* ------------------ session seasonality (V8) ------------------ */

#[test]
fn test_migration_v20_rescales_lighter_funding_history() {
    // Pre-fix rows: Lighter-family 8h prints persisted with interval 1
    // (8x inflated rate_8h). v20 rescales exactly those rows; other
    // venues and already-correct rows pass through untouched.
    let conn = Connection::open_in_memory().unwrap();
    for batch in [
        MIGRATION_V1_SQL,
        MIGRATION_V2_SQL,
        MIGRATION_V3_SQL,
        MIGRATION_V4_SQL,
        MIGRATION_V5_SQL,
        MIGRATION_V6_SQL,
        MIGRATION_V7_SQL,
        MIGRATION_V8_SQL,
        MIGRATION_V9_SQL,
        MIGRATION_V10_SQL,
        MIGRATION_V11_SQL,
        MIGRATION_V12_SQL,
        MIGRATION_V13_SQL,
        MIGRATION_V14_SQL,
        MIGRATION_V15_SQL,
        MIGRATION_V16_SQL,
        MIGRATION_V17_SQL,
        MIGRATION_V18_SQL,
        MIGRATION_V19_SQL,
    ] {
        conn.execute_batch(batch).unwrap();
    }
    conn.execute(
        "INSERT INTO funding_rate_samples (venue_id, symbol, ts_ms, rate_8h, interval_hours) VALUES
             ('lighter', 'LINK-USDC', 1, -0.002688, 1),
             ('robinhood', 'LINK-USDC', 1, 0.000768, 1),
             ('hyperliquid', 'LINK-USDC', 1, 0.0001, 1),
             ('lighter', 'LINK-USDC', 2, -0.000336, 8)",
        [],
    )
    .unwrap();
    StateStore::migrate(&conn).unwrap();
    let v: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, 29);

    let mut stmt = conn
            .prepare("SELECT venue_id, rate_8h, interval_hours FROM funding_rate_samples ORDER BY venue_id, ts_ms")
            .unwrap();
    let rows: Vec<(String, f64, i64)> = stmt
        .query_map([], |r| {
            Ok((r.get(0).unwrap(), r.get(1).unwrap(), r.get(2).unwrap()))
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(rows.len(), 4);
    let approx = |a: f64, b: f64| (a - b).abs() < 1e-12;
    // Untouched venue keeps its print and interval.
    assert_eq!(rows[0].0, "hyperliquid");
    assert!(approx(rows[0].1, 0.0001));
    assert_eq!(rows[0].2, 1);
    // Buggy-era Lighter rows rescale exactly 8x to the true 8h print.
    for row in rows.iter().filter(|r| r.0 == "lighter") {
        assert!(approx(row.1, -0.000336), "lighter row: {row:?}");
        assert_eq!(row.2, 8);
    }
    // Robinhood shares the Lighter parser, same repair.
    let rh = rows.iter().find(|r| r.0 == "robinhood").unwrap();
    assert!(approx(rh.1, 0.000096), "robinhood row: {rh:?}");
    assert_eq!(rh.2, 8);
}

#[test]
fn execution_log_insert_load_round_trip() {
    let (dir, file) = temp_store_path("execlog");
    let store = StateStore::open(&file).unwrap();
    let mk = |id: &str, ok: bool| ExecutionLogEntry {
        id: id.to_string(),
        timestamp: "2026-09-20T00:00:00Z".to_string(),
        trade_type: "Funding".to_string(),
        strategy_id: "funding".to_string(),
        symbol: "NEAR-USDC".to_string(),
        venue_a: "vanta".to_string(),
        venue_b: "lighter".to_string(),
        leg_a_status: if ok {
            "filled".into()
        } else {
            "blocked".into()
        },
        leg_b_status: if ok {
            "filled".into()
        } else {
            "blocked".into()
        },
        success: ok,
        gross_profit: 0.0,
        fees: 1.0,
        net_profit: -1.0,
        latency_us: 7,
        note: if ok {
            "ok".into()
        } else {
            "Execution blocked: test reason".into()
        },
        direction: None,
        trigger_spread_bps: Some(61.1),
        baseline_mean_spread_bps: None,
        position_step: None,
        max_units: None,
        nominal_hurdle_bps: Some(8.0),
        is_live: false,
    };
    store.insert_execution_log(&mk("a", true)).unwrap();
    store.insert_execution_log(&mk("b", false)).unwrap();
    // REPLACE semantics: same id overwrites, never duplicates.
    store.insert_execution_log(&mk("b", true)).unwrap();
    let rows = store.load_execution_logs(500).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].id, "b");
    assert!(rows[0].success);
    assert_eq!(rows[0].trigger_spread_bps, Some(61.1));
    assert_eq!(rows[1].id, "a");
    assert_eq!(rows[1].note, "ok");
    // Limit respected (newest-first).
    assert_eq!(store.load_execution_logs(1).unwrap().len(), 1);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn test_migration_v7_to_v8_preserves_data() {
    let conn = Connection::open_in_memory().unwrap();
    for batch in [
        MIGRATION_V1_SQL,
        MIGRATION_V2_SQL,
        MIGRATION_V3_SQL,
        MIGRATION_V4_SQL,
        MIGRATION_V5_SQL,
        MIGRATION_V6_SQL,
        MIGRATION_V7_SQL,
    ] {
        conn.execute_batch(batch).unwrap();
    }
    let v7: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v7, 7);
    conn.execute(
        "INSERT INTO spread_samples (symbol, venue_id, ts_ms, mid, best_bid, best_ask)
             VALUES ('BTC', 'hyperliquid', 1000, 100.0, 99.9, 100.1)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO monitor_signals
                 (symbol, buy_venue, sell_venue, status, first_seen_ms, last_seen_ms, updated_at)
             VALUES ('BTC', 'hyperliquid', 'lighter', 'watching', 1, 2, 't')",
        [],
    )
    .unwrap();
    StateStore::migrate(&conn).unwrap();
    let v10: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v10, 29);
    // v19 remaps the renamed lifecycle statuses on the way through.
    let remapped: String = conn
        .query_row(
            "SELECT status FROM monitor_signals WHERE symbol = 'BTC'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(remapped, "tracking");
    let kept: i64 = conn
        .query_row("SELECT COUNT(*) FROM spread_samples", [], |r| r.get(0))
        .unwrap();
    assert_eq!(kept, 1, "V7 rows must survive the V8..V18 migration");
    let kept_signals: i64 = conn
        .query_row("SELECT COUNT(*) FROM monitor_signals", [], |r| r.get(0))
        .unwrap();
    assert_eq!(kept_signals, 1, "V5 lifecycle rows must survive");
    for table in [
        "venue_volume_samples",
        "session_profiles",
        "session_hour_bins",
        "monitor_signal_events",
        "radar_bot_config",
        "radar_bot_positions",
        "har_anchors",
        "promotion_state",
        "autotune_apply_events",
        "spatial_entry_baseline",
        "paper_equity_expectations",
        "account_equity_events",
        "funding_rate_samples",
    ] {
        let found: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [table],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(found, 1, "migration must create {table}");
    }
}

#[test]
fn test_volume_sampler_roundtrip_and_alias() {
    use std::collections::HashMap;
    let store = StateStore::open_in_memory().unwrap();
    let mut vols = HashMap::new();
    vols.insert("BTCUSDT".to_string(), 1_000_000.0);
    vols.insert("ETHUSDT".to_string(), 500_000.0);
    vols.insert("JUNK".to_string(), f64::NAN);
    vols.insert("ZERO".to_string(), 0.0);
    store
        .record_venue_volume_samples("hyperliquid", 60_000, &vols)
        .unwrap();
    // Canonical-base storage reads back through quoted aliases.
    let btc = store
        .venue_volume_series("BTC-USDC", "hyperliquid", 0, i64::MAX)
        .unwrap();
    assert_eq!(btc, vec![(60_000, 1_000_000.0)]);
    let eth = store
        .venue_volume_series("eth", "hyperliquid", 0, i64::MAX)
        .unwrap();
    assert_eq!(eth.len(), 1);
    // Non-finite / non-positive rows are never persisted.
    let all: i64 = store
        .conn
        .lock()
        .query_row("SELECT COUNT(*) FROM venue_volume_samples", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(all, 2);
    // Retention bounds the raw log.
    store.prune_venue_volume_samples(60_000, 1_000_000).unwrap();
    let kept: i64 = store
        .conn
        .lock()
        .query_row("SELECT COUNT(*) FROM venue_volume_samples", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(kept, 2);
    store.prune_venue_volume_samples(60_001, 1_000_000).unwrap();
    let kept: i64 = store
        .conn
        .lock()
        .query_row("SELECT COUNT(*) FROM venue_volume_samples", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(kept, 0);
}

/// Frozen sixteen-day fixture: hourly two-venue polls, Thursday 2026-01-15
/// 00:00 UTC onward. Spreads alternate between a session peak and a
/// 7bps revert level so every session segments winning episodes. Peaks
/// land on even poll indexes, so a session with H hours calibrates
/// H/2 episodes per day.
fn seed_session_history(store: &StateStore, days: i64) -> (i64, i64) {
    use chrono::TimeZone;
    let base = chrono::Utc
        .with_ymd_and_hms(2026, 1, 15, 0, 0, 0)
        .single()
        .unwrap()
        .timestamp_millis();
    let peak_for_hour = |h: i64| match h % 24 {
        0..=7 => 4.0,
        8..=12 => 12.0,
        13..=16 => 22.0,
        17..=21 => 14.0,
        _ => 3.0,
    };
    for i in 0..(days * 24) {
        let ts = base + i * 3_600_000 + 60_000;
        let spread_bps = if i % 2 == 0 { peak_for_hour(i) } else { 7.0 };
        store
            .record_spread_samples(
                "BTC",
                ts,
                &[
                    sample("hyperliquid", 100.0),
                    sample("lighter", 100.0 * (1.0 + spread_bps / 10_000.0)),
                ],
            )
            .unwrap();
    }
    (base, base + days * 24 * 3_600_000)
}

#[test]
fn test_rollup_seeded_multi_day_history() {
    let store = StateStore::open_in_memory().unwrap();
    let (from, to) = seed_session_history(&store, 16);
    let written = store
        .rollup_session_profiles("BTC-USDC", from - 1_000, to + 1_000, to)
        .unwrap();
    assert_eq!(written, 5, "one row per session");
    let profiles = store.session_profiles("BTC").unwrap();
    assert_eq!(profiles.len(), 5);
    let by_session: std::collections::HashMap<&str, &SessionProfileRow> = profiles
        .iter()
        .map(|r| (r.session_id.as_str(), r))
        .collect();
    // Overlap carries 32 episodes: calibrated, not provisional.
    let overlap = by_session["overlap"];
    assert!(
        overlap.n_episodes >= 30,
        "overlap episodes: {}",
        overlap.n_episodes
    );
    assert!(!overlap.provisional);
    assert!(overlap.med_abs_spread_bps.unwrap() > 0.0);
    assert!(
        overlap.win_rate.unwrap() >= 0.9,
        "alternating fixture must mostly revert, got {:?}",
        overlap.win_rate
    );
    assert!(overlap.mean_hold_s.unwrap() > 0.0);
    assert!(overlap.rv_bps.unwrap() > 0.0);
    // Dead zone carries 16 episodes: provisional, never fabricated.
    let dead = by_session["dead"];
    assert!(dead.n_episodes < 30);
    assert!(dead.provisional);
    // Hour bins: 24 rows, modulators inside +/-25%, provisional reads 1.0.
    let bins = store.session_hour_bins("BTC-USDC").unwrap();
    assert_eq!(bins.len(), 24);
    for b in &bins {
        assert!(
            (0.75..=1.25).contains(&b.modulator),
            "hour {} modulator {}",
            b.hour,
            b.modulator
        );
        if b.provisional {
            assert_eq!(b.modulator, 1.0);
        }
    }
    let peak_bin = bins.iter().find(|b| b.hour == 14).unwrap();
    assert!(!peak_bin.provisional);
    assert!(peak_bin.med_spread_bps.unwrap() > 0.0);
}

#[test]
fn test_rollup_thin_data_stays_provisional() {
    let store = StateStore::open_in_memory().unwrap();
    use chrono::TimeZone;
    let ts = chrono::Utc
        .with_ymd_and_hms(2026, 1, 15, 14, 5, 0)
        .single()
        .unwrap()
        .timestamp_millis();
    store
        .record_spread_samples(
            "BTC",
            ts,
            &[sample("hyperliquid", 100.0), sample("lighter", 100.5)],
        )
        .unwrap();
    let written = store
        .rollup_session_profiles("BTC", ts - 1_000, ts + 1_000, ts)
        .unwrap();
    assert_eq!(written, 5);
    for row in store.session_profiles("BTC").unwrap() {
        assert!(
            row.provisional,
            "single-tick history must stay provisional: {:?}",
            row.session_id
        );
    }
}

#[test]
fn test_prune_keeps_session_profiles() {
    let store = StateStore::open_in_memory().unwrap();
    let (from, to) = seed_session_history(&store, 16);
    store
        .rollup_session_profiles("BTC", from - 1_000, to + 1_000, to)
        .unwrap();
    assert_eq!(store.session_profiles("BTC").unwrap().len(), 5);
    // Nuke the entire raw log: time cutoff plus zero-row cap.
    store.prune_spread_samples(i64::MAX, 0).unwrap();
    assert_eq!(store.spread_sample_count().unwrap(), 0);
    assert_eq!(store.delete_spread_samples_before(i64::MAX).unwrap_or(0), 0);
    // Derived state survives every raw-sample prune path.
    assert_eq!(store.session_profiles("BTC").unwrap().len(), 5);
    assert_eq!(store.session_hour_bins("BTC").unwrap().len(), 24);
}

/* ------------------ open-position aggregation ------------------ */

#[allow(clippy::too_many_arguments)]
fn filled_two_leg(
    store: &StateStore,
    id: &str,
    symbol: &str,
    strategy: &str,
    long_venue: &str,
    short_venue: &str,
    qty: f64,
    long_px: f64,
    short_px: f64,
) {
    let mut sig = two_leg_signal(id, symbol, None, None);
    sig.strategy_id = strategy.to_string();
    sig.legs[0].venue = long_venue.to_string();
    sig.legs[1].venue = short_venue.to_string();
    sig.legs[0].intended_qty = qty;
    sig.legs[1].intended_qty = qty;
    store.record_signal_execution(&sig).unwrap();
    store
        .apply_leg_fill(id, LegSide::Long, qty, long_px, 0.5, None)
        .unwrap();
    store
        .apply_leg_fill(id, LegSide::Short, qty, short_px, 0.5, None)
        .unwrap();
    store
        .update_signal_status(id, SignalStatus::Balanced, None)
        .unwrap();
}

#[test]
fn test_unknown_leg_side_fails_closed_instead_of_defaulting_long() {
    let store = StateStore::open_in_memory().unwrap();
    let sig = two_leg_signal("sig_bad_side", "BTC-USDC", None, None);
    store.record_signal_execution(&sig).unwrap();
    {
        let conn = store.conn.lock();
        conn.execute_batch("PRAGMA ignore_check_constraints = ON;")
            .unwrap();
        conn.execute(
            "UPDATE signal_legs SET side = 'sideways' WHERE signal_id = ?1",
            [&sig.id],
        )
        .unwrap();
    }
    assert!(store.get_signal(&sig.id).is_err());
}

#[test]
fn test_aggregate_open_positions_groups_by_venue_pair() {
    let store = StateStore::open_in_memory().unwrap();
    // BTC traded twice on hyperliquid-vs-lighter, once on
    // hyperliquid-vs-arcus: two cards, first aggregates both signals.
    filled_two_leg(
        &store,
        "s1",
        "BTC-USDC",
        "futures",
        "hyperliquid",
        "lighter",
        1.0,
        60000.0,
        60100.0,
    );
    filled_two_leg(
        &store,
        "s2",
        "BTC-USDC",
        "futures",
        "hyperliquid",
        "lighter",
        0.5,
        60000.0,
        60100.0,
    );
    filled_two_leg(
        &store,
        "s3",
        "BTC-USDC",
        "futures",
        "hyperliquid",
        "arcus",
        1.0,
        60000.0,
        60100.0,
    );

    let signals = store.list_signals(None, None).unwrap();
    let positions = aggregate_open_positions(&signals, &|_, _| None);
    assert_eq!(positions.len(), 2, "A-vs-B merges, A-vs-C stays separate");

    let hl_lt = positions
        .iter()
        .find(|p| p.venues.len() == 2 && p.venues[1].venue == "lighter")
        .unwrap();
    assert_eq!(hl_lt.signal_count, 2);
    assert_eq!(hl_lt.symbol, "BTC-USDC");
    // Long row on top (+1.5 on hyperliquid), short row below.
    assert_eq!(hl_lt.venues[0].venue, "hyperliquid");
    assert!((hl_lt.venues[0].net_qty - 1.5).abs() < 1e-9);
    assert_eq!(hl_lt.venues[1].venue, "lighter");
    assert!((hl_lt.venues[1].net_qty + 1.5).abs() < 1e-9);
    // No marks supplied -> PnL honestly unavailable, not fabricated zero.
    assert!(hl_lt.unrealized_pnl_usd.is_none());
    assert!(hl_lt.notional_usd.is_some());
    assert_eq!(hl_lt.expected_net_usd, Some(25.0));
}

#[test]
fn test_aggregate_open_positions_marks_to_market_and_filters() {
    let store = StateStore::open_in_memory().unwrap();
    filled_two_leg(
        &store,
        "s1",
        "BTC-USDC",
        "futures",
        "hyperliquid",
        "lighter",
        1.0,
        100.0,
        105.0,
    );
    filled_two_leg(
        &store,
        "s2",
        "ETH-USDC",
        "funding",
        "hyperliquid",
        "lighter",
        2.0,
        10.0,
        10.0,
    );
    // A closed signal must not contribute.
    filled_two_leg(
        &store,
        "s3",
        "SOL-USDC",
        "futures",
        "hyperliquid",
        "lighter",
        5.0,
        1.0,
        1.0,
    );
    store
        .update_signal_status("s3", SignalStatus::Closed, None)
        .unwrap();

    let signals = store.list_signals(None, None).unwrap();
    // Marks: both venues at 110 -> long +10, short loses (110-105)*1 = -5.
    let positions = aggregate_open_positions(&signals, &|_, _| Some(110.0));
    assert_eq!(positions.len(), 2, "closed signals drop out");

    let btc = positions.iter().find(|p| p.symbol == "BTC-USDC").unwrap();
    // gross = +10 - 5 = 5; minus fees (2 legs * 0.5 = 1.0) => 4.0
    let pnl = btc.unrealized_pnl_usd.expect("all legs markable");
    assert!((pnl - 4.0).abs() < 1e-9, "unexpected pnl {pnl}");

    // Strategy scoping: the funding signal is a separate group.
    let funding: Vec<_> = positions
        .iter()
        .filter(|p| p.strategy_id == "funding")
        .collect();
    assert_eq!(funding.len(), 1);
    assert_eq!(funding[0].symbol, "ETH-USDC");
}

#[test]
fn test_aggregate_open_positions_partial_flatten_nets_residual() {
    let store = StateStore::open_in_memory().unwrap();
    filled_two_leg(
        &store,
        "s1",
        "BTC-USDC",
        "futures",
        "hyperliquid",
        "lighter",
        1.0,
        100.0,
        100.0,
    );
    // Partial flatten child: opposite-side legs on the same venues,
    // still open (unresolved) -> nets the group to residual 0.4.
    let mut child = two_leg_signal("flat-s1-0001", "BTC-USDC", None, None);
    child.legs[0].side = LegSide::Short;
    child.legs[1].side = LegSide::Long;
    child.legs[0].intended_qty = 0.6;
    child.legs[1].intended_qty = 0.6;
    store.record_signal_execution(&child).unwrap();
    store
        .apply_leg_fill("flat-s1-0001", LegSide::Short, 0.6, 100.0, 0.0, None)
        .unwrap();
    store
        .apply_leg_fill("flat-s1-0001", LegSide::Long, 0.6, 100.0, 0.0, None)
        .unwrap();
    store
        .update_signal_status("flat-s1-0001", SignalStatus::Unresolved, None)
        .unwrap();

    let signals = store.list_signals(None, None).unwrap();
    let positions = aggregate_open_positions(&signals, &|_, _| None);
    assert_eq!(positions.len(), 1, "flatten child joins the same group");
    let p = &positions[0];
    assert!((p.venues[0].net_qty - 0.4).abs() < 1e-9);
    assert!((p.venues[1].net_qty + 0.4).abs() < 1e-9);
    assert_eq!(p.signal_count, 2);
}

#[test]
fn test_aggregate_open_positions_single_leg_flatten_child_merges() {
    // The canary shape: a residual flatten child carries ONLY the residual
    // venue's leg — under venue-set grouping it formed its own zero card.
    let store = StateStore::open_in_memory().unwrap();
    filled_two_leg(
        &store,
        "s1",
        "ETH-USDC",
        "futures",
        "hyperliquid",
        "lighter",
        0.005,
        2700.0,
        2700.0,
    );
    let mut child = two_leg_signal("flat-s1-a1b2", "ETH-USDC", None, None);
    child.legs.truncate(1); // keep only the first (hyperliquid) leg…
    child.legs[0].venue = "lighter".into(); // …retargeted to the residual venue
    child.legs[0].side = LegSide::Long; // close the parent's short
    child.legs[0].intended_qty = 0.005;
    store.record_signal_execution(&child).unwrap();
    store
        .apply_leg_fill("flat-s1-a1b2", LegSide::Long, 0.005, 2700.0, 0.0, None)
        .unwrap();
    store
        .update_signal_status("flat-s1-a1b2", SignalStatus::Closed, None)
        .unwrap();

    let signals = store.list_signals(None, None).unwrap();
    let positions = aggregate_open_positions(&signals, &|_, _| None);
    // The single-leg child must merge into the parent's group — the canary
    // bug rendered it as a second card showing a fabricated +0 position.
    assert_eq!(
        positions.len(),
        1,
        "single-leg flatten child must join the parent's position group"
    );
    let p = &positions[0];
    // Parent lighter short (−0.005) + child lighter long (+0.005) → flat.
    let lgt = p.venues.iter().find(|v| v.venue == "lighter");
    assert!(lgt.map(|v| v.net_qty.abs() < 1e-9).unwrap_or(true));
    assert_eq!(p.signal_count, 2);
}

/* ------------------ seasonality Phase 2 + autotune apply (V12) -------- */

fn har_row() -> HarAnchorRow {
    HarAnchorRow {
        symbol: "BTC-USDC".to_string(),
        intercept: 0.2,
        b_daily: 0.5,
        b_weekly: 0.3,
        b_monthly: 0.15,
        r2: 0.92,
        n_days: 60,
        forecast_rv_bps: 11.0,
        trailing_mean_rv_bps: 10.0,
        updated_ts: 7_000,
    }
}

#[test]
fn test_phase2_har_anchor_roundtrip_and_validation() {
    let store = StateStore::open_in_memory().unwrap();
    assert_eq!(store.phase2_har_anchor("BTC-USDC").unwrap(), None);
    store.phase2_upsert_har_anchor(&har_row()).unwrap();
    let got = store.phase2_har_anchor("BTC").unwrap().expect("alias read");
    assert!((got.forecast_rv_bps - 11.0).abs() < 1e-12);
    let mut bad = har_row();
    bad.r2 = 1.5;
    assert!(store.phase2_upsert_har_anchor(&bad).is_err());
    let mut nan = har_row();
    nan.b_daily = f64::NAN;
    assert!(store.phase2_upsert_har_anchor(&nan).is_err());
    let mut neg = har_row();
    neg.forecast_rv_bps = -1.0;
    assert!(store.phase2_upsert_har_anchor(&neg).is_err());
}

#[test]
fn test_phase2_promotion_state_roundtrip_and_validation() {
    let store = StateStore::open_in_memory().unwrap();
    assert_eq!(store.phase2_promotion_state("ETH-USDC").unwrap(), None);
    store
        .phase2_upsert_promotion_state(&PromotionStateRow {
            symbol: "ETH-USDC".to_string(),
            status: "promoted".to_string(),
            fit_delta_bps: 2.0,
            val_delta_bps: 1.5,
            reason: "agree".to_string(),
            updated_ts: 9_000,
        })
        .unwrap();
    let got = store
        .phase2_promotion_state("ETH")
        .unwrap()
        .expect("alias read");
    assert_eq!(got.status, "promoted");
    assert!(store
        .phase2_upsert_promotion_state(&PromotionStateRow {
            symbol: "ETH-USDC".to_string(),
            status: "live".to_string(),
            fit_delta_bps: 2.0,
            val_delta_bps: 1.5,
            reason: "agree".to_string(),
            updated_ts: 9_000,
        })
        .is_err());
}

#[test]
fn test_autotune_apply_events_audit_roundtrip() {
    let store = StateStore::open_in_memory().unwrap();
    assert!(store
        .recent_autotune_apply_events("BTC-USDC", 10)
        .unwrap()
        .is_empty());
    let id = store
        .record_autotune_apply_event(
            "BTC-USDC",
            r#"{"targetProfitBps": 8.5}"#,
            r#"{"span": [1, 2], "fit": "ok"}"#,
            12_000,
        )
        .unwrap();
    assert!(id > 0);
    let rows = store.recent_autotune_apply_events("BTC", 10).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].applied_json, r#"{"targetProfitBps": 8.5}"#);
    assert!(store
        .record_autotune_apply_event("BTC-USDC", "not json", "{}", 1)
        .is_err());
    assert!(store.recent_autotune_apply_events("BTC-USDC", 0).is_err());
}

#[test]
fn test_max_leverage_validation_bounds() {
    let store = StateStore::open_in_memory().unwrap();
    assert!(store
        .upsert_account(
            "a1",
            "hyperliquid",
            "Main",
            true,
            false,
            "standard",
            Some(0),
            None
        )
        .is_err());
    assert!(store
        .upsert_account(
            "a1",
            "hyperliquid",
            "Main",
            true,
            false,
            "standard",
            Some(6),
            None
        )
        .is_err());
    assert!(store
        .upsert_account(
            "a1",
            "hyperliquid",
            "Main",
            true,
            false,
            "standard",
            Some(99),
            None
        )
        .is_err());
    for ok in [None, Some(1), Some(3), Some(5)] {
        store
            .upsert_account(
                "a1",
                "hyperliquid",
                "Main",
                true,
                false,
                "standard",
                ok,
                None,
            )
            .unwrap();
        let got = store.get_account("a1").unwrap().expect("row");
        assert_eq!(got.max_leverage, ok);
    }
}

#[test]
fn test_max_leverage_roundtrip_update_and_clear() {
    let store = StateStore::open_in_memory().unwrap();
    let created = store
        .upsert_account(
            "a1",
            "lighter",
            "Main",
            true,
            true,
            "standard",
            Some(2),
            None,
        )
        .unwrap();
    assert_eq!(created.max_leverage, Some(2));
    let listed = store.list_accounts(Some("lighter")).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].max_leverage, Some(2));
    let cleared = store
        .upsert_account("a1", "lighter", "Main", true, true, "standard", None, None)
        .unwrap();
    assert_eq!(cleared.max_leverage, None);
    let fetched = store.get_account("a1").unwrap().expect("row");
    assert_eq!(fetched.max_leverage, None);
    assert!(fetched.is_default);
}

#[test]
fn test_equity_expectation_baseline_init_then_quiet() {
    let store = StateStore::open_in_memory().unwrap();
    assert_eq!(store.get_equity_expectation("hyperliquid").unwrap(), None);
    let first = store
        .check_account_equity("hyperliquid", 100_000.0, "", 1_000)
        .unwrap();
    assert_eq!(first.event, None);
    assert!(first.note.is_some(), "init must leave a coverage note");
    let (expected, sig, poll) = store
        .get_equity_expectation("hyperliquid")
        .unwrap()
        .expect("baseline stored");
    assert_eq!(expected, 100_000.0);
    assert_eq!(sig, "");
    assert_eq!(poll, 1_000);
    let quiet = store
        .check_account_equity("hyperliquid", 100_000.0, "", 2_000)
        .unwrap();
    assert_eq!(quiet.event, None);
    assert_eq!(quiet.note, None);
    assert!(store
        .list_equity_events(Some("hyperliquid"), 10)
        .unwrap()
        .is_empty());
}

#[test]
fn test_equity_manual_deposit_detected_and_rebased() {
    let store = StateStore::open_in_memory().unwrap();
    store
        .check_account_equity("lighter", 50_000.0, "", 1_000)
        .unwrap();
    let report = store
        .check_account_equity("lighter", 55_000.0, "", 2_000)
        .unwrap();
    let event = report.event.expect("deposit event");
    assert_eq!(event.kind, "deposit");
    assert_eq!(event.delta, 5_000.0);
    assert_eq!(event.expected, 50_000.0);
    assert_eq!(event.actual, 55_000.0);
    let (expected, _, _) = store
        .get_equity_expectation("lighter")
        .unwrap()
        .expect("rebased");
    assert_eq!(
        expected, 55_000.0,
        "baseline must rebase so PnL never absorbs the flow"
    );
    let rows = store.list_equity_events(Some("lighter"), 10).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, event.id);
}

#[test]
fn test_equity_withdrawal_and_transfer_guess() {
    let store = StateStore::open_in_memory().unwrap();
    store
        .check_account_equity("vanta", 20_000.0, "", 1_000)
        .unwrap();
    let out = store
        .check_account_equity("vanta", 19_000.0, "", 2_000)
        .unwrap();
    assert_eq!(
        out.event.as_ref().map(|e| e.kind.as_str()),
        Some("withdrawal")
    );
    let moved = store
        .check_account_equity("vanta", 19_050.0, "BTC-USDC:Buy:1", 3_000)
        .unwrap();
    assert_eq!(
        moved.event.as_ref().map(|e| e.kind.as_str()),
        Some("transfer"),
        "position change cross-check must win over the sign guess"
    );
}

#[test]
fn test_equity_missed_poll_gap_produces_no_event() {
    let store = StateStore::open_in_memory().unwrap();
    store
        .check_account_equity("grvt", 10_000.0, "", 1_000)
        .unwrap();
    let gap = crate::equity_attribution::EQUITY_POLL_GAP_MAX_MS + 1;
    let report = store
        .check_account_equity("grvt", 99_000.0, "", 1_000 + gap)
        .unwrap();
    assert_eq!(report.event, None);
    assert!(report.note.unwrap_or_default().contains("gap"));
    let (expected, _, _) = store
        .get_equity_expectation("grvt")
        .unwrap()
        .expect("rebased");
    assert_eq!(expected, 99_000.0);
    assert!(store.list_equity_events(None, 100).unwrap().is_empty());
}

#[test]
fn test_equity_events_ipc_listing_filters_and_limits() {
    let store = StateStore::open_in_memory().unwrap();
    for (acct, base) in [("hyperliquid", 10_000.0), ("lighter", 20_000.0)] {
        store.check_account_equity(acct, base, "", 1_000).unwrap();
        store
            .check_account_equity(acct, base + 100.0, "", 2_000)
            .unwrap();
    }
    let all = store.list_equity_events(None, 100).unwrap();
    assert_eq!(all.len(), 2);
    assert!(all[0].ts_ms >= all[1].ts_ms, "newest first");
    let one = store.list_equity_events(Some("lighter"), 100).unwrap();
    assert_eq!(one.len(), 1);
    assert_eq!(one[0].account_id, "lighter");
    let capped = store.list_equity_events(None, 1).unwrap();
    assert_eq!(capped.len(), 1);
    assert!(store
        .list_equity_events(Some("nope"), 10)
        .unwrap()
        .is_empty());
}

#[test]
fn test_margin_snapshots_record_list_prune() {
    let store = StateStore::open_in_memory().unwrap();
    store
        .record_margin_snapshot("hyperliquid", "hyperliquid", 1_000, 10_000.0, 2_000.0, 20.0)
        .unwrap();
    store
        .record_margin_snapshot("hyperliquid", "hyperliquid", 2_000, 10_000.0, 3_000.0, 30.0)
        .unwrap();
    store
        .record_margin_snapshot("lighter", "lighter", 2_000, 5_000.0, 500.0, 10.0)
        .unwrap();
    let rows = store.list_margin_snapshots("hyperliquid", 0).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].ts_ms, 1_000);
    assert_eq!(rows[1].rate_pct, 30.0);
    assert!(rows[0].ts_ms <= rows[1].ts_ms, "oldest first");
    let windowed = store.list_margin_snapshots("hyperliquid", 2_000).unwrap();
    assert_eq!(windowed.len(), 1);
    let removed = store.prune_margin_snapshots(2_000).unwrap();
    assert_eq!(removed, 1);
    assert_eq!(
        store.list_margin_snapshots("hyperliquid", 0).unwrap().len(),
        1
    );
    assert_eq!(store.list_margin_snapshots("lighter", 0).unwrap().len(), 1);
    let removed_all = store.prune_margin_snapshots(3_000).unwrap();
    assert_eq!(removed_all, 2);
    assert!(store
        .list_margin_snapshots("lighter", 0)
        .unwrap()
        .is_empty());
}

#[test]
fn test_margin_snapshots_reject_bad_rows() {
    let store = StateStore::open_in_memory().unwrap();
    assert!(store
        .record_margin_snapshot("", "a", 1_000, 10_000.0, 1_000.0, 10.0)
        .is_err());
    assert!(store
        .record_margin_snapshot("hyperliquid", "a", -1, 10_000.0, 1_000.0, 10.0)
        .is_err());
    assert!(store
        .record_margin_snapshot("hyperliquid", "a", 1_000, f64::NAN, 1_000.0, 10.0)
        .is_err());
    assert!(store
        .record_margin_snapshot("hyperliquid", "a", 1_000, 10_000.0, -5.0, 10.0)
        .is_err());
    assert!(store.list_margin_snapshots("hyperliquid", -1).is_err());
    assert!(store.prune_margin_snapshots(-1).is_err());
}

#[test]
fn test_margin_snapshots_table_exists_after_migration() {
    let store = StateStore::open_in_memory().unwrap();
    let count: i64 = store
        .conn
        .lock()
        .query_row("SELECT COUNT(*) FROM margin_snapshots", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn pending_order_legs_exclude_simulated_ids() {
    let store = StateStore::open_in_memory().unwrap();
    store
        .record_signal_execution(&two_leg_signal("s-paper", "BTC-USDC", None, None))
        .unwrap();
    // Paper fills write synthetic sim-* order ids — they must never be
    // polled against a real venue.
    store
        .record_leg_order_id("s-paper", LegSide::Short, "sim-deadbeef", None)
        .unwrap();
    assert!(store.list_pending_order_legs().unwrap().is_empty());
    // A real venue id on the other leg still enters the poll set.
    store
        .record_leg_order_id("s-paper", LegSide::Long, "real-ord-9", None)
        .unwrap();
    let pending = store.list_pending_order_legs().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].1.side, LegSide::Long);
}
#[test]
fn test_migration_v24_admits_external_and_preserves_rows() {
    // Replay up to v13 (the last signal_executions rebuild), seed a
    // normal signal, then migrate fully — the row must survive the v24
    // rebuild and 'external' must insert cleanly.
    let conn = Connection::open_in_memory().unwrap();
    for batch in [
        MIGRATION_V1_SQL,
        MIGRATION_V2_SQL,
        MIGRATION_V3_SQL,
        MIGRATION_V4_SQL,
        MIGRATION_V5_SQL,
        MIGRATION_V6_SQL,
        MIGRATION_V7_SQL,
        MIGRATION_V8_SQL,
        MIGRATION_V9_SQL,
        MIGRATION_V10_SQL,
        MIGRATION_V11_SQL,
        MIGRATION_V12_SQL,
        MIGRATION_V13_SQL,
    ] {
        conn.execute_batch(batch).unwrap();
    }
    conn.execute(
        "INSERT INTO signal_executions
                (id, strategy_id, symbol, status, created_at, updated_at)
             VALUES ('sig-keep', 'futures', 'BTC-USD', 'closed', 't0', 't0')",
        [],
    )
    .unwrap();
    StateStore::migrate(&conn).unwrap();
    let v: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, 29);
    let kept: String = conn
        .query_row(
            "SELECT strategy_id FROM signal_executions WHERE id = 'sig-keep'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(kept, "futures");
    conn.execute(
        "INSERT INTO signal_executions
                (id, strategy_id, symbol, status, created_at, updated_at)
             VALUES ('sig-ext', 'external', 'BTC-USD', 'in_flight', 't1', 't1')",
        [],
    )
    .unwrap();
    // The other allowed ids still insert — the widened CHECK is a
    // superset, not a replacement.
    conn.execute(
        "INSERT INTO signal_executions
                (id, strategy_id, symbol, status, created_at, updated_at)
             VALUES ('sig-tri', 'triangle', 'BTC-USD', 'closed', 't2', 't2')",
        [],
    )
    .unwrap();
    // A bogus id still fails closed.
    assert!(conn
        .execute(
            "INSERT INTO signal_executions
                    (id, strategy_id, symbol, status, created_at, updated_at)
                 VALUES ('sig-bad', 'bogus', 'BTC-USD', 'closed', 't3', 't3')",
            [],
        )
        .is_err());
}

#[test]
fn external_strategy_signal_persists() {
    // The external-close flow records strategy_id "external" — the DB
    // CHECK must accept it or the close intent can never persist.
    let store = StateStore::open_in_memory().unwrap();
    let sig = NewSignalExecution {
        id: "extclose-test-1".into(),
        strategy_id: "external".into(),
        symbol: "BTC-USD".into(),
        route_id: None,
        expected_net_usd: None,
        note: None,
        legs: vec![NewSignalLeg {
            account_id: None,
            venue: "kraken".into(),
            side: LegSide::Short,
            symbol: Some("BTC-USD".into()),
            intended_qty: 0.5,
            intended_price: None,
        }],
    };
    store.record_signal_execution(&sig).unwrap();
}

#[test]
fn every_allowed_strategy_id_persists() {
    // Regression guard for the v24 class of bug: ALLOWED_STRATEGY_IDS
    // and the signal_executions CHECK list must never diverge — every
    // admitted strategy id must actually insert.
    let store = StateStore::open_in_memory().unwrap();
    for (i, sid) in ALLOWED_STRATEGY_IDS.iter().enumerate() {
        store
            .record_signal_execution(&NewSignalExecution {
                id: format!("allowed-{i}"),
                strategy_id: (*sid).into(),
                symbol: "BTC-USD".into(),
                route_id: None,
                expected_net_usd: None,
                note: None,
                legs: vec![NewSignalLeg {
                    account_id: None,
                    venue: "kraken".into(),
                    side: LegSide::Long,
                    symbol: Some("BTC-USD".into()),
                    intended_qty: 1.0,
                    intended_price: None,
                }],
            })
            .unwrap_or_else(|e| panic!("strategy id {sid} failed CHECK: {e}"));
    }
}

#[test]
fn test_scheduled_exits_crud_and_claim() {
    let store = StateStore::open_in_memory().unwrap();
    let now = "2026-09-21T00:00:00Z".to_string();
    let row = ScheduledExit {
        id: "sched-exit-1".into(),
        venue: "lighter".into(),
        symbol: "ETH-USDC".into(),
        side: "buy".into(),
        close_pct: 50.0,
        mode: "auto".into(),
        limit_price: None,
        execute_at_ms: 1_800_000_000_000,
        status: "pending".into(),
        note: None,
        created_at: now.clone(),
        updated_at: now,
    };
    store.insert_scheduled_exit(&row).unwrap();

    // Due-scan sees it only when execute_at has passed.
    assert!(store
        .due_scheduled_exits(1_799_999_999_999)
        .unwrap()
        .is_empty());
    let due = store.due_scheduled_exits(1_800_000_000_000).unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].id, "sched-exit-1");

    // Claim is atomic: the second claim loses, so a restart or
    // overlapping tick can never dispatch the same exit twice.
    assert!(store.claim_scheduled_exit("sched-exit-1").unwrap());
    assert!(!store.claim_scheduled_exit("sched-exit-1").unwrap());

    // Firing resolves to a terminal status; later writes can't reopen.
    store
        .set_scheduled_exit_status("sched-exit-1", "fired", Some("filled"))
        .unwrap();
    store
        .set_scheduled_exit_status("sched-exit-1", "pending", Some("reopen?"))
        .unwrap();
    let listed = store.list_scheduled_exits(None).unwrap();
    assert_eq!(listed[0].status, "fired");
    assert_eq!(listed[0].note.as_deref(), Some("filled"));

    // Terminal rows are invisible to the due scan and unclaimable.
    assert!(store.due_scheduled_exits(i64::MAX).unwrap().is_empty());
    assert!(!store.claim_scheduled_exit("sched-exit-1").unwrap());
}

#[test]
fn test_scheduled_exits_reject_bad_rows() {
    let store = StateStore::open_in_memory().unwrap();
    let now = "2026-09-21T00:00:00Z".to_string();
    let base = ScheduledExit {
        id: "x".into(),
        venue: "lighter".into(),
        symbol: "ETH-USDC".into(),
        side: "buy".into(),
        close_pct: 100.0,
        mode: "market".into(),
        limit_price: None,
        execute_at_ms: 1,
        status: "pending".into(),
        note: None,
        created_at: now.clone(),
        updated_at: now,
    };
    for (i, mutate) in [
        |r: &mut ScheduledExit| r.close_pct = 0.0,
        |r: &mut ScheduledExit| r.close_pct = 101.0,
        |r: &mut ScheduledExit| r.mode = "garbage".into(),
        |r: &mut ScheduledExit| r.side = "flat".into(),
        |r: &mut ScheduledExit| r.status = "bogus".into(),
    ]
    .into_iter()
    .enumerate()
    {
        let mut bad = base.clone();
        bad.id = format!("bad-{i}");
        mutate(&mut bad);
        assert!(
            store.insert_scheduled_exit(&bad).is_err(),
            "row {i} must fail CHECK"
        );
    }
}
