// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::registry_db_path;
use anyhow::anyhow;
use golem_cli::error::NonSuccessfulExit;
use golem_cli::log::{log_error, logln};
use golem_common::config::DbSqliteConfig;
use golem_service_base::db::sqlite::SqlitePool;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::path::{Path, PathBuf};

/// Current database compatibility version for local server data.
///
/// Bump this when making any intentional, breaking change to local sqlite data.
/// Do not bump for additive, backward-compatible changes.
///
/// Compatibility uses strict equality:
/// - DB version == CLI version: compatible
/// - DB version  > CLI version: DB created by newer CLI
/// - DB version  < CLI version: DB is incompatible with this CLI
///
/// Migration versions are checked independently by sqlx. If migrations in the database are ahead
/// of this CLI (`MigrateError::VersionTooNew`), startup treats that as a newer-DB case and shows
/// the same guidance as for `DB version > CLI version` (upgrade CLI or use a different data dir).
pub const CLI_CURRENT_DB_COMPAT_VERSION: i64 = 1;

const COMPAT_TABLE_NAME: &str = "golem_compat_info";

#[derive(Debug)]
pub enum CompatCheckError {
    MissingCompatTable {
        registry_db_path: PathBuf,
    },
    MissingCompatRow {
        registry_db_path: PathBuf,
    },
    InvalidCompatValue {
        registry_db_path: PathBuf,
        details: String,
    },
    UnsupportedOlderDbFormat {
        registry_db_path: PathBuf,
        db_compat_version: i64,
        supported_db_compat_version: i64,
    },
    UnsupportedNewerDbFormat {
        registry_db_path: PathBuf,
        db_compat_version: i64,
        supported_db_compat_version: i64,
    },
    SqliteError {
        registry_db_path: PathBuf,
        source: anyhow::Error,
    },
}

impl CompatCheckError {
    fn sqlite(registry_db_path: PathBuf, source: anyhow::Error) -> Self {
        Self::SqliteError {
            registry_db_path,
            source,
        }
    }
}

impl Display for CompatCheckError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            CompatCheckError::MissingCompatTable { registry_db_path } => write!(
                f,
                "Compatibility metadata table '{}' is missing in {}",
                COMPAT_TABLE_NAME,
                registry_db_path.display()
            ),
            CompatCheckError::MissingCompatRow { registry_db_path } => write!(
                f,
                "Compatibility metadata row is missing in {}",
                registry_db_path.display()
            ),
            CompatCheckError::InvalidCompatValue {
                registry_db_path,
                details,
            } => write!(
                f,
                "Compatibility metadata in {} is invalid: {}",
                registry_db_path.display(),
                details
            ),
            CompatCheckError::UnsupportedOlderDbFormat {
                registry_db_path,
                db_compat_version,
                supported_db_compat_version,
            } => write!(
                f,
                "Database compatibility version {} in {} is older than this CLI version {}",
                db_compat_version,
                registry_db_path.display(),
                supported_db_compat_version
            ),
            CompatCheckError::UnsupportedNewerDbFormat {
                registry_db_path,
                db_compat_version,
                supported_db_compat_version,
            } => write!(
                f,
                "Database compatibility version {} in {} is newer than this CLI version {}",
                db_compat_version,
                registry_db_path.display(),
                supported_db_compat_version
            ),
            CompatCheckError::SqliteError {
                registry_db_path,
                source,
            } => write!(
                f,
                "Failed to access compatibility metadata in {}: {}",
                registry_db_path.display(),
                source
            ),
        }
    }
}

impl Error for CompatCheckError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            CompatCheckError::SqliteError { source, .. } => Some(source.as_ref()),
            _ => None,
        }
    }
}

