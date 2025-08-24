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

use super::model::RecordWithEnvironmentAuth;
use crate::repo::model::BindFields;
pub use crate::repo::model::environment::{
    EnvironmentExtRevisionRecord, EnvironmentPluginInstallationRecord,
    EnvironmentPluginInstallationRevisionRecord, EnvironmentRecord, EnvironmentRevisionRecord,
};
use async_trait::async_trait;
use conditional_trait_gen::trait_gen;
use futures::FutureExt;
use futures::future::BoxFuture;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{LabelledPoolApi, LabelledPoolTransaction, Pool, PoolApi};
use golem_service_base::repo::{RepoResult, ResultExt};
use indoc::indoc;
use sqlx::Database;
use tracing::{Instrument, Span, info_span};
use uuid::Uuid;

#[async_trait]
pub trait EnvironmentRepo: Send + Sync {
    async fn get_by_name(
        &self,
        application_id: &Uuid,
        name: &str,
        actor: &Uuid,
        override_visibility: bool,
        include_deleted: bool,
    ) -> RepoResult<Option<RecordWithEnvironmentAuth<EnvironmentExtRevisionRecord>>>;

    async fn get_by_id(
        &self,
        environment_id: &Uuid,
        actor: &Uuid,
        override_visibility: bool,
        include_deleted: bool,
    ) -> RepoResult<Option<RecordWithEnvironmentAuth<EnvironmentExtRevisionRecord>>>;

    async fn list_by_app(
        &self,
        application_id: &Uuid,
        actor: &Uuid,
        override_visibility: bool,
        include_deleted: bool,
    ) -> RepoResult<Vec<RecordWithEnvironmentAuth<EnvironmentExtRevisionRecord>>>;

    async fn create(
        &self,
        application_id: &Uuid,
        name: &str,
        revision: EnvironmentRevisionRecord,
    ) -> RepoResult<Option<EnvironmentExtRevisionRecord>>;

    async fn update(
        &self,
        current_revision_id: i64,
        revision: EnvironmentRevisionRecord,
    ) -> RepoResult<Option<EnvironmentExtRevisionRecord>>;

    async fn delete(
        &self,
        user_account_id: &Uuid,
        environment_id: &Uuid,
        current_revision_id: i64,
    ) -> RepoResult<bool>;

    async fn get_current_plugin_installations(
        &self,
        environment_id: &Uuid,
        actor: &Uuid,
        override_visibility: bool,
        include_deleted: bool,
    ) -> RepoResult<Option<RecordWithEnvironmentAuth<EnvironmentPluginInstallationRecord>>>;

    async fn create_plugin_installations(
        &self,
        plugin_installation: EnvironmentPluginInstallationRecord,
    ) -> RepoResult<Option<EnvironmentPluginInstallationRecord>>;

    async fn update_plugin_installations(
        &self,
        current_revision_id: i64,
        plugin_installation: EnvironmentPluginInstallationRecord,
    ) -> RepoResult<Option<EnvironmentPluginInstallationRecord>>;
}

pub struct LoggedEnvironmentRepo<Repo: EnvironmentRepo> {
    repo: Repo,
}

static SPAN_NAME: &str = "environment repository";

impl<Repo: EnvironmentRepo> LoggedEnvironmentRepo<Repo> {
    pub fn new(repo: Repo) -> Self {
        Self { repo }
    }

    fn span_name(application_id: &Uuid, name: &str) -> Span {
        info_span!(SPAN_NAME, application_id = %application_id, name)
    }

    fn span_env(environment_id: &Uuid) -> Span {
        info_span!(SPAN_NAME, environment_id = %environment_id)
    }

    fn span_plugin_installation(
        environment_id: &Uuid,
        plugin_installation_revision_id: i64,
    ) -> Span {
        info_span!(SPAN_NAME, environment_id = %environment_id, plugin_installation_revision_id)
    }

    fn span_app_id(application_id: &Uuid) -> Span {
        info_span!(SPAN_NAME, application_id = %application_id)
    }
}

