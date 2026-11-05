#!/usr/bin/env bash
# Removes the KIZUNA pilot installed by deploy/install-pilot.sh. Run as root.
#   sudo deploy/uninstall-pilot.sh --dry-run    # print actions only
#   sudo deploy/uninstall-pilot.sh              # stop + remove units, Tailscale Serve mapping and /opt/kizuna;
#                                               # KEEPS data: database, /var/lib/kizuna, /etc/kizuna, backups
#   sudo deploy/uninstall-pilot.sh --purge      # also drops database kizuna_pilot and deletes data, secrets,
#                                               # local backups and the kizuna user (asks to type the db name)
# Never removes apt packages (ClamAV, rclone, bubblewrap), PostgreSQL roles (shared with dev) or off-host copies.
set -euo pipefail
REPO=$(cd "$(dirname "$0")/.." && pwd)
DRY=0 PURGE=0
while (($#)); do
  case $1 in
    --dry-run) DRY=1; shift ;;
    --purge) PURGE=1; shift ;;
    *) echo "unknown option $1" >&2; exit 64 ;;
  esac
done
DB=kizuna_pilot TS_PORT=8446
UNITS=(kizuna.service kizuna-web.service kizuna-backup.service kizuna-backup.timer kizuna-integrity.service
  kizuna-integrity.timer kizuna-alert@.service)
run() { if ((DRY)); then printf '   would run: %s\n' "$*"; else "$@"; fi; }
((EUID == 0)) || ((DRY)) || { echo "run as root" >&2; exit 1; }
((DRY)) && echo "DRY RUN: nothing will be changed"

echo "== services"
for u in kizuna-backup.timer kizuna-integrity.timer kizuna-web.service kizuna.service; do
  if systemctl list-unit-files "$u" --no-legend 2>/dev/null | grep -q .; then run systemctl disable --now "$u"; fi
done
for u in "${UNITS[@]}"; do
  if [[ -e /etc/systemd/system/$u ]]; then run rm -f "/etc/systemd/system/$u"; fi
done
run systemctl daemon-reload

echo "== Tailscale Serve :$TS_PORT"
if tailscale serve status 2>/dev/null | grep -qF ":$TS_PORT"; then run tailscale serve --https="$TS_PORT" off; else echo "   not configured"; fi

echo "== code"
if [[ -e /opt/kizuna ]]; then run rm -rf /opt/kizuna; else echo "   /opt/kizuna absent"; fi

if ((PURGE)); then
  echo "== PURGE data (database $DB, /var/lib/kizuna, /etc/kizuna, /var/backups/kizuna, user kizuna)"
  if ((!DRY)); then
    read -r -p "type the database name ($DB) to confirm: " ans
    [[ $ans == "$DB" ]] || { echo "not confirmed; data kept" >&2; exit 1; }
  fi
  if ((EUID == 0)); then
    # shellcheck source=scripts/lib/db.sh
    source "$REPO/scripts/lib/db.sh"
    if [[ $(kz_pg -c "SELECT 1 FROM pg_database WHERE datname = '$DB'") == 1 ]]; then
      run kz_pg -c "DROP DATABASE $DB WITH (FORCE)"
    fi
  fi
  run rm -rf /var/lib/kizuna /var/lib/kizuna-backup /etc/kizuna /var/backups/kizuna
  if id kizuna >/dev/null 2>&1; then run userdel kizuna; fi
else
  echo "== data kept: database $DB, /var/lib/kizuna, /etc/kizuna (secrets), /var/backups/kizuna (use --purge to delete)"
fi
echo "packages left installed: clamav-daemon clamav-freshclam rclone bubblewrap (apt remove them if unwanted)"
