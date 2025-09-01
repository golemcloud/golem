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

use crate::components::cloud_service::CloudService;
use crate::components::component_compilation_service::ComponentCompilationService;
use crate::components::component_service::ComponentService;
use crate::components::rdb::Rdb;
use crate::components::redis::Redis;
use crate::components::redis_monitor::RedisMonitor;
use crate::components::service::Service;
use crate::components::shard_manager::ShardManager;
use crate::components::worker_executor_cluster::WorkerExecutorCluster;
use crate::components::worker_service::WorkerService;
use async_trait::async_trait;
use clap::ValueEnum;
pub use cli::{CliParams, CliTestDependencies, CliTestService};
pub use env::EnvBasedTestDependencies;
pub use env::EnvBasedTestDependenciesConfig;
use golem_client::model::AccountData;
use golem_common::model::{AccountId, ProjectId};
use golem_service_base::service::initial_component_files::InitialComponentFilesService;
use golem_service_base::service::plugin_wasm_files::PluginWasmFilesService;
use golem_service_base::storage::blob::BlobStorage;
use std::path::Path;
use std::sync::Arc;
use uuid::Uuid;

pub mod cli;
mod env;

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
#[clap(rename_all = "kebab-case")]
pub enum GolemClientProtocol {
    Grpc,
    Http,
}

#[async_trait]
pub trait TestDependencies: Send + Sync {
    fn rdb(&self) -> Arc<dyn Rdb>;
    fn redis(&self) -> Arc<dyn Redis>;
    fn blob_storage(&self) -> Arc<dyn BlobStorage>;
    fn redis_monitor(&self) -> Arc<dyn RedisMonitor>;
    fn shard_manager(&self) -> Arc<dyn ShardManager>;
    fn component_directory(&self) -> &Path;
    fn component_temp_directory(&self) -> &Path;
    fn component_service(&self) -> Arc<dyn ComponentService>;
    fn component_compilation_service(&self) -> Arc<dyn ComponentCompilationService>;
    fn worker_service(&self) -> Arc<dyn WorkerService>;
    fn worker_executor_cluster(&self) -> Arc<dyn WorkerExecutorCluster>;
    fn initial_component_files_service(&self) -> Arc<InitialComponentFilesService>;
    fn plugin_wasm_files_service(&self) -> Arc<PluginWasmFilesService>;
    fn cloud_service(&self) -> Arc<dyn CloudService>;

    // TODO: this need to be cached, especially when using in benchmarks
    async fn admin(&self) -> TestDependenciesDsl<&Self> {
        TestDependenciesDsl {
            deps: self,
            account_id: self.cloud_service().admin_account_id(),
            account_email: self.cloud_service().admin_email(),
            default_project_id: self
                .cloud_service()
                .get_default_project(&self.cloud_service().admin_token())
                .await
                .expect("failed to get default project for admin"),
            token: self.cloud_service().admin_token(),
        }
    }

    async fn into_admin(self) -> TestDependenciesDsl<Self>
    where
        Self: Sized,
    {
        let account_id = self.cloud_service().admin_account_id();
        let token = self.cloud_service().admin_token();
        let account_email = self.cloud_service().admin_email();
        let default_project_id = self
            .cloud_service()
            .get_default_project(&token)
            .await
            .expect("failed to get default project for admin");

        TestDependenciesDsl {
            deps: self,
            account_id,
            account_email,
            default_project_id,
            token,
        }
    }

    async fn user(&self) -> TestDependenciesDsl<&Self> {
        let name = Uuid::new_v4().to_string();
        let account_data = AccountData {
            email: format!("{name}@golem.cloud"),
            name,
        };

        let account = self
            .cloud_service()
            .create_account(&self.cloud_service().admin_token(), &account_data)
            .await
            .expect("failed to create user");
        let default_project_id = self
            .cloud_service()
            .get_default_project(&account.token)
            .await
            .expect("failed to get default project for user");

        TestDependenciesDsl {
            deps: self,
            account_id: account.id,
            account_email: account.email,
            token: account.token,
            default_project_id,
        }
    }

    async fn into_user(self) -> TestDependenciesDsl<Self>
    where
        Self: Sized,
    {
        let name = Uuid::new_v4().to_string();
        let account_data = AccountData {
            email: format!("{name}@golem.cloud"),
            name,
        };

        let account = self
            .cloud_service()
            .create_account(&self.cloud_service().admin_token(), &account_data)
            .await
            .expect("failed to create user");
        let default_project_id = self
            .cloud_service()
            .get_default_project(&account.token)
            .await
            .expect("failed to get default project for user");

        TestDependenciesDsl {
            deps: self,
            account_id: account.id,
            account_email: account.email,
            token: account.token,
            default_project_id,
        }
    }

    async fn kill_all(&self) {
        self.worker_executor_cluster().kill_all().await;
        self.worker_service().kill().await;
        self.component_compilation_service().kill().await;
        self.component_service().kill().await;
        self.shard_manager().kill().await;
        self.rdb().kill().await;
        self.redis_monitor().kill();
        self.redis().kill().await;
    }
}

impl<T: TestDependencies> TestDependencies for &T {
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
    fn component_temp_directory(&self) -> &Path {
        <T as TestDependencies>::component_temp_directory(self)
    }
    fn component_service(&self) -> Arc<dyn ComponentService> {
        <T as TestDependencies>::component_service(self)
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
    fn cloud_service(&self) -> Arc<dyn CloudService> {
        <T as TestDependencies>::cloud_service(self)
    }
}

#[derive(Clone)]
pub struct TestDependenciesDsl<Deps> {
    pub deps: Deps,
    pub account_id: AccountId,
    pub account_email: String,
    pub default_project_id: ProjectId,
    pub token: Uuid,
}

#[derive(Debug, Clone)]
pub enum DbType {
    Postgres,
    Sqlite,
}

pub trait TestService {
    fn service(&self) -> Arc<dyn Service>;

    fn kill_all(&self) {
        self.service().kill();
    }
}
