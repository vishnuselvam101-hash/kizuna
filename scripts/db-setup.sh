#!/usr/bin/env bash
# Creates KIZUNA's own PostgreSQL roles and databases on a local dev server (idempotent).
# Touches nothing else. Run as root (uses `sudo -u postgres`). Writes credentials to .env.local (gitignored).
set -euo pipefail
cd "$(dirname "$0")/.."
ENV_FILE=.env.local
if [[ -f $ENV_FILE ]]; then
  # shellcheck disable=SC1090
  source "$ENV_FILE"
fi
OWNER_PW=${KIZUNA_OWNER_PASSWORD:-$(openssl rand -hex 24)}
APP_PW=${KIZUNA_APP_PASSWORD:-$(openssl rand -hex 24)}
# shellcheck source=scripts/lib/db.sh
source scripts/lib/db.sh
kz_ensure_roles "$OWNER_PW" "$APP_PW"
for db in kizuna_dev kizuna_test; do kz_ensure_db "$db"; done
umask 077
cat > "$ENV_FILE" <<ENV
KIZUNA_OWNER_PASSWORD=$OWNER_PW
KIZUNA_APP_PASSWORD=$APP_PW
KIZUNA_DATABASE_URL=postgres://kizuna_app:$APP_PW@127.0.0.1:5432/kizuna_dev
KIZUNA_OWNER_DATABASE_URL=postgres://kizuna:$OWNER_PW@127.0.0.1:5432/kizuna_dev
KIZUNA_TEST_DATABASE_URL=postgres://kizuna_app:$APP_PW@127.0.0.1:5432/kizuna_test
KIZUNA_TEST_OWNER_DATABASE_URL=postgres://kizuna:$OWNER_PW@127.0.0.1:5432/kizuna_test
KIZUNA_DATA_KEY=${KIZUNA_DATA_KEY:-$(openssl rand -base64 32)}
ENV
echo "roles kizuna/kizuna_app and databases kizuna_dev/kizuna_test ready; credentials in $ENV_FILE"
