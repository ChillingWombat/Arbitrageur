#!/usr/bin/env python3
import json
from arb_rpc import rpc_call

def run_scans():
    print("=== SCAN FUNDING (BTC) ===")
    f_btc = rpc_call("scan_funding", {"symbol": "BTC", "trade_size_usd": 200.0})
    opps = f_btc.get("ok", [])
    print(f"BTC funding opps: {len(opps)}")
    for o in opps[:3]:
        print(f"  {o.get('symbol')} Long {o.get('longVenue')} / Short {o.get('shortVenue')}: Net APR {o.get('netAprPct')}%, Break-even {o.get('breakEvenHours')}h, Net Edge ${o.get('expectedNetProfitUsd')}")

    print("\n=== SCAN FUNDING (ETH) ===")
    f_eth = rpc_call("scan_funding", {"symbol": "ETH", "trade_size_usd": 200.0})
    opps = f_eth.get("ok", [])
    print(f"ETH funding opps: {len(opps)}")
    for o in opps[:3]:
        print(f"  {o.get('symbol')} Long {o.get('longVenue')} / Short {o.get('shortVenue')}: Net APR {o.get('netAprPct')}%, Break-even {o.get('breakEvenHours')}h, Net Edge ${o.get('expectedNetProfitUsd')}")

    print("\n=== SCAN FUNDING (SOL) ===")
    f_sol = rpc_call("scan_funding", {"symbol": "SOL", "trade_size_usd": 200.0})
    opps = f_sol.get("ok", [])
    print(f"SOL funding opps: {len(opps)}")
    for o in opps[:3]:
        print(f"  {o.get('symbol')} Long {o.get('longVenue')} / Short {o.get('shortVenue')}: Net APR {o.get('netAprPct')}%, Break-even {o.get('breakEvenHours')}h, Net Edge ${o.get('expectedNetProfitUsd')}")

    print("\n=== SCAN OPTIONS PACKAGES (box_spread) ===")
    boxes = rpc_call("scan_options_packages", {"substrategy_id": "box_spread", "currency": "BTC", "limit": 5})
    box_list = boxes.get("ok", [])
    print(f"Boxes found: {len(box_list)}")
    for b in box_list[:5]:
        print(f"  {b.get('id')}: edge_bps={b.get('netEdgeBps')}, yield_pct={b.get('expectedYieldPct')}%, net_edge_usd=${b.get('netEdgeUsd')}")

    print("\n=== SCAN OPTIONS PACKAGES (jelly_roll) ===")
    jelly = rpc_call("scan_options_packages", {"substrategy_id": "jelly_roll", "currency": "BTC", "limit": 5})
    jelly_list = jelly.get("ok", [])
    print(f"Jelly rolls found: {len(jelly_list)}")
    for j in jelly_list[:5]:
        print(f"  {j.get('id')}: edge_bps={j.get('netEdgeBps')}, yield_pct={j.get('expectedYieldPct')}%, net_edge_usd=${j.get('netEdgeUsd')}")

    print("\n=== SCAN OPTIONS PACKAGES (convexity) ===")
    cvx = rpc_call("scan_options_packages", {"substrategy_id": "convexity", "currency": "BTC", "limit": 5})
    cvx_list = cvx.get("ok", [])
    print(f"Convexity found: {len(cvx_list)}")
    for c in cvx_list[:5]:
        print(f"  {c.get('id')}: edge_bps={c.get('netEdgeBps')}, yield_pct={c.get('expectedYieldPct')}%, net_edge_usd=${c.get('netEdgeUsd')}")

if __name__ == "__main__":
    run_scans()
