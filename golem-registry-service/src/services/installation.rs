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

use crate::repo::installation::InstallationRepo;
use anyhow::{Context, anyhow};
use golem_common::model::golem_issuer::GolemIssuer;
use std::str::FromStr;
use tracing::info;

/// Settles the Golem issuer of the installation, which every service of it then asks this
/// registry for. The issuer lives in the registry database, so it survives a restart, a redeploy
/// and a restore of the database, and it is the same for every process.
///
/// - a configured value and nothing stored: the configured value is stored;
/// - nothing configured and nothing stored: a `urn:golem:issuer:<uuid>` value is generated and
///   stored in one atomic statement, so two first starts that race end with one value;
/// - a stored value and nothing configured: the stored value;
/// - a configured value that differs from the stored one: the start is refused, because every
///   identity of the installation carries the issuer and changing it would change them all.
pub async fn resolve_golem_issuer(
    repo: &dyn InstallationRepo,
    configured: Option<GolemIssuer>,
) -> anyhow::Result<GolemIssuer> {
    let stored = repo
        .get()
        .await
        .context("Failed to read the Golem issuer of the installation")?
        .map(|record| parse_stored(&record.golem_issuer))
        .transpose()?;

    match (configured, stored) {
        (Some(configured), Some(stored)) => {
            ensure_same(&configured, &stored)?;
            Ok(stored)
        }
        (Some(configured), None) => {
            let stored = store_if_absent(repo, &configured).await?;
            // A concurrent first start may have stored another value in the meantime.
            ensure_same(&configured, &stored)?;
            Ok(stored)
        }
        (None, Some(stored)) => Ok(stored),
        (None, None) => {
            let generated = GolemIssuer::generate();
            let stored = store_if_absent(repo, &generated).await?;
            if stored == generated {
                info!(
                    "No Golem issuer is configured (golem_issuer), so {stored} was generated and \
                     stored in the registry database. Identity proofs to outside receivers need \
                     an https issuer: set golem_issuer to an https URL on its own host before \
                     the installation has agents"
                );
            }
            Ok(stored)
        }
    }
}

async fn store_if_absent(
    repo: &dyn InstallationRepo,
    golem_issuer: &GolemIssuer,
) -> anyhow::Result<GolemIssuer> {
    let record = repo
        .store_golem_issuer_if_absent(golem_issuer.as_str())
        .await
        .context("Failed to store the Golem issuer of the installation")?;
    parse_stored(&record.golem_issuer)
}

fn parse_stored(stored: &str) -> anyhow::Result<GolemIssuer> {
    GolemIssuer::from_str(stored).map_err(|error| {
        anyhow!("The Golem issuer stored in the registry database is not valid: {error}")
    })
}

fn ensure_same(configured: &GolemIssuer, stored: &GolemIssuer) -> anyhow::Result<()> {
    if configured == stored {
        Ok(())
    } else {
        Err(anyhow!(
            "The configured golem_issuer {configured} differs from the Golem issuer {stored} \
             that this installation stored: every identity of the installation carries its \
             issuer, so it cannot change. Remove golem_issuer from the configuration, or set it \
             to {stored}"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::installation::DbInstallationRepo;
    use golem_common::config::DbSqliteConfig;
    use golem_service_base::db;
    use golem_service_base::db::sqlite::SqlitePool;
    use golem_service_base::migration::{Migrations, MigrationsDir};
    use std::sync::Arc;
    use test_r::test;

    struct TestDb {
        config: DbSqliteConfig,
        _dir: tempfile::TempDir,
    }

    impl TestDb {
        async fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let config = DbSqliteConfig {
                database: dir
                    .path()
                    .join("registry.db")
                    .to_string_lossy()
                    .into_owned(),
                max_connections: 4,
                foreign_keys: true,
            };
            db::sqlite::migrate(
                &config,
                MigrationsDir::new("db/migration".into()).sqlite_migrations(),
            )
            .await
            .unwrap();
            Self { config, _dir: dir }
        }

        /// A repository over a new connection pool, as after a restart of the registry.
        async fn repo(&self) -> Arc<dyn InstallationRepo> {
            let pool = SqlitePool::configured(&self.config).await.unwrap();
            Arc::new(DbInstallationRepo::logged(pool))
        }
    }

    fn https(value: &str) -> GolemIssuer {
        value.parse().unwrap()
    }

    #[test]
    async fn a_generated_issuer_is_stored_and_returned_again_after_a_restart() {
        let db = TestDb::new().await;
        let first = resolve_golem_issuer(db.repo().await.as_ref(), None)
            .await
            .unwrap();
        assert!(first.is_generated());
        let after_restart = resolve_golem_issuer(db.repo().await.as_ref(), None)
            .await
            .unwrap();
        assert_eq!(after_restart, first);
    }

    #[test]
    async fn a_configured_issuer_is_stored_and_used_without_configuration_later() {
        let db = TestDb::new().await;
        let configured = https("https://id.acme.example");
        let first = resolve_golem_issuer(db.repo().await.as_ref(), Some(configured.clone()))
            .await
            .unwrap();
        assert_eq!(first, configured);
        let later = resolve_golem_issuer(db.repo().await.as_ref(), None)
            .await
            .unwrap();
        assert_eq!(later, configured);
        let same = resolve_golem_issuer(db.repo().await.as_ref(), Some(configured.clone()))
            .await
            .unwrap();
        assert_eq!(same, configured);
    }

    #[test]
    async fn a_configured_issuer_that_differs_from_the_stored_one_stops_the_start() {
        let db = TestDb::new().await;
        let stored = resolve_golem_issuer(db.repo().await.as_ref(), None)
            .await
            .unwrap();
        let error = resolve_golem_issuer(
            db.repo().await.as_ref(),
            Some(https("https://other.acme.example")),
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(error.contains("differs"), "{error}");
        assert!(error.contains(stored.as_str()), "{error}");
        assert!(error.contains("https://other.acme.example"), "{error}");
        // The refusal changed nothing.
        let still = resolve_golem_issuer(db.repo().await.as_ref(), None)
            .await
            .unwrap();
        assert_eq!(still, stored);
    }

    #[test]
    async fn two_concurrent_first_starts_end_with_one_issuer() {
        let db = TestDb::new().await;
        let (a, b) = (db.repo().await, db.repo().await);
        let results = futures::future::join_all((0..6).map(|index| {
            let repo = if index % 2 == 0 { a.clone() } else { b.clone() };
            async move { resolve_golem_issuer(repo.as_ref(), None).await.unwrap() }
        }))
        .await;
        assert!(
            results.iter().all(|issuer| issuer == &results[0]),
            "{results:?}"
        );
    }

    #[test]
    async fn an_invalid_stored_issuer_stops_the_start() {
        let db = TestDb::new().await;
        let repo = db.repo().await;
        repo.store_golem_issuer_if_absent("not an issuer")
            .await
            .unwrap();
        let error = resolve_golem_issuer(repo.as_ref(), None)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("not valid"), "{error}");
    }
}
