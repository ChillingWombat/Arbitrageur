import json
import subprocess

def query(cmd, args={}):
    p = subprocess.run(
        ["socat", "-", "UNIX-CONNECT:/tmp/arb-dev.sock"],
        input=json.dumps({"cmd": cmd, "args": args}).encode(),
        capture_output=True
    )
    return json.loads(p.stdout.decode())

def inspect_snapshot():
    res = query("monitor_snapshot")
    rows = res.get("ok", {}).get("rows", [])
    print(f"Total monitor rows: {len(rows)}")
    for r in rows:
        st = r.get("status")
        if st in ("ready", "triggered"):
            sym = r.get("symbol")
            buy = r.get("buy")
            sell = r.get("sell")
            edge = r.get("edge_bps")
            exp = r.get("exp_profit_bps")
            pwin = r.get("p_win")
            block = r.get("block")
            print(f"[{st.upper()}] {sym} {buy} -> {sell} | edge={edge} bps | exp_profit={exp} bps | p_win={pwin} | block={block}")

if __name__ == "__main__":
    inspect_snapshot()
