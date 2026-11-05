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
psql_su() { sudo -u postgres psql -v ON_ERROR_STOP=1 -qAt "$@"; }

psql_su -v owner_pw="$OWNER_PW" -v app_pw="$APP_PW" <<'SQL'
SELECT 'CREATE ROLE kizuna LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOBYPASSRLS'
 WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'kizuna') \gexec
SELECT 'CREATE ROLE kizuna_app LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOBYPASSRLS'
 WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'kizuna_app') \gexec
ALTER ROLE kizuna PASSWORD :'owner_pw';
ALTER ROLE kizuna_app PASSWORD :'app_pw';
SQL
for db in kizuna_dev kizuna_test; do
  psql_su -c "SELECT 'CREATE DATABASE $db OWNER kizuna' WHERE NOT EXISTS (SELECT 1 FROM pg_database WHERE datname = '$db')" \
    | psql_su >/dev/null
  psql_su -d "$db" -c "REVOKE ALL ON DATABASE $db FROM PUBLIC; GRANT CONNECT, TEMPORARY ON DATABASE $db TO kizuna_app;
                       REVOKE CREATE ON SCHEMA public FROM PUBLIC; ALTER SCHEMA public OWNER TO kizuna;"
done
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
