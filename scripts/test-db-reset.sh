#!/usr/bin/env bash
# Drops and recreates the KIZUNA test database only (needed when an unreleased migration is edited).
set -euo pipefail
sudo -u postgres psql -v ON_ERROR_STOP=1 -qAt <<'SQL'
DROP DATABASE IF EXISTS kizuna_test WITH (FORCE);
CREATE DATABASE kizuna_test OWNER kizuna;
SQL
sudo -u postgres psql -v ON_ERROR_STOP=1 -qAt -d kizuna_test -c "REVOKE ALL ON DATABASE kizuna_test FROM PUBLIC; GRANT CONNECT, TEMPORARY ON DATABASE kizuna_test TO kizuna_app; REVOKE CREATE ON SCHEMA public FROM PUBLIC; ALTER SCHEMA public OWNER TO kizuna;"
echo "kizuna_test recreated"
