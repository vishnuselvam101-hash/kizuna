# shellcheck shell=bash
# KIZUNA PostgreSQL roles and databases (idempotent). Shared by scripts/db-setup.sh (dev/test) and
# deploy/install-pilot.sh (pilot). Needs root (runs psql as the postgres superuser via runuser/sudo).

kz_pg() {
  if ((EUID == 0)) && command -v runuser >/dev/null; then
    runuser -u postgres -- psql -v ON_ERROR_STOP=1 -qAt "$@"
  else
    sudo -u postgres psql -v ON_ERROR_STOP=1 -qAt "$@"
  fi
}

# kz_role_exists NAME
kz_role_exists() { [[ $(kz_pg -c "SELECT 1 FROM pg_roles WHERE rolname = '$1'") == 1 ]]; }

kz_valid_role() { [[ $1 =~ ^[a-z_][a-z0-9_]{0,62}$ ]] || { echo "invalid role name $1" >&2; return 1; }; }

# kz_ensure_roles OWNER_PW APP_PW [OWNER_ROLE APP_ROLE] — schema-owner and runtime roles (default kizuna /
# kizuna_app; the pilot uses its own, see KIZUNA_DB_OWNER_ROLE/KIZUNA_DB_APP_ROLE), never superuser or BYPASSRLS;
# passwords set to the given values.
kz_ensure_roles() {
  local o=${3:-kizuna} a=${4:-kizuna_app}
  kz_valid_role "$o" && kz_valid_role "$a"
  kz_pg -v owner_pw="$1" -v app_pw="$2" -v o="$o" -v a="$a" <<'SQL'
SELECT format('CREATE ROLE %I LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOBYPASSRLS', :'o')
 WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = :'o') \gexec
SELECT format('CREATE ROLE %I LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOBYPASSRLS', :'a')
 WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = :'a') \gexec
SELECT format('ALTER ROLE %I PASSWORD %L', :'o', :'owner_pw') \gexec
SELECT format('ALTER ROLE %I PASSWORD %L', :'a', :'app_pw') \gexec
SQL
}

# kz_ensure_db NAME [OWNER_ROLE APP_ROLE] — database owned by the owner role; only the runtime role may connect;
# public schema owned by the owner role.
kz_ensure_db() {
  local db=$1 o=${2:-kizuna} a=${3:-kizuna_app}
  [[ $db =~ ^kizuna_[a-z0-9_]+$ ]] || { echo "refusing database name $db" >&2; return 1; }
  kz_valid_role "$o" && kz_valid_role "$a"
  kz_pg -c "SELECT 'CREATE DATABASE $db OWNER $o' WHERE NOT EXISTS (SELECT 1 FROM pg_database WHERE datname = '$db')" \
    | kz_pg >/dev/null
  kz_pg -d "$db" -c "REVOKE ALL ON DATABASE $db FROM PUBLIC; GRANT CONNECT, TEMPORARY ON DATABASE $db TO $a;
                     REVOKE CREATE ON SCHEMA public FROM PUBLIC; ALTER SCHEMA public OWNER TO $o;"
}
