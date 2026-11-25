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

//! The audit records of the worker service's decisions, as they wait in the registry's
//! staging table.

use golem_client::api::RegistryServiceClient;
use golem_common::agent_key;
use golem_common::model::OwnedAgentId;
use golem_common::model::authority::CatalogOperation;
use golem_common::model::environment::EnvironmentId;
use golem_service_base::model::auth::audit::{AuditOutcome, AuditRecord};
use golem_test_framework::components::rdb::DbInfo;
use golem_test_framework::config::dsl_impl::TestUserContext;
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use pretty_assertions::assert_eq;
use sqlx::postgres::PgPoolOptions;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(EnvBasedTestDependencies);

/// The records that wait in the registry's staging table for `operation` on a target in
/// `environment_id`.
async fn staged_records(
    deps: &EnvBasedTestDependencies,
    environment_id: EnvironmentId,
    operation: CatalogOperation,
) -> anyhow::Result<Vec<AuditRecord>> {
    let rows: Vec<String> = match deps.rdb().info() {
        DbInfo::Sqlite(root) => {
            let pool = SqlitePoolOptions::new()
                .max_connections(1)
                .connect_with(
                    SqliteConnectOptions::new()
                        .filename(root.join("golem_registry.db"))
                        .journal_mode(SqliteJournalMode::Wal),
                )
                .await?;
            sqlx::query_scalar(
                "SELECT record FROM audit_staging \
                 WHERE target_environment_id = $1 AND operation = $2",
            )
            .bind(environment_id.0)
            .bind(operation.name())
            .fetch_all(&pool)
            .await?
        }
        DbInfo::Postgres(postgres) => {
            let pool = PgPoolOptions::new()
                .max_connections(1)
                .connect_with(postgres.to_connect_options())
                .await?;
            sqlx::query_scalar(
                "SELECT record FROM golem_registry.audit_staging \
                 WHERE target_environment_id = $1 AND operation = $2",
            )
            .bind(environment_id.0)
            .bind(operation.name())
            .fetch_all(&pool)
            .await?
        }
        DbInfo::Mysql(_) => anyhow::bail!("registry service does not support MySQL"),
    };
    rows.iter()
        .map(|row| Ok(serde_json::from_str(row)?))
        .collect()
}

/// An agent of a new account in its own environment.
async fn owned_agent(
    deps: &EnvBasedTestDependencies,
    name: &str,
) -> anyhow::Result<(TestUserContext<EnvBasedTestDependencies>, OwnedAgentId)> {
    let owner = deps.user().await?;
    let (_, env) = owner.app_and_env().await?;
    let component = owner
        .component(&env.id, "golem_it_agent_rpc_rust_release")
        .store()
        .await?;
    let agent_id = owner
        .start_agent(&component.id, agent_key!("RpcBlockingCounter", name))
        .await?;
    Ok((owner, agent_id))
}

#[test]
#[timeout("6m")]
async fn an_allowed_oplog_read_waits_in_the_staging_table_with_its_token_and_grantors(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let (owner, agent_id) = owned_agent(deps, "audit-allowed").await?;
    let tokens = deps
        .registry_service()
        .client(&owner.token)
        .await
        .list_account_tokens(&owner.account_id.0)
        .await?;
    assert_eq!(tokens.values.len(), 1);

    owner.get_oplog_last_index(&agent_id).await?;

    let records = staged_records(
        deps,
        agent_id.environment_id,
        CatalogOperation::ReadAgentOplog,
    )
    .await?;
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record.outcome, AuditOutcome::Allowed);
    assert_eq!(record.surface, "rest");
    assert_eq!(record.authenticated_by, Some(tokens.values[0].id));
    assert_eq!(
        record.parts[0].target.id,
        agent_id.agent_id.as_uuid().to_string()
    );
    assert_eq!(
        record.target_environment_id(),
        Some(agent_id.environment_id)
    );
    assert!(!record.grantors.is_empty(), "{record:?}");
    Ok(())
}

#[test]
#[timeout("6m")]
async fn a_refused_oplog_read_waits_in_the_staging_table_with_the_real_target(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let (_owner, agent_id) = owned_agent(deps, "audit-refused").await?;
    let stranger = deps.user().await?;

    let refused = stranger.get_oplog_last_index(&agent_id).await;
    assert!(refused.is_err(), "{refused:?}");

    let records = staged_records(
        deps,
        agent_id.environment_id,
        CatalogOperation::ReadAgentOplog,
    )
    .await?;
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert!(record.outcome != AuditOutcome::Allowed, "{record:?}");
    assert_eq!(record.surface, "rest");
    assert_eq!(
        record.parts[0].target.id,
        agent_id.agent_id.as_uuid().to_string()
    );
    assert_eq!(
        record.target_environment_id(),
        Some(agent_id.environment_id)
    );
    assert!(record.grantors.is_empty());
    Ok(())
}
