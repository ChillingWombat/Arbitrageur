# Arbitrager multi-bot deployment runbook

Paper-only headless deployment for Ubuntu VPS instances. The desktop Tauri
app requires a graphical session and is **not** headless-safe; remote bots
run the headless CLI daemon instead.

> Current headless scope: `arbitrager serve` exposes a **read-only
> placeholder** health listener only (HTTP `503 {"status":"not_ready"}`).
> It ingests no market data and submits no orders. Deploy it today for
> process supervision, port allocation, log rotation, and backup drills;
> do **not** treat the 200-OK of a health check as trading readiness.

## 1. Ports

| Listener | Default | Source |
|---|---|---|
| Headless health (`serve --port`) | `8787` | CLI default; systemd instance name (`arbitrager@8787`) |
| Engine RPC (config field only, Tauri app does not listen) | `8787` | `engine_rpc_port` in engine settings |
| Engine WS (config field only, Tauri app does not listen) | `8789` | `engine_ws_port` in engine settings |
| Frontend Vite dev server | `1420` | `frontend/vite.config.ts` (dev only) |

Multi-bot = one systemd instance per port: `arbitrager@8787`,
`arbitrager@8788`, … Each instance **must** keep its own data directory
(see §2); never point two instances at one SQLite file.

## 2. Data directory

Per instance (systemd sets `HOME=/var/lib/arbitrager-<port>`):

- SQLite state: `/var/lib/arbitrager-<port>/.arbitrager/state.sqlite3`
  (WAL mode, `synchronous=FULL`, 5s busy timeout; dir `0700`, file `0600`
  enforced by the app on open).
- Encrypted credential vault: `/var/lib/arbitrager-<port>/.arbitrager/vault.enc`
  (AES-256-GCM + Argon2id; created/unlocked from the desktop UI only —
  there is no headless unlock path, by design).

Desktop single-user default for comparison: `~/.arbitrager/` with the same
two files.

## 3. Install (per host)

```bash
# 1. Build and install the binary (from the repo root).
cargo build --release --bin arbitrager
sudo install -m 0755 target/release/arbitrager /usr/local/bin/arbitrager

# 2. Service user + config.
sudo useradd --system --home-dir /var/lib/arbitrager --create-home arbitrager
sudo mkdir -p /etc/arbitrager /var/backups/arbitrager
sudo cp ops/deploy/arbitrager.env.sample /etc/arbitrager/arbitrager.env
sudo cp ops/deploy/arbitrager@.service /etc/systemd/system/
sudo chmod 600 /etc/arbitrager/arbitrager.env
sudo chown -R arbitrager:arbitrager /var/backups/arbitrager

# 3. Validate the unit template before enabling.
sudo systemd-analyze verify arbitrager@8787.service

# 4. Start two paper bots on distinct ports/data dirs.
sudo systemctl daemon-reload
sudo systemctl enable --now arbitrager@8787.service
sudo systemctl enable --now arbitrager@8788.service
sudo systemctl status arbitrager@8787.service

# 5. Health check (expect HTTP 503 not_ready placeholder).
curl -i http://127.0.0.1:8787/
```

To bind a non-loopback address, set `ARBITRAGER_BIND` in
`/etc/arbitrager/arbitrager.env`. Loopback is the default; exposing the
placeholder publicly gains nothing and widens fingerprinting.

## 4. Log rotation

Services log to the journal (`SyslogIdentifier=arbitrager-<port>`):

```bash
journalctl -u arbitrager@8787.service -f
journalctl -u 'arbitrager@*.service' --since '1 hour ago' | tail -50
```

Cap journal usage in `/etc/systemd/journald.conf`
(`SystemMaxUse=500M`, `RuntimeMaxUse=200M`). Only if you tee output to
files, install `ops/deploy/arbitrager-logrotate.conf.sample` to
`/etc/logrotate.d/arbitrager`.

## 5. Backup & restore (SQLite)

Nightly per instance (example for port 8787):

```cron
30 3 * * * arbitrager /opt/arbitrager/ops/deploy/backup-sqlite.sh 8787 /var/backups/arbitrager
```

The script uses the `sqlite3` online backup API (safe against a running
daemon — never `cp` the live `state.sqlite3`), `chmod 600`s the snapshot,
runs `pragma integrity_check` before pruning, and retains 14 daily
snapshots by default (`RETAIN_DAYS=` overrides).

Restore drill (stop the instance first; keep a copy of the live file):

```bash
sudo systemctl stop arbitrager@8787.service
cp /var/lib/arbitrager-8787/.arbitrager/state.sqlite3 /tmp/state.sqlite3.pre-restore
cp /var/backups/arbitrager/state-8787-<stamp>.sqlite3 /var/lib/arbitrager-8787/.arbitrager/state.sqlite3
chown arbitrager:arbitrager /var/lib/arbitrager-8787/.arbitrager/state.sqlite3
chmod 600 /var/lib/arbitrager-8787/.arbitrager/state.sqlite3
rm -f /var/lib/arbitrager-8787/.arbitrager/state.sqlite3-wal /var/lib/arbitrager-8787/.arbitrager/state.sqlite3-shm
sudo systemctl start arbitrager@8787.service
```

`vault.enc` is **not** part of the SQLite backup. Back it up separately
(also `0600`) or keep desktop-side copies; losing it only loses stored
credentials (funds stay on venues), but re-entry needs the password.

## 6. Security notes

- No secrets in templates: `arbitrager@.service` and
  `arbitrager.env.sample` carry ports/paths/log filters only.
- Service hardening in the unit: `NoNewPrivileges`, `PrivateTmp`,
  `ProtectSystem=full`, `UMask=0077`.
- Live trading is disabled fail-closed in code; the daemon cannot sign or
  submit orders on any venue.
