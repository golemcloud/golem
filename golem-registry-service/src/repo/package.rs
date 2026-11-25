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

use async_trait::async_trait;
use bytes::Bytes;
use conditional_trait_gen::trait_gen;
use futures::StreamExt;
use futures::stream::BoxStream;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{Pool, PoolApi};
use golem_service_base::repo::{RepoError, RepoResult};
use golem_service_base::storage::blob::{BlobStorage, StoredPublishedBlob};
use sqlx::types::Json;
use uuid::Uuid;

#[derive(Clone)]
pub struct NewPackageVersion {
    pub package_path: String,
    pub publisher_account_id: Uuid,
    pub version: String,
    pub description: Option<String>,
    pub callables: Vec<String>,
}

/// An archive whose transport digest and OCI manifest digest were checked before storage.
#[derive(Debug)]
pub struct StoredPackageArchive {
    archive: StoredPublishedBlob,
    manifest_digest: String,
}

impl StoredPackageArchive {
    pub async fn store(
        storage: &dyn BlobStorage,
        archive_digest: &str,
        manifest_digest: &str,
        input: BoxStream<'_, Result<Bytes, anyhow::Error>>,
    ) -> anyhow::Result<Self> {
        let mut remaining = golem_package::MAX_PACKAGE_BYTES;
        let input = input
            .map(|chunk| {
                let chunk = chunk?;
                remaining = remaining
                    .checked_sub(chunk.len() as u64)
                    .ok_or_else(|| anyhow::anyhow!("Package size limit exceeded"))?;
                Ok(chunk)
            })
            .boxed();
        let staged = storage.stage_published_blob(archive_digest, input).await?;
        let manifest_digest = manifest_digest.to_string();
        let (staged, manifest_digest) = tokio::task::spawn_blocking(move || {
            let file = staged.open()?;
            golem_package::verify_manifest_digest(file, &manifest_digest)?;
            Ok::<_, anyhow::Error>((staged, manifest_digest))
        })
        .await??;
        let (archive, _) = storage.store_staged_published_blob(staged).await?;
        Ok(Self {
            archive,
            manifest_digest,
        })
    }
}

#[derive(Debug, PartialEq, Eq, sqlx::FromRow)]
pub struct PackageVersionRecord {
    pub package_path: String,
    pub publisher_account_id: Uuid,
    pub version: String,
    pub manifest_digest: String,
    pub archive_digest: String,
    pub lifecycle: i16,
    pub description: Option<String>,
    pub callables: Json<Vec<String>>,
}

#[derive(Debug, thiserror::Error)]
pub enum InsertPackageVersionError {
    #[error("Package path belongs to another publisher")]
    PublisherMismatch,
    #[error(transparent)]
    Repo(#[from] RepoError),
}

#[async_trait]
pub trait PackageRepo: Send + Sync {
    /// Inserts an immutable version only after its archive has been verified and stored.
    async fn insert_version(
        &self,
        record: &NewPackageVersion,
        archive: &StoredPackageArchive,
    ) -> Result<(), InsertPackageVersionError>;

    async fn get_version(
        &self,
        path: &str,
        version: &str,
    ) -> RepoResult<Option<PackageVersionRecord>>;
}

pub struct DbPackageRepo<DBP: Pool> {
    db_pool: DBP,
}

impl<DBP: Pool> DbPackageRepo<DBP> {
    pub fn new(db_pool: DBP) -> Self {
        Self { db_pool }
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl PackageRepo for DbPackageRepo<PostgresPool> {
    async fn insert_version(
        &self,
        record: &NewPackageVersion,
        archive: &StoredPackageArchive,
    ) -> Result<(), InsertPackageVersionError> {
        let record = record.clone();
        let manifest_digest = archive.manifest_digest.clone();
        let archive_digest = archive.archive.digest().to_owned();
        self.db_pool.with_tx_err("packages", "insert_version", |tx| Box::pin(async move {
            tx.execute(sqlx::query(
                "INSERT INTO packages (package_path, publisher_account_id) VALUES ($1, $2)
                 ON CONFLICT (package_path) DO NOTHING",
            ).bind(&record.package_path).bind(record.publisher_account_id)).await?;
            let result = tx.execute(sqlx::query(
                "INSERT INTO package_versions
                     (package_path, version, manifest_digest, archive_digest, description, callables)
                 SELECT package_path, $3, $4, $5, $6, $7 FROM packages
                 WHERE package_path = $1 AND publisher_account_id = $2",
            ).bind(&record.package_path).bind(record.publisher_account_id).bind(&record.version)
                .bind(manifest_digest).bind(archive_digest).bind(&record.description)
                .bind(Json(&record.callables))).await?;
            if result.rows_affected() == 0 {
                return Err(InsertPackageVersionError::PublisherMismatch);
            }
            Ok(())
        })).await
    }

    async fn get_version(
        &self,
        path: &str,
        version: &str,
    ) -> RepoResult<Option<PackageVersionRecord>> {
        self.db_pool
            .with_ro("packages", "get_version")
            .fetch_optional_as(
                sqlx::query_as(
                    "SELECT v.package_path, p.publisher_account_id, v.version, v.manifest_digest,
                    v.archive_digest, v.lifecycle, v.description, v.callables
             FROM package_versions v JOIN packages p ON p.package_path = v.package_path
             WHERE v.package_path = $1 AND v.version = $2",
                )
                .bind(path)
                .bind(version),
            )
            .await
    }
}
