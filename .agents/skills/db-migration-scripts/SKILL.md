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
the owning `postgres/` and `sqlite/` directories with the same numbered prefix and filename. Use
the appropriate SQL dialect for each database; do not assume PostgreSQL DDL is accepted by SQLite.

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
- Keep PostgreSQL and SQLite query-visible schema names aligned even when their underlying types or
  DDL differ.
- Use uppercase SQL keywords and match the indentation of the neighboring files.

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
- `golem-shard-manager`'s `persistence` tests run each test through both `sqlite` and `postgres`
  matrix dimensions.
- Worker-executor indexed/key-value storage tests include SQLite and PostgreSQL dimensions; select
  the storage test that exercises the changed schema.

PostgreSQL-backed tests use `golem-test-framework` to provision the database. Do not spawn database
processes directly from the test.
