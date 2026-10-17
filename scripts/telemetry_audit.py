import subprocess
import json

def rpc(cmd, args=None):
    payload = {"cmd": cmd}
    if args:
        payload["args"] = args
    raw = json.dumps(payload).encode() + b"\n"
    res = subprocess.check_output(["socat", "-t", "5", "-", "UNIX-CONNECT:/tmp/arb-dev.sock"], input=raw)
    return json.loads(res).get("ok")

vault = rpc("vault_status")
print("=== VAULT STATUS ===")
print("Unlocked:", vault.get("is_unlocked"), "| Unlocked until:", vault.get("unlocked_until"))

radar = rpc("radar_bot_status")
print("\n=== RADAR BOT STATUS ===")
print("Enabled:", radar.get("enabled"), "| Mode:", radar.get("mode"), "| Monitor Running:", radar.get("monitorRunning"))
print("Triggered Rows:", radar.get("triggeredRows"), "| Skips Today:", radar.get("skipsToday"))
print("Last Decision:", radar.get("lastDecision"))

options = rpc("options_bot_status")
print("\n=== OPTIONS BOT STATUS ===")
for sub, st in options.items():
    m = st.get("mode")
    op = st.get("openPositions")
    mop = st.get("maxOpenPositions")
    c = st.get("candidates")
    b = st.get("bestEdgeUsd")
    r = st.get("lastRefusal")
    print(f"[{sub}] mode={m} | open={op}/{mop} | candidates={c} | bestEdgeUsd={b} | refusal={r}")

positions = rpc("open_positions")
print("\n=== OPEN POSITIONS ===")
total_exp = sum(p.get("expectedNetUsd") or 0.0 for p in positions)
total_pnl = sum(p.get("unrealizedPnlUsd") or 0.0 for p in positions)
print(f"Count: {len(positions)} | Total Exp Net: +${total_exp:.2f} USD | Total Unrealized PnL: ${total_pnl:.2f} USD")
for p in positions:
    sid = p.get("strategyId")
    tid = p.get("tradeId")
    sym = p.get("symbol")
    exp = p.get("expectedNetUsd") or 0.0
    pnl = p.get("unrealizedPnlUsd") or 0.0
    fees = p.get("feesUsd") or 0.0
    print(f"  [{sid}] {tid} ({sym}): exp=+${exp:.2f}, pnl=${pnl:.2f}, fees=${fees:.2f}")