#[async_trait]
impl<Repo: EnvironmentRepo> EnvironmentRepo for LoggedEnvironmentRepo<Repo> {
    async fn get_by_name(
        &self,
        application_id: &Uuid,
        name: &str,
        actor: &Uuid,
        override_visibility: bool,
        include_deleted: bool,
    ) -> RepoResult<Option<RecordWithEnvironmentAuth<EnvironmentExtRevisionRecord>>> {
        self.repo
            .get_by_name(
                application_id,
                name,
                actor,
                override_visibility,
                include_deleted,
            )
            .instrument(Self::span_name(application_id, name))
            .await
    }

    async fn get_by_id(
        &self,
        environment_id: &Uuid,
        actor: &Uuid,
        override_visibility: bool,
        include_deleted: bool,
    ) -> RepoResult<Option<RecordWithEnvironmentAuth<EnvironmentExtRevisionRecord>>> {
        self.repo
            .get_by_id(environment_id, actor, override_visibility, include_deleted)
            .instrument(Self::span_env(environment_id))
            .await
    }

    async fn list_by_app(
        &self,
        application_id: &Uuid,
        actor: &Uuid,
        override_visibility: bool,
        include_deleted: bool,
    ) -> RepoResult<Vec<RecordWithEnvironmentAuth<EnvironmentExtRevisionRecord>>> {
        self.repo
            .list_by_app(application_id, actor, override_visibility, include_deleted)
            .instrument(Self::span_env(application_id))
            .await
    }

    async fn create(
        &self,
        application_id: &Uuid,
        name: &str,
        revision: EnvironmentRevisionRecord,
    ) -> RepoResult<Option<EnvironmentExtRevisionRecord>> {
        self.repo
            .create(application_id, name, revision)
            .instrument(Self::span_app_id(application_id))
            .await
    }

    async fn update(
        &self,
        current_revision_id: i64,
        revision: EnvironmentRevisionRecord,
    ) -> RepoResult<Option<EnvironmentExtRevisionRecord>> {
        let span = Self::span_env(&revision.environment_id);
        self.repo
            .update(current_revision_id, revision)
            .instrument(span)
            .await
    }

    async fn delete(
        &self,
        user_account_id: &Uuid,
        environment_id: &Uuid,
        current_revision_id: i64,
    ) -> RepoResult<bool> {
        let span = Self::span_env(user_account_id);
        self.repo
            .delete(user_account_id, environment_id, current_revision_id)
            .instrument(span)
            .await
    }

    async fn get_current_plugin_installations(
        &self,
        environment_id: &Uuid,
        actor: &Uuid,
        override_visibility: bool,
        include_deleted: bool,
    ) -> RepoResult<Option<RecordWithEnvironmentAuth<EnvironmentPluginInstallationRecord>>> {
        self.repo
            .get_current_plugin_installations(
                environment_id,
                actor,
                override_visibility,
                include_deleted,
            )
            .instrument(Self::span_env(environment_id))
            .await
    }

    async fn create_plugin_installations(
        &self,
        plugin_installation: EnvironmentPluginInstallationRecord,
    ) -> RepoResult<Option<EnvironmentPluginInstallationRecord>> {
        let span = Self::span_plugin_installation(
            &plugin_installation.environment_id,
            plugin_installation.current_revision_id,
        );
        self.repo
            .create_plugin_installations(plugin_installation)
            .instrument(span)
            .await
    }

    async fn update_plugin_installations(
        &self,
        current_revision_id: i64,
        plugin_installation: EnvironmentPluginInstallationRecord,
    ) -> RepoResult<Option<EnvironmentPluginInstallationRecord>> {
        let span = Self::span_plugin_installation(
            &plugin_installation.environment_id,
            plugin_installation.current_revision_id,
        );
        self.repo
            .update_plugin_installations(current_revision_id, plugin_installation)
            .instrument(span)
            .await
    }
}

