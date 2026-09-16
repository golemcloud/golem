-- Init script for the integration-test Postgres instance.
--
-- This runs once at container creation (postgres-alpine sources every
-- *.sql in /docker-entrypoint-initdb.d/ as the superuser at first
-- start). The PgCounter agent itself creates the `pg_counters` table
-- via `CREATE TABLE IF NOT EXISTS` on first invoke, so this file is
-- intentionally minimal — it just ensures the public schema exists
-- and the role has the privileges it needs.

GRANT ALL PRIVILEGES ON SCHEMA public TO golem;
