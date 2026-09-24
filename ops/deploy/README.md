# Arbitrageur multi-bot deployment runbook

Paper-only headless deployment for Ubuntu VPS instances. The desktop Tauri
app requires a graphical session and is **not** headless-safe; remote bots
run the headless CLI daemon instead.

> Current headless scope: `arbitrageur serve` exposes a **read-only
> placeholder** health listener only (HTTP `503 {"status":"not_ready"}`).
> It ingests no market data and submits no orders. Deploy it today for
> process supervision, port allocation, log rotation, and backup drills;
> do **not** treat the 200-OK of a health check as trading readiness.

## 1. Ports

| Listener | Default | Source |
|---|---|---|
| Headless health (`serve --port`) | `8787` | CLI default; systemd instance name (`arbitrageur@8787`) |
| Engine RPC (config field only, Tauri app does not listen) | `8787` | `engine_rpc_port` in engine settings |
| Engine WS (config field only, Tauri app does not listen) | `8789` | `engine_ws_port` in engine settings |
| Frontend Vite dev server | `1420` | `frontend/vite.config.ts` (dev only) |

Multi-bot = one systemd instance per port: `arbitrageur@8787`,
`arbitrageur@8788`, … Each instance **must** keep its own data directory
(see §2); never point two instances at one SQLite file.

## 2. Data directory

Per instance (systemd sets `HOME=/var/lib/arbitrageur-<port>`):

- SQLite state: `/var/lib/arbitrageur-<port>/.arbitrageur/state.sqlite3`
  (WAL mode, `synchronous=FULL`, 5s busy timeout; dir `0700`, file `0600`
  enforced by the app on open).
- Encrypted credential vault: `/var/lib/arbitrageur-<port>/.arbitrageur/vault.enc`
  (AES-256-GCM + Argon2id; created/unlocked from the desktop UI only —
  there is no headless unlock path, by design).

Desktop single-user default for comparison: `~/.arbitrageur/` with the same
two files.

## 3. Install (per host)

Prerequisites: `sqlite3` (backup/restore scripts), `curl` (alert hook),
`jq`-free shell tools only.

```bash
# 0. Host packages.
sudo apt-get update && sudo apt-get install -y sqlite3 curl

# 1. Build and install the binary (from the repo root). The build copies
#    the vendored Lighter signer beside the binary — install it too, or
#    Lighter live signing fails closed on this host (the loader pins the
#    executable's directory and verifies the SHA-256 before dlopen).
cargo build --release --bin arbitrageur
sudo install -m 0755 target/release/arbitrageur /usr/local/bin/arbitrageur
sudo install -m 0755 target/release/liblighter_signer.so /usr/local/bin/liblighter_signer.so

# 2. Install the ops tree to /opt/arbitrageur — the unit's
#    Documentation= path, the backup cron line, and the restore script
#    all resolve under it.
sudo mkdir -p /opt/arbitrageur
sudo cp -a ops /opt/arbitrageur/

# 3. Service user + config.
sudo useradd --system --home-dir /var/lib/arbitrageur --create-home arbitrageur
sudo mkdir -p /etc/arbitrageur /var/backups/arbitrageur
sudo cp ops/deploy/arbitrageur.env.sample /etc/arbitrageur/arbitrageur.env
sudo cp ops/deploy/arbitrageur@.service ops/deploy/arbitrageur-notify@.service \
    /etc/systemd/system/
sudo cp ops/deploy/alert.sh.sample /etc/arbitrageur/alert.sh
sudo chmod 600 /etc/arbitrageur/arbitrageur.env
sudo chmod 700 /etc/arbitrageur/alert.sh
sudo chown -R arbitrageur:arbitrageur /var/backups/arbitrageur

# 4. Alert channel (see §6): set ARBITRAGEUR_ALERT_WEBHOOK in
#    /etc/arbitrageur/arbitrageur.env, then re-chmod if your editor
#    relaxed it. Skip to stay journal-only.

# 5. Validate the unit template before enabling.
sudo systemd-analyze verify arbitrageur@8787.service
sudo systemd-analyze verify arbitrageur-notify@8787.service

# 6. Start two paper bots on distinct ports/data dirs.
sudo systemctl daemon-reload
sudo systemctl enable --now arbitrageur@8787.service
sudo systemctl enable --now arbitrageur@8788.service
sudo systemctl status arbitrageur@8787.service

# 7. Health check (expect HTTP 503 not_ready placeholder).
curl -i http://127.0.0.1:8787/
```