pub struct DbEnvironmentRepo<DBP: Pool> {
    db_pool: DBP,
}

static METRICS_SVC_NAME: &str = "environment";

impl<DBP: Pool> DbEnvironmentRepo<DBP> {
    pub fn new(db_pool: DBP) -> Self {
        Self { db_pool }
    }

    pub fn logged(db_pool: DBP) -> LoggedEnvironmentRepo<Self>
    where
        Self: EnvironmentRepo,
    {
        LoggedEnvironmentRepo::new(Self::new(db_pool))
    }

    fn with_ro(&self, api_name: &'static str) -> DBP::LabelledApi {
        self.db_pool.with_ro(METRICS_SVC_NAME, api_name)
    }

    async fn with_tx<R, F>(&self, api_name: &'static str, f: F) -> RepoResult<R>
    where
        R: Send,
        F: for<'f> FnOnce(
                &'f mut <DBP::LabelledApi as LabelledPoolApi>::LabelledTransaction,
            ) -> BoxFuture<'f, RepoResult<R>>
            + Send,
    {
        self.db_pool.with_tx(METRICS_SVC_NAME, api_name, f).await
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl EnvironmentRepo for DbEnvironmentRepo<PostgresPool> {
    async fn get_by_name(
        &self,
        application_id: &Uuid,
        name: &str,
        actor: &Uuid,
        override_visibility: bool,
        include_deleted: bool,
    ) -> RepoResult<Option<RecordWithEnvironmentAuth<EnvironmentExtRevisionRecord>>> {
        self.with_ro("get_by_name")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT
                        e.name, e.application_id,
                        r.environment_id, r.revision_id, r.hash,
                        r.created_at, r.created_by, r.deleted,
                        r.compatibility_check, r.version_check, r.security_overrides,
                        a.account_id as owner_account_id,
                        COALESCE(esr.roles, 0) AS environment_roles_from_shares
                    FROM accounts a
                    JOIN applications ap
                        ON ap.account_id = a.account_id
                    JOIN environments e
                        ON e.application_id = ap.application_id
                    JOIN environment_revisions r
                        ON r.environment_id = e.environment_id
                        AND r.revision_id = e.current_revision_id
                    LEFT JOIN environment_shares es
                        ON es.environment_id = e.environment_id
                        AND es.grantee_account_id = $2
                    LEFT JOIN environment_share_revisions esr
                        ON esr.environment_share_id = es.environment_id
                        AND esr.revision_id = es.current_revision_id
                    WHERE
                        a.application_id = $1
                        AND e.name = $2
                        -- check deletion
                        AND (
                            $5
                            OR (
                                a.deleted_at IS NULL
                                AND ap.deleted_at IS NULL
                                AND e.deleted_at IS NULL
                            )
                        )
                        -- check visibility
                        AND (
                            $4                                 -- override
                            OR a.account_id = $3      -- owner is querying
                            OR esr.roles IS NOT NULL  -- share exists
                        )
                "# })
                .bind(application_id)
                .bind(name)
                .bind(actor)
                .bind(override_visibility)
                .bind(include_deleted),
            )
            .await
    }

    async fn get_by_id(
        &self,
        environment_id: &Uuid,
        actor: &Uuid,
        override_visibility: bool,
        include_deleted: bool,
    ) -> RepoResult<Option<RecordWithEnvironmentAuth<EnvironmentExtRevisionRecord>>> {
        self.with_ro("get_by_id")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT
                        e.name, e.application_id,
                        r.environment_id, r.revision_id, r.hash,
                        r.created_at, r.created_by, r.deleted,
                        r.compatibility_check, r.version_check, r.security_overrides,
                        a.account_id as owner_account_id,
                        COALESCE(esr.roles, 0) AS environment_roles_from_shares
                    FROM accounts a
                    JOIN applications ap
                        ON ap.account_id = a.account_id
                    JOIN environments e
                        ON e.application_id = ap.application_id
                    JOIN environment_revisions r
                        ON r.environment_id = e.environment_id
                        AND r.revision_id = e.current_revision_id
                    LEFT JOIN environment_shares es
                        ON es.environment_id = e.environment_id
                        AND es.grantee_account_id = $2
                    LEFT JOIN environment_share_revisions esr
                        ON esr.environment_share_id = es.environment_id
                        AND esr.revision_id = es.current_revision_id
                    WHERE
                        e.environment_id = $1
                        -- check deletion
                        AND (
                            $4
                            OR (
                                a.deleted_at IS NULL
                                AND ap.deleted_at IS NULL
                                AND e.deleted_at IS NULL
                            )
                        )
                        -- check visibility
                        AND (
                            $3                                 -- override
                            OR a.account_id = $2      -- owner is querying
                            OR esr.roles IS NOT NULL  -- share exists
                        )
                "# })
                .bind(environment_id)
                .bind(actor)
                .bind(override_visibility)
                .bind(include_deleted),
            )
            .await
    }