pub async fn preflight_registry_db_compat(data_dir: &Path) -> Result<(), CompatCheckError> {
    let registry_db_path = registry_db_path(data_dir);

    if !registry_db_path.exists() {
        return Ok(());
    }

    let pool = sqlite_pool(&registry_db_path)
        .await
        .map_err(|err| CompatCheckError::sqlite(registry_db_path.clone(), err))?;

    let has_table = pool
        .with_ro("compat", "table_exists")
        .fetch_optional_as::<(i64,), _>(
            sqlx::query_as(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ? LIMIT 1;",
            )
            .bind(COMPAT_TABLE_NAME),
        )
        .await
        .map_err(|err| CompatCheckError::sqlite(registry_db_path.clone(), err.into()))?
        .is_some();

    if !has_table {
        return Err(CompatCheckError::MissingCompatTable { registry_db_path });
    }

    let rows = pool
        .with_ro("compat", "read_db_compat_version")
        .fetch_all_as::<(i64,), _>(sqlx::query_as(
            "SELECT db_compat_version FROM golem_compat_info ORDER BY rowid ASC LIMIT 2;",
        ))
        .await
        .map_err(|err| CompatCheckError::sqlite(registry_db_path.clone(), err.into()))?;

    let db_compat_version = if rows.is_empty() {
        return Err(CompatCheckError::MissingCompatRow { registry_db_path });
    } else if rows.len() > 1 {
        return Err(CompatCheckError::InvalidCompatValue {
            registry_db_path,
            details: "multiple compatibility rows found".to_string(),
        });
    } else {
        rows[0].0
    };

    if db_compat_version < 0 {
        return Err(CompatCheckError::InvalidCompatValue {
            registry_db_path,
            details: format!("negative compatibility version: {}", db_compat_version),
        });
    }

    if db_compat_version > CLI_CURRENT_DB_COMPAT_VERSION {
        return Err(CompatCheckError::UnsupportedNewerDbFormat {
            registry_db_path,
            db_compat_version,
            supported_db_compat_version: CLI_CURRENT_DB_COMPAT_VERSION,
        });
    }

    if db_compat_version < CLI_CURRENT_DB_COMPAT_VERSION {
        return Err(CompatCheckError::UnsupportedOlderDbFormat {
            registry_db_path,
            db_compat_version,
            supported_db_compat_version: CLI_CURRENT_DB_COMPAT_VERSION,
        });
    }

    Ok(())
}

pub async fn write_registry_db_compat(data_dir: &Path) -> anyhow::Result<()> {
    let registry_db_path = registry_db_path(data_dir);
    let pool = sqlite_pool(&registry_db_path).await?;

    pool.with_rw("compat", "create_table")
        .execute(sqlx::query(
            r#"
                CREATE TABLE IF NOT EXISTS golem_compat_info (
                    id INTEGER PRIMARY KEY CHECK (id = 1),
                    db_compat_version INTEGER NOT NULL CHECK (db_compat_version >= 0)
                );
            "#,
        ))
        .await?;

    pool.with_rw("compat", "upsert_db_compat_version")
        .execute(
            sqlx::query(
                r#"
                    INSERT INTO golem_compat_info (id, db_compat_version)
                    VALUES (1, ?)
                    ON CONFLICT(id) DO UPDATE
                    SET db_compat_version = excluded.db_compat_version;
                "#,
            )
            .bind(CLI_CURRENT_DB_COMPAT_VERSION),
        )
        .await?;

    Ok(())
}

