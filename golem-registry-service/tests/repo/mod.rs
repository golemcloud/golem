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

use crate::Tracing;
use golem_registry_service::repo::account::AccountRepo;
use golem_registry_service::repo::account_usage::AccountUsageRepo;
use golem_registry_service::repo::application::ApplicationRepo;
use golem_registry_service::repo::component::ComponentRepo;
use golem_registry_service::repo::deployment::DeploymentRepo;
use golem_registry_service::repo::environment::EnvironmentRepo;
use golem_registry_service::repo::environment_share::EnvironmentShareRepo;
use golem_registry_service::repo::http_api_deployment::HttpApiDeploymentRepo;
use golem_registry_service::repo::mcp_deployment::McpDeploymentRepo;
use golem_registry_service::repo::model::account::{
    AccountExtRevisionRecord, AccountRevisionRecord,
};
use golem_registry_service::repo::model::application::{
    ApplicationExtRevisionRecord, ApplicationRevisionRecord,
};
use golem_registry_service::repo::model::audit::DeletableRevisionAuditFields;
use golem_registry_service::repo::model::environment::{
    EnvironmentExtRevisionRecord, EnvironmentRevisionRecord,
};
use golem_registry_service::repo::model::new_repo_uuid;
use golem_registry_service::repo::model::plan::PlanRecord;
use golem_registry_service::repo::plan::PlanRepo;
use golem_registry_service::repo::plugin::PluginRepo;
use golem_registry_service::repo::registry_change::{
    ChangeEventId, DbRegistryChangeRepo, NewRegistryChangeEvent, RegistryChangeRepo,
};
use golem_registry_service::services::registry_change_notifier::RegistryChangeNotifier;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::Pool;
use futures::FutureExt;
use std::str::FromStr;
use test_r::{inherit_test_dep, sequential_suite};
use uuid::Uuid;

pub mod common;
pub mod postgres;
pub mod sqlite;

inherit_test_dep!(Tracing);

sequential_suite!(postgres);
sequential_suite!(sqlite);

pub struct Deps {
    pub account_repo: Box<dyn AccountRepo>,
    pub account_usage_repo: Box<dyn AccountUsageRepo>,
    pub application_repo: Box<dyn ApplicationRepo>,
    pub environment_repo: Box<dyn EnvironmentRepo>,
    pub plan_repo: Box<dyn PlanRepo>,
    pub component_repo: Box<dyn ComponentRepo>,
    pub http_api_deployment_repo: Box<dyn HttpApiDeploymentRepo>,
    pub mcp_deployment_repo: Box<dyn McpDeploymentRepo>,
    pub deployment_repo: Box<dyn HttpApiDeploymentRepo>,
    pub full_deployment_repo: Box<dyn DeploymentRepo>,
    pub environment_share_repo: Box<dyn EnvironmentShareRepo>,
    pub plugin_repo: Box<dyn PluginRepo>,
    pub registry_change_repo: Box<dyn RegistryChangeRepo>,
    pub test_db: TestDb,
}

pub enum TestDb {
    Postgres(PostgresPool),
    Sqlite(SqlitePool),
}

struct NoopRegistryChangeNotifier {
    sender: tokio::sync::broadcast::Sender<
        golem_registry_service::repo::registry_change::RegistryChangeEvent,
    >,
}

impl NoopRegistryChangeNotifier {
    fn new() -> Self {
        let (sender, _) = tokio::sync::broadcast::channel(1);
        Self { sender }
    }
}

impl RegistryChangeNotifier for NoopRegistryChangeNotifier {
    fn signal_new_events_available(&self) {}

    fn subscribe(
        &self,
    ) -> tokio::sync::broadcast::Receiver<
        golem_registry_service::repo::registry_change::RegistryChangeEvent,
    > {
        self.sender.subscribe()
    }
}

impl Deps {
    pub fn registry_change_repo_for_notifier(&self) -> std::sync::Arc<dyn RegistryChangeRepo> {
        match &self.test_db {
            TestDb::Postgres(pool) => std::sync::Arc::new(DbRegistryChangeRepo::new(pool.clone())),
            TestDb::Sqlite(pool) => std::sync::Arc::new(DbRegistryChangeRepo::new(pool.clone())),
        }
    }

