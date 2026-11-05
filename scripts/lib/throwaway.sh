# shellcheck shell=bash
# Shared helpers for scripts that need a throwaway KIZUNA database and server (e2e, bench, restore drill).
# Requires the roles from scripts/db-setup.sh; reads passwords from $KIZUNA_ENV_FILE (default .env.local).
# Source from a script that has already cd'd to the repository root and set `set -euo pipefail`.

kz_load_env() {
  local f=${KIZUNA_ENV_FILE:-.env.local}
  # shellcheck disable=SC1090
  if [[ -f $f ]]; then set -a; source "$f"; set +a; fi
  : "${KIZUNA_OWNER_PASSWORD:?run scripts/db-setup.sh or set KIZUNA_ENV_FILE}"
  : "${KIZUNA_APP_PASSWORD:?run scripts/db-setup.sh or set KIZUNA_ENV_FILE}"
  KZ_PGHOST=${KIZUNA_PGHOST:-127.0.0.1}
  KZ_PGPORT=${KIZUNA_PGPORT:-5432}
}

kz_psql_su() { sudo -u postgres psql -v ON_ERROR_STOP=1 -qAt "$@"; }

# kz_create_db NAME — empty database owned by `kizuna`, CONNECT for `kizuna_app` only (same as db-setup.sh).
kz_create_db() {
  local db=$1
  [[ $db =~ ^kizuna_[a-z0-9_]+$ ]] || { echo "refusing database name $db" >&2; return 1; }
  kz_psql_su -c "CREATE DATABASE $db OWNER kizuna"
  kz_psql_su -d "$db" -c "REVOKE ALL ON DATABASE $db FROM PUBLIC; GRANT CONNECT, TEMPORARY ON DATABASE $db TO kizuna_app;
                          REVOKE CREATE ON SCHEMA public FROM PUBLIC; ALTER SCHEMA public OWNER TO kizuna;"
}

# kz_drop_db NAME — only ever drops throwaway databases (names kizuna_{e2e,bench,drill}_*).
kz_drop_db() {
  local db=$1
  [[ $db =~ ^kizuna_(e2e|bench|drill)_[a-z0-9_]+$ ]] || { echo "refusing to drop $db" >&2; return 1; }
  kz_psql_su -c "DROP DATABASE IF EXISTS $db WITH (FORCE)"
}

kz_owner_url() { echo "postgres://kizuna:$KIZUNA_OWNER_PASSWORD@$KZ_PGHOST:$KZ_PGPORT/$1"; }
kz_app_url() { echo "postgres://kizuna_app:$KIZUNA_APP_PASSWORD@$KZ_PGHOST:$KZ_PGPORT/$1"; }

kz_free_port() { python3 -I -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])'; }

# kz_wait_http URL SECONDS
kz_wait_http() {
  local i
  for ((i = 0; i < $2 * 10; i++)); do
    curl -fsS -o /dev/null "$1" 2>/dev/null && return 0
    sleep 0.1
  done
  echo "timed out waiting for $1" >&2
  return 1
}

# kz_extraction_env — points the server at a Python env that has the extraction worker's dependencies.
kz_extraction_env() {
  if [[ -z ${KIZUNA_EXTRACTION_PYTHON:-} ]]; then
    if [[ -x services/extraction/.venv/bin/python ]]; then
      KIZUNA_EXTRACTION_PYTHON=$PWD/services/extraction/.venv/bin/python
    else
      (cd services/extraction && uv sync -q)
      KIZUNA_EXTRACTION_PYTHON=$PWD/services/extraction/.venv/bin/python
    fi
  fi
  export KIZUNA_EXTRACTION_PYTHON KIZUNA_EXTRACTION_DIR=${KIZUNA_EXTRACTION_DIR:-$PWD/services/extraction/src}
}
