#!/usr/bin/env bash
# Online backup of one Arbitrageur SQLite state file.
# Uses the sqlite3 online backup API (".backup"), which is safe against a
# running daemon holding the WAL — never copy state.sqlite3 with cp/rsync
# while the process runs (you may capture -wal/-shm inconsistently).
#
# Usage:
#   sudo -u arbitrageur ops/deploy/backup-sqlite.sh 8787 [/var/backups/arbitrageur]
#   DB_PATH=/var/lib/arbitrageur-8787/.arbitrageur/state.sqlite3 \
#     BACKUP_DIR=/var/backups/arbitrageur RETAIN_DAYS=14 \
#     ops/deploy/backup-sqlite.sh
#
# Instance naming: backups are tagged per instance so several daemons can
# share one BACKUP_DIR — `state-<port>-<stamp>.sqlite3` when invoked with a
# port, `state-<datadir-hash>-<stamp>.sqlite3` for DB_PATH-only invocation
# (hash = first 12 hex of sha256 over the resolved DB_PATH; collisions are
# vanishingly unlikely and only affect retention grouping, not integrity).
# Retention prunes only backups for THIS instance tag — it never deletes
# another instance's snapshots.
#
# Restore: see ops/deploy/restore-sqlite.sh — stop the daemon, snapshot the
# live DB, copy a verified backup into place, restart. Never overwrite a
# running daemon's DB.
#
# The backup contains the ledger + monitor history and account rows; the
# ENCRYPTED vault blob is a separate file (vault.enc is NOT inside the
# sqlite file). No secrets are printed or exported by this script.
set -euo pipefail
umask 077

PORT="${1:-${PORT:-}}"
DATA_HOME="${DATA_HOME:-}"
if [ -z "$DATA_HOME" ] && [ -n "$PORT" ]; then
  DATA_HOME="/var/lib/arbitrageur-${PORT}"
fi
DB_PATH="${DB_PATH:-${DATA_HOME}/.arbitrageur/state.sqlite3}"
BACKUP_DIR="${2:-${BACKUP_DIR:-/var/backups/arbitrageur}}"
RETAIN_DAYS="${RETAIN_DAYS:-14}"

if [ -z "$DB_PATH" ] || [ "$DB_PATH" = "/.arbitrageur/state.sqlite3" ]; then
  echo "error: no instance given. Pass a port (e.g. '8787') or set DB_PATH." >&2
  exit 2
fi
if [ ! -f "$DB_PATH" ]; then
  echo "error: database not found at $DB_PATH" >&2
  exit 2
fi
command -v sqlite3 >/dev/null 2>&1 || {
  echo "error: sqlite3 CLI not found (apt install sqlite3)." >&2
  exit 2
}

# Instance tag: the port when given, else a stable hash of the DB path so
# DB_PATH-only runs never collide on 'manual' across instances.
if [ -n "$PORT" ]; then
  INSTANCE="${PORT}"
else
  INSTANCE="db-$(printf '%s' "$DB_PATH" | sha256sum | cut -c1-12)"
fi

mkdir -p "$BACKUP_DIR"
chmod 700 "$BACKUP_DIR" 2>/dev/null || true
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
DEST="${BACKUP_DIR}/state-${INSTANCE}-${STAMP}.sqlite3"

sqlite3 "$DB_PATH" ".backup '${DEST}'"
chmod 600 "$DEST"

# Fail closed on a corrupt snapshot: verify before pruning old backups.
sqlite3 "$DEST" "pragma integrity_check;" | grep -qx "ok" || {
  echo "error: integrity_check failed for $DEST (kept for inspection)." >&2
  exit 1
}

# Retention is instance-scoped — only this tag's backups age out.
find "$BACKUP_DIR" -maxdepth 1 -name "state-${INSTANCE}-*.sqlite3" -mtime "+${RETAIN_DAYS}" -delete
echo "backup ok: ${DEST}"