    pub fn test_registry_change_notifier(&self) -> std::sync::Arc<dyn RegistryChangeNotifier> {
        std::sync::Arc::new(NoopRegistryChangeNotifier::new())
    }

    pub async fn record_registry_change_event(
        &self,
        event: NewRegistryChangeEvent,
    ) -> ChangeEventId {
        match &self.test_db {
            TestDb::Postgres(pool) => pool
                .with_tx_err("registry_change", "record_change_event_test", |tx| {
                    async move {
                        DbRegistryChangeRepo::<PostgresPool>::create_change_event_in_tx(tx, &event)
                            .await
                    }
                    .boxed()
                })
                .await
                .expect("failed to insert registry change event"),
            TestDb::Sqlite(pool) => pool
                .with_tx_err("registry_change", "record_change_event_test", |tx| {
                    async move {
                        DbRegistryChangeRepo::<SqlitePool>::create_change_event_in_tx(tx, &event)
                            .await
                    }
                    .boxed()
                })
                .await
                .expect("failed to insert registry change event"),
        }
    }

    pub async fn setup(&self) {
        self.plan_repo
            .create_or_update(PlanRecord {
                plan_id: self.test_plan_id(),
                name: "MAIN_TEST_PLAN".to_string(),
                total_app_count: 3.into(),
                total_env_count: 10.into(),
                total_component_count: 15.into(),
                total_worker_count: 20.into(),
                total_worker_connection_count: 25.into(),
                total_component_storage_bytes: 1000.into(),
                monthly_gas_limit: 2000.into(),
                monthly_component_upload_limit_bytes: 3000.into(),
                max_memory_per_worker: 4000.into(),
                max_table_elements_per_worker: 16384.into(),
                max_disk_space_per_worker: 1073741824.into(),
            })
            .await
            .unwrap();
    }

    pub fn test_plan_id(&self) -> Uuid {
        Uuid::from_str("e449dca1-cf07-4270-a8a2-6bcfc6528038").unwrap()
    }

    pub async fn create_account(&self) -> AccountExtRevisionRecord {
        let account_id = new_repo_uuid();
        self.create_account_with_email(&format!("test-{account_id}@golem"))
            .await
    }

    pub async fn create_account_with_email(&self, email: &str) -> AccountExtRevisionRecord {
        let account_id = new_repo_uuid();
        self.account_repo
            .create(AccountRevisionRecord {
                account_id,
                revision_id: 0,
                email: email.to_string(),
                audit: DeletableRevisionAuditFields::new(account_id),
                name: format!("Test Account {account_id}"),
                plan_id: self.test_plan_id(),
                roles: 0,
            })
            .await
            .unwrap()
    }

    pub async fn create_application(&self, owner_account_id: Uuid) -> ApplicationExtRevisionRecord {
        let user = self.create_account().await;

        self.application_repo
            .create(
                owner_account_id,
                ApplicationRevisionRecord {
                    application_id: new_repo_uuid(),
                    revision_id: 0,
                    name: format!("app-name-{}", new_repo_uuid()),
                    audit: DeletableRevisionAuditFields::new(user.revision.account_id),
                },
            )
            .await
            .unwrap()
    }

    pub async fn create_env(&self, parent_application_id: Uuid) -> EnvironmentExtRevisionRecord {
        let user = self.create_account().await;
        self.environment_repo
            .create(
                parent_application_id,
                EnvironmentRevisionRecord {
                    environment_id: new_repo_uuid(),
                    revision_id: 0,
                    name: format!("env-{}", new_repo_uuid()),
                    audit: DeletableRevisionAuditFields::new(user.revision.account_id),
                    compatibility_check: true,
                    version_check: true,
                    security_overrides: true,
                    hash: blake3::hash("test".as_bytes()).into(),
                },
            )
            .await
            .unwrap()
    }
}
