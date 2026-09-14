---
name: db-migration-scripts
description: Writing database migration SQL scripts. Use when creating or modifying migration files under db/migration/ directories, adding tables, indexes, or columns.
---

# Database Migration Scripts

## Owning Directories

The repository currently has these migration roots:

- `golem-registry-service/db/migration/{postgres,sqlite}/`
- `golem-shard-manager/db/migration/{postgres,sqlite}/`
- `golem-worker-executor/db/migration/{indexed,keyvalue,scheduler}/{postgres,sqlite}/`

Put the migration beside the subsystem that owns the schema. Do not create another migration root
when one of these already owns the table.

## PostgreSQL and SQLite

Every schema change must be implemented for **both** PostgreSQL and SQLite. Add matching files to
the owning `postgres/` and `sqlite/` directories with the same numbered prefix and filename. Do not
assume PostgreSQL DDL is accepted by SQLite.

By default, prefer one schema shape for both engines — column types included — so that a single SQL
string can serve both. This is a preference, not a requirement. Some roots deliberately diverge
because the two engines play genuinely different roles there. The deciding question is whether that
root's query layer is shared:

| Migration root | Query layer | Convention |
|---|---|---|
| `golem-registry-service/db/migration/` | one impl expanded per backend by `#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]` | converge by default |
| `golem-shard-manager/db/migration/` | same `trait_gen` pattern (`src/quota/quota_repo.rs`, `src/sharding/persistence/db.rs`) | converge by default |
| `golem-worker-executor/db/migration/{indexed,keyvalue,scheduler}/` | separate `postgres.rs` / `sqlite.rs` under `src/storage/<subsystem>/` (alongside `multi_sqlite.rs`, `redis.rs`, `memory.rs`) | diverge where the engine calls for it |

### Shared-query roots

In the registry service and shard manager, each SQL literal is written once and expanded for both
pools, so a divergent schema would force the query layer to be split per dialect. Keep the resulting
schema identical whenever possible. SQLite's type affinity accepts `UUID`, `TIMESTAMP`, `BIGINT`,
`BYTEA`, and `NUMERIC` verbatim, so use them in both. Reach for an engine-specific type
(`BIGSERIAL` vs `INTEGER PRIMARY KEY AUTOINCREMENT`, `TEXT[]` vs `TEXT`) only where there is no
common alternative.

DDL *mechanics* may still differ where SQLite requires it — no `ALTER COLUMN ... TYPE` (drop and
recreate), no multi-column `ADD COLUMN` (split the statements), no `GREATEST` (use `CASE`). Converge
on the same end-state schema regardless: after `002_code_first_routes.sql` takes either path, both
tables have the same columns, types, and constraint names.

### Per-engine roots

The worker-executor storage subsystems have fully separate implementations per backend, so their
migrations are free to diverge and already do: `BIGINT`/`BYTEA`/`DOUBLE PRECISION` against
`INTEGER`/`BLOB`/`REAL`, different primary-key column ordering, different indexes, and
PostgreSQL-only autovacuum tuning. Tune each engine on its own terms here rather than forcing a
shared shape.

## File Naming

Migration files use sequential, zero-padded three-digit prefixes within their owning directory:

```
001_init.sql
002_code_first_routes.sql
003_wasi_config.sql
```

Check both database directories and choose the same next number in each.

## Follow the Local Schema Style

- Read neighboring migrations before choosing types, constraint names, index names, quoting, and
  DDL structure. Naming is not globally uniform across all migration roots.
- Do not create an extra index for a primary key; both databases already index it.
- Keep PostgreSQL and SQLite query-visible schema names aligned.
- Use uppercase SQL keywords and match the indentation of the neighboring files.

Index and constraint naming follows the owning root:

- `golem-registry-service` and `golem-shard-manager` use `<table>_<column(s)>_<idx|uk>` for indexes
  (`_idx` regular, `_uk` unique) and `<table>_pk` for primary key constraints:

  ```sql
  CREATE INDEX accounts_deleted_at_idx ON accounts (deleted_at);
  CREATE UNIQUE INDEX accounts_email_uk ON accounts (email) WHERE deleted_at IS NULL;
  CONSTRAINT accounts_pk PRIMARY KEY (account_id)
  ```

- `golem-worker-executor`'s `keyvalue` and `indexed` roots use an `idx_<...>` prefix instead; its
  `scheduler` root uses the suffix style above. Match the files you are editing.

## No Compatibility Layer

SQL migration files are still the mechanism for moving the repository's current database schema
forward. They do not authorize backward-compatibility work. Make the required schema transition
directly, update all in-tree queries and models in the same change, and remove replaced columns or
tables. Do not add dual-read/dual-write behavior, legacy columns, compatibility views, old-format
parsing, historical-data backfills, or support for old application binaries unless the repository's
backward-compatibility policy is explicitly changed.

## Verification

1. Confirm that the PostgreSQL and SQLite filenames and schema results match.
2. Run an affected persistence/repository test against both database variants. The selected test
   should apply migrations to a fresh database before exercising the changed table.
3. Run the smallest affected crate check and tests with `--report-time`.

Examples of the relevant integration coverage:

- Registry repository tests initialize PostgreSQL and SQLite from
  `golem-registry-service/db/migration/`.
- `golem-shard-manager`'s `persistence` tests run each test through the `sqlite`, `postgres` and
  `etcd` matrix dimensions.
- Worker-executor indexed/key-value storage tests include SQLite and PostgreSQL dimensions; select
  the storage test that exercises the changed schema.

PostgreSQL-backed tests use `golem-test-framework` to provision the database. Do not spawn database
processes directly from the test.