pub fn map_local_server_startup_error(error: anyhow::Error, data_dir: &Path) -> anyhow::Error {
    if let Some(compat_error) = error.downcast_ref::<CompatCheckError>() {
        logln("");
        match compat_error {
            CompatCheckError::UnsupportedNewerDbFormat {
                db_compat_version,
                supported_db_compat_version,
                ..
            } => {
                log_error(
                    "The database format in this data directory is newer than this CLI supports.",
                );
                logln(format!(
                    "- database compatibility version: {db_compat_version}, CLI compatibility version: {supported_db_compat_version}"
                ));
                logln(format!("- data directory: {}", data_dir.display()));
                logln("- action: upgrade the CLI or use a different --data-dir");
                logln("- action: if data loss is acceptable, run `golem server run --clean`");
            }
            CompatCheckError::UnsupportedOlderDbFormat {
                db_compat_version,
                supported_db_compat_version,
                ..
            } => {
                log_error(
                    "The database format in this data directory is incompatible with this CLI.",
                );
                logln(format!(
                    "- database compatibility version: {db_compat_version}, CLI compatibility version: {supported_db_compat_version}"
                ));
                logln(format!("- data directory: {}", data_dir.display()));
                logln("- action: use a different --data-dir");
                logln("- action: if data loss is acceptable, run `golem server run --clean`");
            }
            CompatCheckError::MissingCompatTable { .. }
            | CompatCheckError::MissingCompatRow { .. }
            | CompatCheckError::InvalidCompatValue { .. } => {
                log_error(
                    "The database format in this data directory is incompatible with this CLI.",
                );
                logln(format!("- data directory: {}", data_dir.display()));
                logln("- action: use a different --data-dir");
                logln("- action: if data loss is acceptable, run `golem server run --clean`");
            }
            CompatCheckError::SqliteError { .. } => {
                log_error("Failed to read database compatibility metadata.");
                logln(format!("- data directory: {}", data_dir.display()));
                logln("- action: use a different --data-dir");
                logln("- action: if data loss is acceptable, run `golem server run --clean`");
            }
        }

        logln(format!("- details: {compat_error:#}"));
        return anyhow!(NonSuccessfulExit);
    }

    if let Some(migrate_error) = error.downcast_ref::<sqlx::migrate::MigrateError>() {
        logln("");
        if let sqlx::migrate::MigrateError::VersionTooNew(db_version, supported_version) =
            migrate_error
        {
            log_error(
                "The database format in this data directory is newer than this CLI supports.",
            );
            logln(format!(
                "- migration version in database: {db_version}, max supported migration version: {supported_version}"
            ));
            logln(format!("- data directory: {}", data_dir.display()));
            logln("- action: upgrade the CLI or use a different --data-dir");
            logln("- action: if data loss is acceptable, run `golem server run --clean`");
            logln(format!("- details: {migrate_error:#}"));
            return anyhow!(NonSuccessfulExit);
        }

        log_error("SQLite migration failed and the database is incompatible with this CLI.");
        logln(format!("- data directory: {}", data_dir.display()));
        logln("- action: use a different --data-dir");
        logln("- action: if data loss is acceptable, run `golem server run --clean`");
        logln(format!("- details: {migrate_error:#}"));
        return anyhow!(NonSuccessfulExit);
    }

    error
}

