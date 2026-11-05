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
  KZ_OWNER=${KIZUNA_DB_OWNER_ROLE:-kizuna}   # role names: default dev roles, or e.g. the pilot's (drill.env)
  KZ_APP=${KIZUNA_DB_APP_ROLE:-kizuna_app}
  KZ_PGHOST=${KIZUNA_PGHOST:-127.0.0.1}
  KZ_PGPORT=${KIZUNA_PGPORT:-5432}
}

kz_psql_su() { sudo -u postgres psql -v ON_ERROR_STOP=1 -qAt "$@"; }

# kz_create_db NAME [OWNER APP] — empty database owned by the owner role, CONNECT for the runtime role only.
kz_create_db() {
  local db=$1
  [[ $db =~ ^kizuna_[a-z0-9_]+$ ]] || { echo "refusing database name $db" >&2; return 1; }
  local o=${2:-$KZ_OWNER} a=${3:-$KZ_APP}
  [[ $o =~ ^[a-z_][a-z0-9_]*$ && $a =~ ^[a-z_][a-z0-9_]*$ ]] || { echo "invalid role name" >&2; return 1; }
  kz_psql_su -c "CREATE DATABASE $db OWNER $o"
  kz_psql_su -d "$db" -c "REVOKE ALL ON DATABASE $db FROM PUBLIC; GRANT CONNECT, TEMPORARY ON DATABASE $db TO $a;
                          REVOKE CREATE ON SCHEMA public FROM PUBLIC; ALTER SCHEMA public OWNER TO $o;"
}

# kz_drop_db NAME — only ever drops throwaway databases (names kizuna_{e2e,bench,drill,ci}_*).
kz_drop_db() {
  local db=$1
  [[ $db =~ ^kizuna_(e2e|bench|drill|ci)_[a-z0-9_]+$ ]] || { echo "refusing to drop $db" >&2; return 1; }
  kz_psql_su -c "DROP DATABASE IF EXISTS $db WITH (FORCE)"
}

kz_owner_url() { echo "postgres://$KZ_OWNER:$KIZUNA_OWNER_PASSWORD@$KZ_PGHOST:$KZ_PGPORT/$1"; }
kz_app_url() { echo "postgres://$KZ_APP:$KIZUNA_APP_PASSWORD@$KZ_PGHOST:$KZ_PGPORT/$1"; }

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
