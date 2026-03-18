---
name: db-migration-scripts
description: Writing database migration SQL scripts. Use when creating or modifying migration files under db/migration/ directories, adding tables, indexes, or columns.
---

# Database Migration Scripts

## Dual-Database Support

Every migration must be written for **both** PostgreSQL and SQLite. Migration files live in parallel directories:
- `db/migration/postgres/NNN_description.sql`
- `db/migration/sqlite/NNN_description.sql`

Both files must have the same numbered prefix and name. Use the appropriate SQL dialect for each (e.g., `BIGSERIAL` vs `INTEGER PRIMARY KEY AUTOINCREMENT`, `UUID` vs `TEXT`, `TIMESTAMPTZ` vs `TEXT`).

## File Naming

Migration files are numbered sequentially with zero-padded three-digit prefixes:
```
001_init.sql
002_code_first_routes.sql
003_wasi_config.sql
```

Check existing files to determine the next number.

## Index Naming Convention

Index names follow the format: `<table>_<column(s)>_<idx|uk>`

- `_idx` for regular indexes
- `_uk` for unique indexes

Examples:
```sql
CREATE INDEX accounts_deleted_at_idx ON accounts (deleted_at);
CREATE UNIQUE INDEX accounts_email_uk ON accounts (email) WHERE deleted_at IS NULL;
CREATE UNIQUE INDEX plugins_name_version_uk ON plugins (account_id, name, version);
```

## Primary Keys

- **Do not create indexes on primary key columns.** Both PostgreSQL and SQLite automatically create an index for primary key columns.
- Name primary key constraints as `<table>_pk`:
  ```sql
  CONSTRAINT accounts_pk PRIMARY KEY (account_id)
  ```

## Table Style

- Use uppercase SQL keywords (`CREATE TABLE`, `NOT NULL`, `PRIMARY KEY`)
- Column definitions are indented and aligned
- Primary key constraints are defined inline or as named table constraints