async fn sqlite_pool(registry_db_path: &Path) -> anyhow::Result<SqlitePool> {
    SqlitePool::configured(&DbSqliteConfig {
        database: registry_db_path.to_string_lossy().to_string(),
        max_connections: 1,
        foreign_keys: true,
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_cli::error::NonSuccessfulExit;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    use test_r::test;

    struct TestDataDir {
        path: PathBuf,
    }

    impl TestDataDir {
        fn new() -> Self {
            static NEXT_ID: AtomicU64 = AtomicU64::new(0);
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock went backwards")
                .as_nanos();
            let path = std::env::temp_dir().join(format!("golem-compat-test-{nanos}-{id}"));
            std::fs::create_dir_all(&path).expect("failed to create temp data dir");
            Self { path }
        }
    }

    impl Drop for TestDataDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    async fn setup_registry_db(data_dir: &Path) {
        let db_path = registry_db_path(data_dir);
        let _ = sqlite_pool(&db_path)
            .await
            .expect("failed to initialize registry sqlite db");
    }

    async fn create_compat_table(data_dir: &Path) {
        let db_path = registry_db_path(data_dir);
        let pool = sqlite_pool(&db_path)
            .await
            .expect("failed to initialize sqlite pool");

        pool.with_rw("test", "create_compat_table")
            .execute(sqlx::query(
                "CREATE TABLE golem_compat_info (db_compat_version INTEGER NOT NULL);",
            ))
            .await
            .expect("failed to create compat table");
    }

    async fn insert_compat_row(data_dir: &Path, version: i64) {
        let db_path = registry_db_path(data_dir);
        let pool = sqlite_pool(&db_path)
            .await
            .expect("failed to initialize sqlite pool");

        pool.with_rw("test", "insert_compat_row")
            .execute(
                sqlx::query("INSERT INTO golem_compat_info (db_compat_version) VALUES (?);")
                    .bind(version),
            )
            .await
            .expect("failed to insert compat row");
    }

    #[test]
    async fn preflight_allows_missing_registry_db() {
        let data_dir = TestDataDir::new();
        let result = preflight_registry_db_compat(&data_dir.path).await;
        assert!(result.is_ok(), "expected success, got: {result:?}");
    }

    #[test]
    async fn preflight_rejects_missing_compat_table() {
        let data_dir = TestDataDir::new();
        setup_registry_db(&data_dir.path).await;

        let result = preflight_registry_db_compat(&data_dir.path).await;
        assert!(matches!(
            result,
            Err(CompatCheckError::MissingCompatTable { .. })
        ));
    }

    #[test]
    async fn preflight_rejects_missing_compat_row() {
        let data_dir = TestDataDir::new();
        create_compat_table(&data_dir.path).await;

        let result = preflight_registry_db_compat(&data_dir.path).await;
        assert!(matches!(
            result,
            Err(CompatCheckError::MissingCompatRow { .. })
        ));
    }

    #[test]
    async fn preflight_rejects_multiple_rows() {
        let data_dir = TestDataDir::new();
        create_compat_table(&data_dir.path).await;
        insert_compat_row(&data_dir.path, CLI_CURRENT_DB_COMPAT_VERSION).await;
        insert_compat_row(&data_dir.path, CLI_CURRENT_DB_COMPAT_VERSION).await;

        let result = preflight_registry_db_compat(&data_dir.path).await;
        assert!(matches!(
            result,
            Err(CompatCheckError::InvalidCompatValue { .. })
        ));
    }

    #[test]
    async fn preflight_rejects_negative_version() {
        let data_dir = TestDataDir::new();
        create_compat_table(&data_dir.path).await;
        insert_compat_row(&data_dir.path, -1).await;

        let result = preflight_registry_db_compat(&data_dir.path).await;
        assert!(matches!(
            result,
            Err(CompatCheckError::InvalidCompatValue { .. })
        ));
    }

    #[test]
    async fn preflight_rejects_older_version() {
        let data_dir = TestDataDir::new();
        create_compat_table(&data_dir.path).await;
        insert_compat_row(&data_dir.path, CLI_CURRENT_DB_COMPAT_VERSION - 1).await;

        let result = preflight_registry_db_compat(&data_dir.path).await;
        assert!(matches!(
            result,
            Err(CompatCheckError::UnsupportedOlderDbFormat { .. })
        ));
    }

    #[test]
    async fn preflight_rejects_newer_version() {
        let data_dir = TestDataDir::new();
        create_compat_table(&data_dir.path).await;
        insert_compat_row(&data_dir.path, CLI_CURRENT_DB_COMPAT_VERSION + 1).await;

        let result = preflight_registry_db_compat(&data_dir.path).await;
        assert!(matches!(
            result,
            Err(CompatCheckError::UnsupportedNewerDbFormat { .. })
        ));
    }

    #[test]
    async fn preflight_accepts_exact_version() {
        let data_dir = TestDataDir::new();
        create_compat_table(&data_dir.path).await;
        insert_compat_row(&data_dir.path, CLI_CURRENT_DB_COMPAT_VERSION).await;

        let result = preflight_registry_db_compat(&data_dir.path).await;
        assert!(result.is_ok(), "expected success, got: {result:?}");
    }

    #[test]
    async fn write_registry_db_compat_creates_and_upserts_single_row() {
        let data_dir = TestDataDir::new();

        write_registry_db_compat(&data_dir.path)
            .await
            .expect("first compat write failed");
        write_registry_db_compat(&data_dir.path)
            .await
            .expect("second compat write failed");

        let db_path = registry_db_path(&data_dir.path);
        let pool = sqlite_pool(&db_path)
            .await
            .expect("failed to initialize sqlite pool");

        let rows = pool
            .with_ro("test", "read_rows")
            .fetch_all_as::<(i64, i64), _>(sqlx::query_as(
                "SELECT id, db_compat_version FROM golem_compat_info ORDER BY id;",
            ))
            .await
            .expect("failed to query compat rows");

        assert_eq!(rows.len(), 1, "expected exactly one compat row");
        assert_eq!(rows[0].0, 1, "compat row id must be 1");
        assert_eq!(
            rows[0].1, CLI_CURRENT_DB_COMPAT_VERSION,
            "compat row should store current CLI compat version"
        );
    }

    #[test]
    fn map_startup_error_treats_version_too_new_migration_as_non_successful_exit() {
        let source = anyhow::Error::new(sqlx::migrate::MigrateError::VersionTooNew(2, 1));
        let mapped = map_local_server_startup_error(source, Path::new("/tmp/golem-test-data"));

        assert!(mapped.downcast_ref::<NonSuccessfulExit>().is_some());
    }

    #[test]
    fn map_startup_error_treats_dirty_migration_as_non_successful_exit() {
        let source = anyhow::Error::new(sqlx::migrate::MigrateError::Dirty(3));
        let mapped = map_local_server_startup_error(source, Path::new("/tmp/golem-test-data"));

        assert!(mapped.downcast_ref::<NonSuccessfulExit>().is_some());
    }
}