To bind a non-loopback address, set `ARBITRAGEUR_BIND` in
`/etc/arbitrageur/arbitrageur.env` or in the per-instance
`/etc/arbitrageur/arbitrageur-<port>.env` override. Loopback is the default;
exposing the placeholder publicly gains nothing and widens fingerprinting.
The `arbitrager*` files in this directory are compatibility aliases only;
new installs should copy the `arbitrageur*` filenames shown above.

## 4. Log rotation

Services log to the journal (`SyslogIdentifier=arbitrageur-<port>`):

```bash
journalctl -u arbitrageur@8787.service -f
journalctl -u 'arbitrageur@*.service' --since '1 hour ago' | tail -50
```

Cap journal usage in `/etc/systemd/journald.conf`
(`SystemMaxUse=500M`, `RuntimeMaxUse=200M`). Only if you tee output to
files, install `ops/deploy/arbitrageur-logrotate.conf.sample` to
`/etc/logrotate.d/arbitrageur`.

## 5. Backup & restore (SQLite)

Nightly per instance (example for port 8787):

```cron
30 3 * * * arbitrageur /opt/arbitrageur/ops/deploy/backup-sqlite.sh 8787 /var/backups/arbitrageur
```

The script uses the `sqlite3` online backup API (safe against a running
daemon — never `cp` the live `state.sqlite3`), `chmod 600`s the snapshot,
runs `pragma integrity_check` before pruning, and retains 14 daily
snapshots by default (`RETAIN_DAYS=` overrides). Backup names carry the
instance tag (`state-<port>-<stamp>.sqlite3`, or `state-db-<hash>-*` for
`DB_PATH`-only invocation) so several daemons can share one backup dir and
retention never prunes another instance's snapshots.

Restore drill — use `restore-sqlite.sh`, which verifies the candidate's
`integrity_check`, preserves the outgoing DB as `*.pre-restore-<stamp>`,
clears stale `-wal`/`-shm` files, stages the copy atomically, and refuses
to run while a daemon process is alive:

```bash
sudo systemctl stop arbitrageur@8787.service
sudo -u arbitrageur /opt/arbitrageur/ops/deploy/restore-sqlite.sh \
  8787 /var/backups/arbitrageur/state-8787-<stamp>.sqlite3
sudo systemctl start arbitrageur@8787.service
```

A backup that has never been restored is unverified. Schedule a periodic
restore drill — e.g. monthly, restore the latest snapshot into a scratch
instance (`DB_PATH=/tmp/restore-drill/state.sqlite3 restore-sqlite.sh ''
/var/backups/arbitrageur/state-8787-<latest>.sqlite3`) and confirm the
integrity check passes and the app boots against it.

`vault.enc` is **not** part of the SQLite backup. Back it up separately
(also `0600`) or keep desktop-side copies; losing it only loses stored
credentials (funds stay on venues), but re-entry needs the password.

## 6. Alerting

Two complementary channels — wire at least one before running unattended:

1. **In-app event alerts.** With `ARBITRAGEUR_ALERT_WEBHOOK=<http(s) URL>`
   in the environment, the engine POSTs a small JSON body on every
   circuit-breaker trip (once per halt, not per re-trip) and every signal
   escalating to `unresolved`. The desktop app reads it from its process
   environment; the daemon reads it from the unit's EnvironmentFile.
   Compatible with generic webhook receivers — Slack/Discord incoming
   webhooks, ntfy.sh topics, Alertmanager `webhook_configs`. Unset or
   invalid = channel disabled (one warn line in the journal at startup).
   Delivery is fire-and-forget: a dead webhook never blocks trading and
   failures are logged, not retried — the signal ledger stays the source
   of truth.

2. **Service-failure pages.** `arbitrageur@.service` carries
   `OnFailure=arbitrageur-notify@%i.service`; when the daemon exits
   failed, systemd runs `/etc/arbitrageur/alert.sh <port>` once (install
   from `alert.sh.sample`). The sample POSTs the same
   `ARBITRAGEUR_ALERT_WEBHOOK`; replace the `curl` body for receivers
   needing a different shape (e.g. PagerDuty Events v2).

Test the channel after configuring: `sudo systemctl start
arbitrageur-notify@8787.service` should produce exactly one delivery.

## 7. Security notes

- No secrets in templates: `arbitrageur@.service` and
  `arbitrageur.env.sample` carry ports/paths/log filters only.
- Service hardening in the unit: `NoNewPrivileges`, `PrivateTmp`,
  `ProtectSystem=full`, `UMask=0077`.
- New live strategy entries are disabled fail-closed application-wide;
  separately, this headless daemon is only a health placeholder and performs
  no market ingestion, account reads, signing, or order submission.
