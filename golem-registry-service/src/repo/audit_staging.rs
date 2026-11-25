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

//! Audit records wait in the staging table until the account audit export takes them. A row
//! leaves only after the export acknowledges it; the table is not a history to query.

use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{LabelledPoolApi, Pool, PoolApi};
use golem_service_base::model::auth::audit::AuditRecord;
use golem_service_base::repo::{RepoResult, SqlDateTime};
use indoc::indoc;
use tracing::Instrument;
use tracing::info_span;
use uuid::Uuid;

/// One staged audit record: the full record as JSON, and the columns that the export selects by.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct AuditStagingRecord {
    pub event_id: Uuid,
    pub recorded_at: SqlDateTime,
    pub surface: String,
    pub operation: String,
    pub outcome: String,
    pub target_account_id: Option<Uuid>,
    pub target_environment_id: Option<Uuid>,
    pub record: String,
}

impl AuditStagingRecord {
    pub fn from_audit_record(record: &AuditRecord) -> Result<Self, serde_json::Error> {
        Ok(Self {
            event_id: record.event_id,
            recorded_at: SqlDateTime::new(record.recorded_at),
            surface: record.surface.clone(),
            operation: record.operation.clone(),
            outcome: record.outcome.label().to_string(),
            target_account_id: record.target_account_id().map(|account_id| account_id.0),
            target_environment_id: record
                .target_environment_id()
                .map(|environment_id| environment_id.0),
            record: serde_json::to_string(record)?,
        })
    }

    pub fn to_audit_record(&self) -> Result<AuditRecord, serde_json::Error> {
        serde_json::from_str(&self.record)
    }
}

#[async_trait]
pub trait AuditStagingRepo: Send + Sync {
    /// Stores the record. A record whose event is already stored stays as it is, so a sender
    /// may send the same record again after a lost answer.
    async fn insert(&self, record: &AuditStagingRecord) -> RepoResult<()>;

    async fn get(&self, event_id: Uuid) -> RepoResult<Option<AuditStagingRecord>>;
}

pub struct LoggedAuditStagingRepo<Repo: AuditStagingRepo> {
    repo: Repo,
}

static SPAN_NAME: &str = "audit staging repository";

impl<Repo: AuditStagingRepo> LoggedAuditStagingRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }
}

#[async_trait]
#[golem_async_trait::region_simple]
impl<Repo: AuditStagingRepo> AuditStagingRepo for LoggedAuditStagingRepo<Repo> {
    async fn insert(&self, record: &AuditStagingRecord) -> RepoResult<()> {
        self.repo
            .insert(record)
            .instrument(info_span!(SPAN_NAME, event_id = %record.event_id))
            .await
    }

    async fn get(&self, event_id: Uuid) -> RepoResult<Option<AuditStagingRecord>> {
        self.repo
            .get(event_id)
            .instrument(info_span!(SPAN_NAME, %event_id))
            .await
    }
}

pub struct DbAuditStagingRepo<DBP: Pool> {
    db_pool: DBP,
}

static METRICS_SVC_NAME: &str = "audit_staging";

/// A record that names an environment but no account (the target of an agent, a tool or an
/// invocation names only its environment) gets the environment's account in the same
/// statement, so that the export finds the account it routes by. An environment that the
/// registry does not know gives no account, and the record is still stored. The record itself
/// stays as the decision named it. `WHERE true` keeps SQLite from reading `ON CONFLICT` as a
/// join constraint of the `SELECT`.
const INSERT: &str = indoc! { r#"
    INSERT INTO audit_staging
        (event_id, recorded_at, surface, operation, outcome,
         target_account_id, target_environment_id, record)
    SELECT $1, $2, $3, $4, $5,
           COALESCE($6, (SELECT applications.account_id
                         FROM environments
                         JOIN applications
                           ON applications.application_id = environments.application_id
                         WHERE environments.environment_id = $7)),
           $7, $8
    WHERE true
    ON CONFLICT (event_id) DO NOTHING
"#};

impl<DBP: Pool> DbAuditStagingRepo<DBP> {
    pub fn new(db_pool: DBP) -> Self {
        Self { db_pool }
    }

    pub fn logged(db_pool: DBP) -> LoggedAuditStagingRepo<Self>
    where
        Self: AuditStagingRepo,
    {
        LoggedAuditStagingRepo::new(Self::new(db_pool))
    }

    fn with_ro(&self, api_name: &'static str) -> DBP::LabelledApi {
        self.db_pool.with_ro(METRICS_SVC_NAME, api_name)
    }

    fn with_rw(&self, api_name: &'static str) -> DBP::LabelledApi {
        self.db_pool.with_rw(METRICS_SVC_NAME, api_name)
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
impl DbAuditStagingRepo<PostgresPool> {
    /// Stores the record inside the caller's transaction, so that it commits or rolls back
    /// together with the change it records.
    pub async fn insert_in_tx(
        tx: &mut <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction,
        record: &AuditStagingRecord,
    ) -> RepoResult<()> {
        tx.execute(
            sqlx::query(INSERT)
                .bind(record.event_id)
                .bind(&record.recorded_at)
                .bind(&record.surface)
                .bind(&record.operation)
                .bind(&record.outcome)
                .bind(record.target_account_id)
                .bind(record.target_environment_id)
                .bind(&record.record),
        )
        .await?;
        Ok(())
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
#[golem_async_trait::region_simple]
impl AuditStagingRepo for DbAuditStagingRepo<PostgresPool> {
    async fn insert(&self, record: &AuditStagingRecord) -> RepoResult<()> {
        self.with_rw("insert")
            .execute(
                sqlx::query(INSERT)
                    .bind(record.event_id)
                    .bind(&record.recorded_at)
                    .bind(&record.surface)
                    .bind(&record.operation)
                    .bind(&record.outcome)
                    .bind(record.target_account_id)
                    .bind(record.target_environment_id)
                    .bind(&record.record),
            )
            .await?;
        Ok(())
    }

    async fn get(&self, event_id: Uuid) -> RepoResult<Option<AuditStagingRecord>> {
        self.with_ro("get")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT event_id, recorded_at, surface, operation, outcome,
                           target_account_id, target_environment_id, record
                    FROM audit_staging
                    WHERE event_id = $1
                "#})
                .bind(event_id),
            )
            .await
    }
}
