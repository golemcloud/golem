// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use super::model::domain_registration::{DomainRegistrationRecord, DomainRegistrationRepoError};
use crate::repo::model::BindFields;
use crate::repo::model::datetime::SqlDateTime;
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{Pool, PoolApi};
use golem_service_base::repo::ResultExt;
use indoc::indoc;
use tracing::{Instrument, Span, info_span};
use uuid::Uuid;

#[async_trait]
pub trait DomainRegistrationRepo: Send + Sync {
    async fn create(
        &self,
        record: DomainRegistrationRecord,
    ) -> Result<DomainRegistrationRecord, DomainRegistrationRepoError>;

    async fn delete(
        &self,
        domain_registration_id: Uuid,
        actor: Uuid,
    ) -> Result<Option<DomainRegistrationRecord>, DomainRegistrationRepoError>;

    async fn get_by_id(
        &self,
        domain_registration_id: Uuid,
    ) -> Result<Option<DomainRegistrationRecord>, DomainRegistrationRepoError>;

    async fn get_in_environment(
        &self,
        environment_id: Uuid,
        domain: &str,
    ) -> Result<Option<DomainRegistrationRecord>, DomainRegistrationRepoError>;

    async fn list_by_environment(
        &self,
        environment_id: Uuid,
    ) -> Result<Vec<DomainRegistrationRecord>, DomainRegistrationRepoError>;
}

pub struct LoggedDomainRegistrationRepo<Repo: DomainRegistrationRepo> {
    repo: Repo,
}

static SPAN_NAME: &str = "domain regiration repository";

impl<Repo: DomainRegistrationRepo> LoggedDomainRegistrationRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }

    fn span_id(domain_registration_id: Uuid) -> Span {
        info_span!(SPAN_NAME, domain_registration_id=%domain_registration_id)
    }

    fn span_environment(environment_id: Uuid) -> Span {
        info_span!(SPAN_NAME, environment_id=%environment_id)
    }
}

#[async_trait]
impl<Repo: DomainRegistrationRepo> DomainRegistrationRepo for LoggedDomainRegistrationRepo<Repo> {
    async fn create(
        &self,
        record: DomainRegistrationRecord,
    ) -> Result<DomainRegistrationRecord, DomainRegistrationRepoError> {
        let span = Self::span_id(record.domain_registration_id);
        self.repo.create(record).instrument(span).await
    }

    async fn delete(
        &self,
        domain_registration_id: Uuid,
        actor: Uuid,
    ) -> Result<Option<DomainRegistrationRecord>, DomainRegistrationRepoError> {
        let span = Self::span_id(domain_registration_id);
        self.repo
            .delete(domain_registration_id, actor)
            .instrument(span)
            .await
    }

    async fn get_by_id(
        &self,
        domain_registration_id: Uuid,
    ) -> Result<Option<DomainRegistrationRecord>, DomainRegistrationRepoError> {
        let span = Self::span_id(domain_registration_id);
        self.repo
            .get_by_id(domain_registration_id)
            .instrument(span)
            .await
    }

    async fn get_in_environment(
        &self,
        environment_id: Uuid,
        domain: &str,
    ) -> Result<Option<DomainRegistrationRecord>, DomainRegistrationRepoError> {
        let span = Self::span_environment(environment_id);
        self.repo
            .get_in_environment(environment_id, domain)
            .instrument(span)
            .await
    }

    async fn list_by_environment(
        &self,
        environment_id: Uuid,
    ) -> Result<Vec<DomainRegistrationRecord>, DomainRegistrationRepoError> {
        let span = Self::span_environment(environment_id);
        self.repo
            .list_by_environment(environment_id)
            .instrument(span)
            .await
    }
}

pub struct DbDomainRegistrationRepo<DBP: Pool> {
    db_pool: DBP,
}

static METRICS_SVC_NAME: &str = "domain_registrations";

impl<DBP: Pool> DbDomainRegistrationRepo<DBP> {
    pub fn new(db_pool: DBP) -> Self {
        Self { db_pool }
    }

    pub fn logged(db_pool: DBP) -> LoggedDomainRegistrationRepo<Self>
    where
        Self: DomainRegistrationRepo,
    {
        LoggedDomainRegistrationRepo::new(Self::new(db_pool))
    }

