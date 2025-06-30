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
use golem_common::model::AccountId;
use golem_service_base::service::initial_component_files::InitialComponentFilesService;
use golem_service_base::service::plugin_wasm_files::PluginWasmFilesService;
use golem_service_base::storage::blob::BlobStorage;
use std::borrow::Borrow;
use std::marker::PhantomData;
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

    fn admin(&self) -> TestDependenciesDsl<Self, &Self> {
        TestDependenciesDsl {
            deps: self,
            account_id: self.cloud_service().admin_account_id(),
            token: self.cloud_service().admin_token(),
            _pd: PhantomData,
        }
    }

    fn into_admin(self) -> TestDependenciesDsl<Self, Self>
    where
        Self: Sized,
    {
        let account_id = self.cloud_service().admin_account_id();
        let token = self.cloud_service().admin_token();

        TestDependenciesDsl {
            deps: self,
            account_id,
            token,
            _pd: PhantomData,
        }
    }

    async fn user<'a>(&'a self) -> TestDependenciesDsl<Self, &'a Self> {
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

        TestDependenciesDsl {
            deps: self,
            account_id: account.account_id,
            token: account.token,
            _pd: PhantomData,
        }
    }

    async fn into_user(self) -> TestDependenciesDsl<Self, Self>
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

        TestDependenciesDsl {
            deps: self,
            account_id: account.account_id,
            token: account.token,
            _pd: PhantomData,
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

#[derive(Clone)]
pub struct TestDependenciesDsl<Deps: TestDependencies + ?Sized, Inner: Borrow<Deps>> {
    pub deps: Inner,
    pub account_id: AccountId,
    pub token: Uuid,
    _pd: PhantomData<Deps>,
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
