-- Init script for the integration-test MySQL instance.
--
-- This runs once at container creation (mysql sources every *.sql in
-- /docker-entrypoint-initdb.d/ as the root user at first start). The
-- MySqlCounter agent itself creates the `mysql_counters` table via
-- `CREATE TABLE IF NOT EXISTS` on first invoke, so this file is
-- intentionally minimal — it just ensures the `golem` role has the
-- privileges it needs on the `golem` database.

GRANT ALL PRIVILEGES ON golem.* TO 'golem'@'%';
FLUSH PRIVILEGES;