    async fn list_by_app(
        &self,
        application_id: &Uuid,
        actor: &Uuid,
        override_visibility: bool,
        include_deleted: bool,
    ) -> RepoResult<Vec<RecordWithEnvironmentAuth<EnvironmentExtRevisionRecord>>> {
        self.with_ro("list_by_owner")
            .fetch_all_as(
                sqlx::query_as(indoc! { r#"
                    SELECT
                        e.name, e.application_id,
                        r.environment_id, r.revision_id, r.hash,
                        r.created_at, r.created_by, r.deleted,
                        r.compatibility_check, r.version_check, r.security_overrides,
                        a.account_id as owner_account_id,
                        COALESCE(esr.roles, 0) AS environment_roles_from_shares
                    FROM accounts a
                    JOIN applications ap
                        ON ap.account_id = a.account_id
                    JOIN environments e
                        ON e.application_id = ap.application_id
                    JOIN environment_revisions r
                        ON r.environment_id = e.environment_id
                        AND r.revision_id = e.current_revision_id
                    LEFT JOIN environment_shares es
                        ON es.environment_id = e.environment_id
                        AND es.grantee_account_id = $2
                    LEFT JOIN environment_share_revisions esr
                        ON esr.environment_share_id = es.environment_id
                        AND esr.revision_id = es.current_revision_id
                    WHERE
                        ap.application_id = $1
                        -- check deletion
                        AND (
                            $4
                            OR (
                                a.deleted_at IS NULL
                                AND ap.deleted_at IS NULL
                                AND e.deleted_at IS NULL
                            )
                        )
                        -- check visibility
                        AND (
                            $3                                 -- override
                            OR a.account_id = $2      -- owner is querying
                            OR esr.roles IS NOT NULL  -- share exists
                        )
                    ORDER BY e.name
                "#})
                .bind(application_id)
                .bind(actor)
                .bind(override_visibility)
                .bind(include_deleted),
            )
            .await
    }

    async fn create(
        &self,
        application_id: &Uuid,
        name: &str,
        revision: EnvironmentRevisionRecord,
    ) -> RepoResult<Option<EnvironmentExtRevisionRecord>> {
        let application_id = *application_id;
        let name = name.to_owned();
        let revision = revision.ensure_first();

        self.with_tx("create", |tx| async move {
            tx.execute(
                sqlx::query(indoc! { r#"
                    INSERT INTO environments
                    (environment_id, name, application_id, created_at, updated_at, deleted_at, modified_by, current_revision_id)
                    VALUES ($1, $2, $3, $4, $5, NULL, $6, 0)
                "# })
                    .bind(revision.environment_id)
                    .bind(&name)
                    .bind(application_id)
                    .bind(&revision.audit.created_at)
                    .bind(&revision.audit.created_at)
                    .bind(revision.audit.created_by)
            ).await?;

            let revision = Self::insert_revision(tx, revision).await?;

            Ok(EnvironmentExtRevisionRecord {
                name,
                application_id,
                revision,
            })
        }.boxed()).await.none_on_unique_violation()
    }

    async fn update(
        &self,
        current_revision_id: i64,
        revision: EnvironmentRevisionRecord,
    ) -> RepoResult<Option<EnvironmentExtRevisionRecord>> {
        let Some(checked_env) = self
            .check_current_revision(&revision.environment_id, current_revision_id)
            .await?
        else {
            return Ok(None);
        };

        self.with_tx("update", |tx| {
            async move {
                let revision: EnvironmentRevisionRecord =
                    Self::insert_revision(tx, revision.ensure_new(current_revision_id)).await?;

                tx.execute(
                    sqlx::query(indoc! { r#"
                        UPDATE environments
                        SET updated_at = $1, modified_by = $2, current_revision_id = $3
                        WHERE environment_id = $4
                    "#})
                    .bind(&revision.audit.created_at)
                    .bind(revision.audit.created_by)
                    .bind(revision.revision_id)
                    .bind(revision.environment_id),
                )
                .await?;

                Ok(EnvironmentExtRevisionRecord {
                    name: checked_env.name,
                    application_id: checked_env.application_id,
                    revision,
                })
            }
            .boxed()
        })
        .await
        .none_on_unique_violation()
    }

    async fn delete(
        &self,
        user_account_id: &Uuid,
        environment_id: &Uuid,
        current_revision_id: i64,
    ) -> RepoResult<bool> {
        let user_account_id = *user_account_id;
        let environment_id = *environment_id;

        let Some(_checked_env) = self
            .check_current_revision(&environment_id, current_revision_id)
            .await?
        else {
            return Ok(false);
        };

        self.with_tx("delete", |tx| {
            async move {
                let revision: EnvironmentRevisionRecord = Self::insert_revision(
                    tx,
                    EnvironmentRevisionRecord::deletion(
                        user_account_id,
                        environment_id,
                        current_revision_id,
                    ),
                )
                .await?;

                tx.execute(
                    sqlx::query(indoc! { r#"
                        UPDATE environments
                        SET deleted_at = $1, modified_by = $2, current_revision_id = $3
                        WHERE environment_id = $4
                    "#})
                    .bind(&revision.audit.created_at)
                    .bind(revision.audit.created_by)
                    .bind(revision.revision_id)
                    .bind(revision.environment_id),
                )
                .await?;

                Ok(())
            }
            .boxed()
        })
        .await
        .false_on_unique_violation()
    }

    async fn get_current_plugin_installations(
        &self,
        environment_id: &Uuid,
        actor: &Uuid,
        override_visibility: bool,
        include_deleted: bool,
    ) -> RepoResult<Option<RecordWithEnvironmentAuth<EnvironmentPluginInstallationRecord>>> {
        let plugin_installation: Option<
            RecordWithEnvironmentAuth<EnvironmentPluginInstallationRecord>,
        > = self
            .with_ro("get_current_plugin_installations")
            .fetch_optional_as(
                sqlx::query_as(indoc! { r#"
                    SELECT
                        epi.environment_id, epi.hash, epi.created_at, epi.updated_at,
                        epi.deleted_at, epi.modified_by, epi.current_revision_id,
                        a.account_id as owner_account_id,
                        COALESCE(esr.roles, 0) AS environment_roles_from_shares
                    FROM accounts a
                    JOIN applications ap
                        ON ap.account_id = a.account_id
                    JOIN environments e
                        ON e.application_id = ap.application_id
                    JOIN environment_plugin_installations epi
                        ON epi.envionment_id = e.environment_id
                    LEFT JOIN environment_shares es
                        ON es.environment_id = e.environment_id
                        AND es.grantee_account_id = $2
                    LEFT JOIN environment_share_revisions esr
                        ON esr.environment_share_id = es.environment_id
                        AND esr.revision_id = es.current_revision_id
                    WHERE
                        e.environment_id = $1
                        -- check deletion
                        AND (
                            $4
                            OR (
                                a.deleted_at IS NULL
                                AND ap.deleted_at IS NULL
                                AND e.deleted_at IS NULL
                                AND epi.deleted_at IS NULL
                            )
                        )
                        -- check visibility
                        AND (
                            $3                                 -- override
                            OR a.account_id = $2      -- owner is querying
                            OR esr.roles IS NOT NULL  -- share exists
                        )
                "#})
                .bind(environment_id)
                .bind(actor)
                .bind(override_visibility)
                .bind(include_deleted),
            )
            .await?;

        match plugin_installation {
            Some(mut plugin_installation) => {
                plugin_installation.value = self.with_plugins(plugin_installation.value).await?;
                Ok(Some(plugin_installation))
            }
            None => Ok(None),
        }
    }

    async fn create_plugin_installations(
        &self,
        plugin_installation: EnvironmentPluginInstallationRecord,
    ) -> RepoResult<Option<EnvironmentPluginInstallationRecord>> {
        let plugin_installation = plugin_installation.with_updated_hash();

        let plugin_installation: Option<EnvironmentPluginInstallationRecord> =
            self.with_tx("create_plugin_installations", |tx| {
                async move {
                    let plugins = plugin_installation.plugins;

                    let plugin_installation: EnvironmentPluginInstallationRecord = tx.fetch_one_as(
                        sqlx::query_as(indoc! { r#"
                            INSERT INTO environment_plugin_installations
                            (environment_id, hash, created_at, updated_at, deleted_at, modified_by, current_revision_id)
                            VALUES ($1, $2, $3, $4, NULL, $5, 0)
                            RETURNING environment_id, hash, created_at, updated_at, deleted_at, modified_by, current_revision_id
                        "#})
                            .bind(plugin_installation.environment_id)
                            .bind(plugin_installation.hash)
                            .bind(&plugin_installation.audit.created_at)
                            .bind(&plugin_installation.audit.created_at)
                            .bind(plugin_installation.audit.modified_by)
                    ).await?;

                    for plugin in plugins {
                        Self::insert_plugin_installation_revision(
                            tx,
                            plugin.ensure_environment(
                                plugin_installation.environment_id,
                                plugin_installation.current_revision_id,
                                plugin_installation.audit.modified_by,
                            ),
                        ).await?;
                    }

                    Ok(plugin_installation)
                }.boxed()
            })
                .await
                .none_on_unique_violation()?;

        match plugin_installation {
            Some(plugin_installation) => Ok(Some(self.with_plugins(plugin_installation).await?)),
            None => Ok(None),
        }
    }

    async fn update_plugin_installations(
        &self,
        current_revision_id: i64,
        plugin_installation: EnvironmentPluginInstallationRecord,
    ) -> RepoResult<Option<EnvironmentPluginInstallationRecord>> {
        let Some(checked_current) = self
            .check_current_plugin_installation_revision(
                &plugin_installation.environment_id,
                current_revision_id,
            )
            .await?
        else {
            return Ok(None);
        };

        let plugin_installation = {
            let mut plugin_installation = plugin_installation;
            plugin_installation.current_revision_id = checked_current.current_revision_id + 1;
            plugin_installation.with_updated_hash()
        };

        let plugin_installation: Option<EnvironmentPluginInstallationRecord> =
            self.with_tx("update_plugin_installations", |tx| {
                async move {
                    let plugins = plugin_installation.plugins;

                    let plugin_installation: Option<EnvironmentPluginInstallationRecord> = tx.fetch_optional_as(
                        sqlx::query_as(indoc! { r#"
                            UPDATE environment_plugin_installations
                            SET hash = $1, updated_at = $2, modified_by = $3, current_revision_id = $4
                            WHERE environment_id = $5 AND current_revision_id = $6
                            RETURNING environment_id, hash, created_at, updated_at, deleted_at, modified_by, current_revision_id
                        "#})
                            .bind(plugin_installation.hash)
                            .bind(plugin_installation.audit.updated_at)
                            .bind(plugin_installation.audit.modified_by)
                            .bind(plugin_installation.current_revision_id)
                            .bind(plugin_installation.environment_id)
                            .bind(current_revision_id)
                    ).await?;

                    let Some(plugin_installation) = plugin_installation else {
                        return Ok(None);
                    };

                    for plugin in plugins {
                        Self::insert_plugin_installation_revision(
                            tx,
                            plugin.ensure_environment(
                                plugin_installation.environment_id,
                                plugin_installation.current_revision_id,
                                plugin_installation.audit.modified_by,
                            ),
                        ).await?;
                    }

                    Ok(Some(plugin_installation))
                }.boxed()
            })
                .await
                .none_on_unique_violation()?.flatten();

        match plugin_installation {
            Some(plugin_installation) => Ok(Some(self.with_plugins(plugin_installation).await?)),
            None => Ok(None),
        }
    }
}

#[async_trait]
trait EnvironmentRepoInternal: EnvironmentRepo {
    type Db: Database;
    type Tx: LabelledPoolTransaction;

    async fn check_current_revision(
        &self,
        environment_id: &Uuid,
        current_revision_id: i64,
    ) -> RepoResult<Option<EnvironmentRecord>>;

    async fn insert_revision(
        tx: &mut Self::Tx,
        revision: EnvironmentRevisionRecord,
    ) -> RepoResult<EnvironmentRevisionRecord>;

    async fn check_current_plugin_installation_revision(
        &self,
        environment_id: &Uuid,
        current_revision_id: i64,
    ) -> RepoResult<Option<EnvironmentPluginInstallationRecord>>;

    async fn get_plugins(
        &self,
        environment_id: &Uuid,
        revision_id: i64,
    ) -> RepoResult<Vec<EnvironmentPluginInstallationRevisionRecord>>;

    async fn with_plugins(
        &self,
        mut plugin_installation: EnvironmentPluginInstallationRecord,
    ) -> RepoResult<EnvironmentPluginInstallationRecord> {
        plugin_installation.plugins = self
            .get_plugins(
                &plugin_installation.environment_id,
                plugin_installation.current_revision_id,
            )
            .await?;
        Ok(plugin_installation)
    }

    async fn insert_plugin_installation_revision(
        tx: &mut Self::Tx,
        revision: EnvironmentPluginInstallationRevisionRecord,
    ) -> RepoResult<()>;
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl EnvironmentRepoInternal for DbEnvironmentRepo<PostgresPool> {
    type Db = <PostgresPool as Pool>::Db;
    type Tx = <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction;

    async fn check_current_revision(
        &self,
        environment_id: &Uuid,
        current_revision_id: i64,
    ) -> RepoResult<Option<EnvironmentRecord>> {
        self.with_ro("check_current_revision").fetch_optional_as(
            sqlx::query_as(indoc! { r#"
                SELECT environment_id, name, application_id, created_at, updated_at, deleted_at, modified_by, current_revision_id
                FROM environments
                WHERE environment_id = $1 AND current_revision_id = $2 and deleted_at IS NULL
            "#})
                .bind(environment_id)
                .bind(current_revision_id),
        )
            .await
    }

    async fn insert_revision(
        tx: &mut Self::Tx,
        revision: EnvironmentRevisionRecord,
    ) -> RepoResult<EnvironmentRevisionRecord> {
        let revision = revision.with_updated_hash();

        tx.fetch_one_as(sqlx::query_as(indoc! { r#"
            INSERT INTO environment_revisions
            (environment_id, revision_id, hash, created_at, created_by, deleted, compatibility_check, version_check, security_overrides)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            RETURNING environment_id, revision_id, hash, created_at, created_by, deleted, compatibility_check, version_check, security_overrides
        "# })
            .bind(revision.environment_id)
            .bind(revision.revision_id)
            .bind(revision.hash)
            .bind_deletable_revision_audit(revision.audit)
            .bind(revision.compatibility_check)
            .bind(revision.version_check)
            .bind(revision.security_overrides)).await
    }

    async fn check_current_plugin_installation_revision(
        &self,
        environment_id: &Uuid,
        current_revision_id: i64,
    ) -> RepoResult<Option<EnvironmentPluginInstallationRecord>> {
        self.with_ro("check_current_plugin_installation_revision").fetch_optional_as(
            sqlx::query_as(indoc! { r#"
                SELECT environment_id, hash, created_at, updated_at, deleted_at, modified_by, current_revision_id
                FROM environment_plugin_installations
                WHERE environment_id = $1 AND current_revision_id = $2 and deleted_at IS NULL
            "#})
                .bind(environment_id)
                .bind(current_revision_id),
        )
            .await
    }

    async fn get_plugins(
        &self,
        environment_id: &Uuid,
        revision_id: i64,
    ) -> RepoResult<Vec<EnvironmentPluginInstallationRevisionRecord>> {
        self.with_ro("get_plugins").fetch_all_as(
            sqlx::query_as(indoc! { r#"
                SELECT
                    epir.environment_id, epir.revision_id, epir.priority, epir.created_at, epir.created_by,
                    epir.plugin_id, p.name as plugin_name, p.version as plugin_version,
                    epir.parameters
                FROM environment_plugin_installation_revisions epir
                JOIN plugins p ON p.plugin_id = epir.plugin_id
                WHERE environment_id = $1 AND revision_id = $2
                ORDER BY priority
            "#})
                .bind(environment_id)
                .bind(revision_id),
        )
            .await
    }

    async fn insert_plugin_installation_revision(
        tx: &mut Self::Tx,
        revision: EnvironmentPluginInstallationRevisionRecord,
    ) -> RepoResult<()> {
        tx.execute(
            sqlx::query(indoc! { r#"
                INSERT INTO environment_plugin_installation_revisions
                (environment_id, revision_id, priority, created_at, created_by, plugin_id, parameters)
                VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#})
                .bind(revision.environment_id)
                .bind(revision.revision_id)
                .bind(revision.priority)
                .bind_revision_audit(revision.audit)
                .bind(revision.plugin_id)
                .bind(revision.parameters),
        )
            .await?;

        Ok(())
    }
}

#[async_trait]
pub(super) trait EnvironmentSharedRepo<DBP: Pool>: Send + Sync {
    type Db: Database;
    type Tx: LabelledPoolTransaction;

    async fn must_get_by_id(
        &self,
        environment_id: &Uuid,
    ) -> RepoResult<EnvironmentExtRevisionRecord>;
}

pub(super) struct EnvironmentSharedRepoDefault<DBP: Pool> {
    db_pool: DBP,
}

impl<DBP: Pool> EnvironmentSharedRepoDefault<DBP> {
    pub fn new(db_pool: DBP) -> Self {
        Self { db_pool }
    }

    fn with_ro(&self, api_name: &'static str) -> DBP::LabelledApi {
        self.db_pool.with_ro(METRICS_SVC_NAME, api_name)
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl EnvironmentSharedRepo<PostgresPool> for EnvironmentSharedRepoDefault<PostgresPool> {
    type Db = <PostgresPool as Pool>::Db;
    type Tx = <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction;

    async fn must_get_by_id(
        &self,
        environment_id: &Uuid,
    ) -> RepoResult<EnvironmentExtRevisionRecord> {
        self.with_ro("must_get_by_id")
            .fetch_one_as(
                sqlx::query_as(indoc! { r"
                    SELECT
                        e.name,e.application_id,
                        r.environment_id, r.revision_id, r.hash,
                        r.created_at, r.created_by, r.deleted,
                        r.compatibility_check, r.version_check, r.security_overrides
                    FROM environments e
                    JOIN environment_revisions r
                        ON e.environment_id = r.environment_id AND e.current_revision_id = r.revision_id
                    WHERE e.environment_id = $1 AND e.deleted_at IS NULL
                 "})
                    .bind(environment_id),
            )
            .await
    }
}
