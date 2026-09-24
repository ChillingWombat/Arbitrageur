#!/usr/bin/env bash
# Safe restore of one Arbitrageur SQLite state file from a backup produced
# by backup-sqlite.sh.
#
# The daemon MUST be stopped before restoring — overwriting a live DB (or
# its -wal sidecar) corrupts state. The current DB is itself backed up
# first so a wrong-restore is reversible.
#
# Usage:
#   sudo -u arbitrageur ops/deploy/restore-sqlite.sh 8787 /var/backups/arbitrageur/state-8787-20260101T000000Z.sqlite3
#   DB_PATH=/var/lib/arbitrageur-8787/.arbitrageur/state.sqlite3 \
#     ops/deploy/restore-sqlite.sh '' /path/to/backup.sqlite3
#
# After this script succeeds, start the daemon normally. The restored file
# is verified with `pragma integrity_check` before it replaces live state.
set -euo pipefail
umask 077

PORT="${1:-${PORT:-}}"
BACKUP_FILE="${2:-}"
DATA_HOME="${DATA_HOME:-}"
if [ -z "$DATA_HOME" ] && [ -n "$PORT" ]; then
  DATA_HOME="/var/lib/arbitrageur-${PORT}"
fi
DB_PATH="${DB_PATH:-${DATA_HOME}/.arbitrageur/state.sqlite3}"

if [ -z "$DB_PATH" ] || [ "$DB_PATH" = "/.arbitrageur/state.sqlite3" ]; then
  echo "error: no instance given. Pass a port or set DB_PATH." >&2
  exit 2
fi
if [ -z "$BACKUP_FILE" ] || [ ! -f "$BACKUP_FILE" ]; then
  echo "error: backup file not found: '${BACKUP_FILE}'" >&2
  exit 2
fi
command -v sqlite3 >/dev/null 2>&1 || {
  echo "error: sqlite3 CLI not found (apt install sqlite3)." >&2
  exit 2
}

# Verify the candidate backup BEFORE touching live state.
sqlite3 "$BACKUP_FILE" "pragma integrity_check;" | grep -qx "ok" || {
  echo "error: integrity_check failed for $BACKUP_FILE — refusing to restore." >&2
  exit 1
}

# Refuse to restore over a live daemon: a held WAL means writes are in
# flight. Derive the instance tag to probe for — explicit PORT, else the
# `arbitrageur-<port>` component of DATA_HOME/DB_PATH. A scratch DB_PATH
# (restore drill into /tmp) has no daemon and skips the probe.
INSTANCE_TAG="${PORT:-}"
if [ -z "$INSTANCE_TAG" ]; then
  INSTANCE_TAG="$(printf '%s' "$DB_PATH" | sed -n 's|.*arbitrageur-\([0-9][0-9]*\)/.*|\1|p')"
fi
if [ -n "$INSTANCE_TAG" ] && pgrep -f "arbitrageur.*${INSTANCE_TAG}" >/dev/null 2>&1; then
  echo "error: an arbitrageur-${INSTANCE_TAG} process looks alive. Stop the service first" >&2
  echo "       (systemctl stop arbitrageur@${INSTANCE_TAG}), then re-run." >&2
  exit 2
fi

DB_DIR="$(dirname "$DB_PATH")"
mkdir -p "$DB_DIR"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"

# Preserve the outgoing DB (with WAL folded in via .backup) so the restore
# is reversible.
if [ -f "$DB_PATH" ]; then
  PRE_RESTORE="${DB_PATH}.pre-restore-${STAMP}.sqlite3"
  sqlite3 "$DB_PATH" ".backup '${PRE_RESTORE}'"
  chmod 600 "$PRE_RESTORE"
  echo "current state preserved at: ${PRE_RESTORE}"
fi

# Remove stale WAL/SHM so no post-backup journal replays over the restore.
rm -f "${DB_PATH}-wal" "${DB_PATH}-shm"

# Atomic-ish replace: copy to a temp file in the same dir, fsync via
# sqlite, then rename over the target.
TMP_RESTORE="${DB_PATH}.restore-${STAMP}.tmp"
cp "$BACKUP_FILE" "$TMP_RESTORE"
chmod 600 "$TMP_RESTORE"
# Open+checkpoint the temp file to force pages to disk before the rename.
sqlite3 "$TMP_RESTORE" "pragma wal_checkpoint(TRUNCATE); pragma integrity_check;" | tail -1 | grep -qx "ok" || {
  rm -f "$TMP_RESTORE"
  echo "error: staged restore failed integrity check." >&2
  exit 1
}
mv "$TMP_RESTORE" "$DB_PATH"
chmod 600 "$DB_PATH"
# Best-effort directory fsync so the rename is durable across a crash.
if command -v sync >/dev/null 2>&1; then
  sync -f "$DB_PATH" 2>/dev/null || sync
fi

echo "restore ok: ${BACKUP_FILE} -> ${DB_PATH}"
echo "next: start the daemon (systemctl start arbitrageur-${PORT:-<port>})"