    fn with_ro(&self, api_name: &'static str) -> DBP::LabelledApi {
        self.db_pool.with_ro(METRICS_SVC_NAME, api_name)
    }

    fn with_rw(&self, api_name: &'static str) -> DBP::LabelledApi {
        self.db_pool.with_rw(METRICS_SVC_NAME, api_name)
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl DomainRegistrationRepo for DbDomainRegistrationRepo<PostgresPool> {
    async fn create(
        &self,
        record: DomainRegistrationRecord,
    ) -> Result<DomainRegistrationRecord, DomainRegistrationRepoError> {
        self.with_rw("create")
            .fetch_one_as(
                sqlx::query_as(indoc! {r#"
                INSERT INTO domain_registrations (
                    domain_registration_id, environment_id, domain,
                    created_at, created_by, deleted_at, deleted_by
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7)
                RETURNING
                    domain_registration_id, environment_id, domain,
                    created_at, created_by, deleted_at, deleted_by
            "#})
                .bind(record.domain_registration_id)
                .bind(record.environment_id)
                .bind(record.domain)
                .bind_immutable_audit(record.audit),
            )
            .await
            .to_error_on_unique_violation(DomainRegistrationRepoError::DomainAlreadyExists)
    }

    async fn delete(
        &self,
        domain_registration_id: Uuid,
        actor: Uuid,
    ) -> Result<Option<DomainRegistrationRecord>, DomainRegistrationRepoError> {
        let deleted_at = SqlDateTime::now();

        let result = self
            .with_ro("get_by_id")
            .fetch_optional_as(
                sqlx::query_as(indoc! {r#"
                    UPDATE domain_registrations
                    SET
                        deleted_at = $2, deleted_by = $3
                    WHERE
                        domain_registration_id = $1
                        AND deleted_at IS NULL
                    RETURNING
                        domain_registration_id, environment_id, domain,
                        created_at, created_by, deleted_at, deleted_by
                "#})
                .bind(domain_registration_id)
                .bind(deleted_at)
                .bind(actor),
            )
            .await?;

        Ok(result)
    }

    async fn get_by_id(
        &self,
        domain_registration_id: Uuid,
    ) -> Result<Option<DomainRegistrationRecord>, DomainRegistrationRepoError> {
        let result = self
            .with_ro("get_by_id")
            .fetch_optional_as(
                sqlx::query_as(indoc! {r#"
                    SELECT
                        domain_registration_id, environment_id, domain,
                        created_at, created_by, deleted_at, deleted_by
                    FROM domain_registrations
                    WHERE
                        domain_registration_id = $1
                        AND deleted_at IS NULL
                "#})
                .bind(domain_registration_id),
            )
            .await?;

        Ok(result)
    }

    async fn get_in_environment(
        &self,
        environment_id: Uuid,
        domain: &str,
    ) -> Result<Option<DomainRegistrationRecord>, DomainRegistrationRepoError> {
        let result = self
            .with_ro("get_in_environment")
            .fetch_optional_as(
                sqlx::query_as(indoc! {r#"
                    SELECT
                        domain_registration_id, environment_id, domain,
                        created_at, created_by, deleted_at, deleted_by
                    FROM domain_registrations
                    WHERE
                        environment_id = $1
                        AND domain = $2
                        AND deleted_at IS NULL
                "#})
                .bind(environment_id)
                .bind(domain),
            )
            .await?;

        Ok(result)
    }

    async fn list_by_environment(
        &self,
        environment_id: Uuid,
    ) -> Result<Vec<DomainRegistrationRecord>, DomainRegistrationRepoError> {
        let result = self
            .with_ro("list_by_environment")
            .fetch_all_as(
                sqlx::query_as(indoc! {r#"
                    SELECT
                        domain_registration_id, environment_id, domain,
                        created_at, created_by, deleted_at, deleted_by
                    FROM domain_registrations
                    WHERE
                        environment_id = $1
                        AND deleted_at IS NULL
                "#})
                .bind(environment_id),
            )
            .await?;

        Ok(result)
    }
}
