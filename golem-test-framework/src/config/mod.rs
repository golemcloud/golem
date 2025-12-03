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

use self::dsl_impl::TestDependenciesTestDsl;
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
use golem_common::model::account::AccountCreation;
use golem_common::model::auth::AccountRole;
use golem_service_base::service::initial_component_files::InitialComponentFilesService;
use golem_service_base::service::plugin_wasm_files::PluginWasmFilesService;
use golem_service_base::storage::blob::BlobStorage;
use std::path::Path;
use std::sync::Arc;
use uuid::Uuid;

pub mod benchmark;
pub mod dsl_impl;
mod env;

#[async_trait]
pub trait TestDependencies: Send + Sync {
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
    fn plugin_wasm_files_service(&self) -> Arc<PluginWasmFilesService>;

    fn registry_service(&self) -> Arc<dyn RegistryService>;

    async fn admin(&self) -> TestDependenciesTestDsl<&Self> {
        self.into_admin().await
    }

    async fn into_admin(self) -> TestDependenciesTestDsl<Self>
    where
        Self: Sized,
    {
        let registry_service = self.registry_service();
        TestDependenciesTestDsl {
            account_id: registry_service.admin_account_id(),
            account_email: registry_service.admin_account_email(),
            token: registry_service.admin_account_token(),
            deps: self,
            auto_deploy_enabled: true,
        }
    }

    async fn user(&self) -> anyhow::Result<TestDependenciesTestDsl<&Self>> {
        self.into_user().await
    }

    async fn into_user(self) -> anyhow::Result<TestDependenciesTestDsl<Self>>
    where
        Self: Sized,
    {
        let registry_service = self.registry_service();

        let client = registry_service
            .client(&registry_service.admin_account_token())
            .await;

        let name = Uuid::new_v4().to_string();
        let account_data = AccountCreation {
            email: format!("{name}@golem.cloud"),
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

        Ok(TestDependenciesTestDsl {
            account_id: account.id,
            account_email: account.email,
            token: token.secret,
            deps: self,
            auto_deploy_enabled: true,
        })
    }

    async fn user_with_roles(
        &self,
        roles: &[AccountRole],
    ) -> anyhow::Result<TestDependenciesTestDsl<&Self>> {
        self.into_user_with_roles(roles).await
    }

    async fn into_user_with_roles(
        self,
        roles: &[AccountRole],
    ) -> anyhow::Result<TestDependenciesTestDsl<Self>>
    where
        Self: Sized,
    {
        let registry_service = self.registry_service();

        let client = registry_service
            .client(&registry_service.admin_account_token())
            .await;

        let name = Uuid::new_v4().to_string();
        let account_data = AccountCreation {
            email: format!("{name}@golem.cloud"),
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

        Ok(TestDependenciesTestDsl {
            account_id: account.id,
            account_email: account.email,
            token: token.secret,
            deps: self,
            auto_deploy_enabled: true,
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

impl<T: TestDependencies + ?Sized> TestDependencies for &T {
    fn rdb(&self) -> Arc<dyn Rdb> {
        <T as TestDependencies>::rdb(self)
    }
    fn redis(&self) -> Arc<dyn Redis> {
        <T as TestDependencies>::redis(self)
    }
    fn blob_storage(&self) -> Arc<dyn BlobStorage> {
        <T as TestDependencies>::blob_storage(self)
    }
    fn redis_monitor(&self) -> Arc<dyn RedisMonitor> {
        <T as TestDependencies>::redis_monitor(self)
    }
    fn shard_manager(&self) -> Arc<dyn ShardManager> {
        <T as TestDependencies>::shard_manager(self)
    }
    fn component_directory(&self) -> &Path {
        <T as TestDependencies>::component_directory(self)
    }
    fn temp_directory(&self) -> &Path {
        <T as TestDependencies>::temp_directory(self)
    }
    fn component_compilation_service(&self) -> Arc<dyn ComponentCompilationService> {
        <T as TestDependencies>::component_compilation_service(self)
    }
    fn worker_service(&self) -> Arc<dyn WorkerService> {
        <T as TestDependencies>::worker_service(self)
    }
    fn worker_executor_cluster(&self) -> Arc<dyn WorkerExecutorCluster> {
        <T as TestDependencies>::worker_executor_cluster(self)
    }
    fn initial_component_files_service(&self) -> Arc<InitialComponentFilesService> {
        <T as TestDependencies>::initial_component_files_service(self)
    }
    fn plugin_wasm_files_service(&self) -> Arc<PluginWasmFilesService> {
        <T as TestDependencies>::plugin_wasm_files_service(self)
    }
    fn registry_service(&self) -> Arc<dyn RegistryService> {
        <T as TestDependencies>::registry_service(self)
    }
}

#[derive(Debug, Clone)]
pub enum DbType {
    Postgres,
    Sqlite,
}
