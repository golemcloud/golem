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

use self::dsl_impl::{NameResolutionCache, TestUserContext};
use crate::components::component_compilation_service::ComponentCompilationService;
use crate::components::rdb::Rdb;
use crate::components::redis::Redis;
use crate::components::redis_monitor::RedisMonitor;
use crate::components::registry_service::RegistryService;
use crate::components::shard_manager::ShardManager;
use crate::components::worker_executor_cluster::WorkerExecutorCluster;
use crate::components::worker_service::WorkerService;
use async_trait::async_trait;
pub use benchmark::{BenchmarkCliParameters, BenchmarkTestDependencies, CliTestService};
use chrono::{DateTime, Utc};
pub use env::EnvBasedTestDependencies;
pub use env::EnvBasedTestDependenciesConfig;
use golem_client::api::RegistryServiceClient;
use golem_client::model::{AccountSetRoles, TokenCreation};
use golem_common::model::account::{AccountCreation, AccountEmail};
use golem_common::model::auth::AccountRole;
use golem_service_base::service::initial_component_files::InitialComponentFilesService;
use golem_service_base::storage::blob::BlobStorage;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use uuid::Uuid;

pub mod benchmark;
pub mod dsl_impl;
mod env;

#[async_trait]
pub trait TestDependencies: Send + Sync + Clone {
    fn rdb(&self) -> Arc<dyn Rdb>;
    fn redis(&self) -> Arc<dyn Redis>;
    fn blob_storage(&self) -> Arc<dyn BlobStorage>;
    fn redis_monitor(&self) -> Arc<dyn RedisMonitor>;
    fn shard_manager(&self) -> Arc<dyn ShardManager>;
    fn component_directory(&self) -> &Path;
    fn temp_directory(&self) -> &Path;
    fn component_compilation_service(&self) -> Arc<dyn ComponentCompilationService>;
    fn worker_service(&self) -> Arc<dyn WorkerService>;
    fn worker_executor_cluster(&self) -> Arc<dyn WorkerExecutorCluster>;
    fn initial_component_files_service(&self) -> Arc<InitialComponentFilesService>;
    fn registry_service(&self) -> Arc<dyn RegistryService>;

    async fn admin(&self) -> TestUserContext<Self>
    where
        Self: Sized,
    {
        let registry_service = self.registry_service();
        TestUserContext {
            account_id: registry_service.admin_account_id(),
            account_email: registry_service.admin_account_email(),
            token: registry_service.admin_account_token(),
            deps: self.clone(),
            auto_deploy_enabled: true,
            name_cache: Arc::new(NameResolutionCache::new()),
            last_deployments: Arc::new(std::sync::RwLock::new(HashMap::new())),
        }
    }

    async fn user(&self) -> anyhow::Result<TestUserContext<Self>>
    where
        Self: Sized,
    {
        let registry_service = self.registry_service();

        let client = registry_service
            .client(&registry_service.admin_account_token())
            .await;

        let name = Uuid::new_v4().to_string();
        let account_data = AccountCreation {
            email: AccountEmail(format!("{name}@golem.cloud")),
            name,
        };

        let account = client.create_account(&account_data).await?;

        let token = client
            .create_token(
                &account.id.0,
                &TokenCreation {
                    expires_at: DateTime::<Utc>::MAX_UTC,
                },
            )
            .await?;

        Ok(TestUserContext {
            account_id: account.id,
            account_email: account.email,
            token: token.secret,
            deps: self.clone(),
            auto_deploy_enabled: true,
            name_cache: Arc::new(NameResolutionCache::new()),
            last_deployments: Arc::new(std::sync::RwLock::new(HashMap::new())),
        })
    }

    async fn user_with_roles(&self, roles: &[AccountRole]) -> anyhow::Result<TestUserContext<Self>>
    where
        Self: Sized,
    {
        let registry_service = self.registry_service();

        let client = registry_service
            .client(&registry_service.admin_account_token())
            .await;

        let name = Uuid::new_v4().to_string();
        let account_data = AccountCreation {
            email: AccountEmail(format!("{name}@golem.cloud")),
            name,
        };

        let account = client.create_account(&account_data).await?;

        client
            .set_account_roles(
                &account.id.0,
                &AccountSetRoles {
                    current_revision: account.revision,
                    roles: roles.to_vec(),
                },
            )
            .await?;

        let token = client
            .create_token(
                &account.id.0,
                &TokenCreation {
                    expires_at: DateTime::<Utc>::MAX_UTC,
                },
            )
            .await?;

        Ok(TestUserContext {
            account_id: account.id,
            account_email: account.email,
            token: token.secret,
            deps: self.clone(),
            auto_deploy_enabled: true,
            name_cache: Arc::new(NameResolutionCache::new()),
            last_deployments: Arc::new(std::sync::RwLock::new(HashMap::new())),
        })
    }

    async fn kill_all(&self) {
        self.worker_executor_cluster().kill_all().await;
        self.worker_service().kill().await;
        self.component_compilation_service().kill().await;
        self.registry_service().kill().await;
        self.shard_manager().kill().await;
        self.rdb().kill().await;
        self.redis_monitor().kill();
        self.redis().kill().await;
    }
}

#[derive(Debug, Clone)]
pub enum DbType {
    Postgres,
    Sqlite,
}
