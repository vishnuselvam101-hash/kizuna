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

# kz_ensure_roles OWNER_PW APP_PW — roles `kizuna` (schema owner) and `kizuna_app` (runtime), never superuser or
# BYPASSRLS; passwords set to the given values.
kz_ensure_roles() {
  kz_pg -v owner_pw="$1" -v app_pw="$2" <<'SQL'
SELECT 'CREATE ROLE kizuna LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOBYPASSRLS'
 WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'kizuna') \gexec
SELECT 'CREATE ROLE kizuna_app LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOBYPASSRLS'
 WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'kizuna_app') \gexec
ALTER ROLE kizuna PASSWORD :'owner_pw';
ALTER ROLE kizuna_app PASSWORD :'app_pw';
SQL
}

# kz_ensure_db NAME — database owned by `kizuna`; only `kizuna_app` may connect; public schema owned by `kizuna`.
kz_ensure_db() {
  local db=$1
  [[ $db =~ ^kizuna_[a-z0-9_]+$ ]] || { echo "refusing database name $db" >&2; return 1; }
  kz_pg -c "SELECT 'CREATE DATABASE $db OWNER kizuna' WHERE NOT EXISTS (SELECT 1 FROM pg_database WHERE datname = '$db')" \
    | kz_pg >/dev/null
  kz_pg -d "$db" -c "REVOKE ALL ON DATABASE $db FROM PUBLIC; GRANT CONNECT, TEMPORARY ON DATABASE $db TO kizuna_app;
                     REVOKE CREATE ON SCHEMA public FROM PUBLIC; ALTER SCHEMA public OWNER TO kizuna;"
}
