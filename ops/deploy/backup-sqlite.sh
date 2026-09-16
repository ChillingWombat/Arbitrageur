#!/usr/bin/env bash
# Online backup of one Arbitrager SQLite state file.
# Uses the sqlite3 online backup API (".backup"), which is safe against a
# running daemon holding the WAL — never copy state.sqlite3 with cp/rsync
# while the process runs (you may capture -wal/-shm inconsistently).
#
# Usage:
#   sudo -u arbitrager ops/deploy/backup-sqlite.sh 8787 [/var/backups/arbitrager]
#   DB_PATH=/var/lib/arbitrager-8787/.arbitrager/state.sqlite3 \
#     BACKUP_DIR=/var/backups/arbitrager RETAIN_DAYS=14 \
#     ops/deploy/backup-sqlite.sh
#
# Paper-only note: the backup contains paper ledger + monitor history and the
# ENCRYPTED vault blob path is separate (vault.enc is NOT inside the sqlite
# file). No secrets are printed or exported by this script.
set -euo pipefail

PORT="${1:-${PORT:-}}"
DATA_HOME="${DATA_HOME:-}"
if [ -z "$DATA_HOME" ] && [ -n "$PORT" ]; then
  DATA_HOME="/var/lib/arbitrager-${PORT}"
fi
DB_PATH="${DB_PATH:-${DATA_HOME}/.arbitrager/state.sqlite3}"
BACKUP_DIR="${BACKUP_DIR:-/var/backups/arbitrager}"
RETAIN_DAYS="${RETAIN_DAYS:-14}"

if [ -z "$DB_PATH" ] || [ "$DB_PATH" = "/.arbitrager/state.sqlite3" ]; then
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

mkdir -p "$BACKUP_DIR"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
DEST="${BACKUP_DIR}/state-${PORT:-manual}-${STAMP}.sqlite3"

sqlite3 "$DB_PATH" ".backup '${DEST}'"
chmod 600 "$DEST"

# Fail closed on a corrupt snapshot: verify before pruning old backups.
sqlite3 "$DEST" "pragma integrity_check;" | grep -qx "ok" || {
  echo "error: integrity_check failed for $DEST (kept for inspection)." >&2
  exit 1
}

find "$BACKUP_DIR" -maxdepth 1 -name 'state-*.sqlite3' -mtime "+${RETAIN_DAYS}" -delete
echo "backup ok: ${DEST}"
