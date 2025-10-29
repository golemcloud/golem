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

use crate::components::redis::Redis;
use crate::config::TestDependencies;
use crate::dsl::{build_ifs_archive, rename_component_if_needed, TestDsl, TestDslExtended};
use crate::model::IFSEntry;
use applying::Apply;
use async_trait::async_trait;
use golem_api_grpc::proto::golem::worker::LogEvent;
use golem_client::api::{RegistryServiceClient, RegistryServiceClientLive};
use golem_common::model::account::AccountId;
use golem_common::model::auth::TokenSecret;
use golem_common::model::component::{ComponentCreation, ComponentUpdate};
use golem_common::model::component::{
    ComponentDto, ComponentFileOptions, ComponentFilePath, ComponentId, ComponentName,
    ComponentRevision, ComponentType, PluginInstallation,
};
use golem_common::model::component_metadata::DynamicLinkedInstance;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::worker::WorkerMetadataDto;
use golem_common::model::IdempotencyKey;
use golem_common::model::{OplogIndex, WorkerId};
use golem_common::model::{PromiseId, ScanCursor, WorkerFilter};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::{PublicOplogEntryWithIndex, RevertWorkerTarget};
use golem_wasm::{Value, ValueAndType};
use std::borrow::Borrow;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs::File;
use tokio::sync::mpsc::UnboundedReceiver;
use uuid::Uuid;

#[derive(Clone)]
pub struct TestDependenciesTestDsl<Deps> {
    pub deps: Deps,
    pub account_id: AccountId,
    pub account_email: String,
    pub token: TokenSecret,
}

#[async_trait]
impl<Deps: TestDependencies> TestDsl for TestDependenciesTestDsl<Deps> {
    fn redis(&self) -> Arc<dyn Redis> {
        self.deps.redis()
    }

    async fn store_component_with(
        &self,
        wasm_name: &str,
        environment_id: EnvironmentId,
        name: &str,
        component_type: ComponentType,
        unique: bool,
        unverified: bool,
        files: Vec<IFSEntry>,
        dynamic_linking: HashMap<String, DynamicLinkedInstance>,
        env: BTreeMap<String, String>,
        plugins: Vec<PluginInstallation>,
    ) -> anyhow::Result<ComponentDto> {
        let component_directy = self.deps.component_directory();

        let source_path = component_directy.join(format!("{wasm_name}.wasm"));

        let component_name = if unique {
            let uuid = Uuid::new_v4();
            ComponentName(format!("{name}-{uuid}"))
        } else {
            match component_type {
                ComponentType::Durable => ComponentName(name.to_string()),
                ComponentType::Ephemeral => ComponentName(format!("{name}-ephemeral")),
            }
        };
        let dynamic_linking = HashMap::from_iter(
            dynamic_linking
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone())),
        );

        let source_path = if !unverified {
            rename_component_if_needed(
                self.deps.borrow().temp_directory(),
                &source_path,
                &component_name.0,
            )
            .expect("Failed to verify and change component metadata")
        } else {
            source_path
        };

        let (_tmp_dir, maybe_files_archive) = if !files.is_empty() {
            let (tmp_dir, files_archive) = build_ifs_archive(component_directy, &files).await?;
            (Some(tmp_dir), Some(File::open(files_archive).await?))
        } else {
            (None, None)
        };

        let file_options = files
            .into_iter()
            .map(|f| {
                (
                    f.target_path,
                    ComponentFileOptions {
                        permissions: f.permissions,
                    },
                )
            })
            .apply(BTreeMap::from_iter);

        let client = self.deps.registry_service().client(&self.token).await;

        let component = client
            .create_component(
                &environment_id.0,
                &ComponentCreation {
                    component_name,
                    component_type: Some(component_type),
                    file_options,
                    dynamic_linking,
                    env,
                    plugins,
                    agent_types: vec![],
                },
                File::open(source_path).await?,
                maybe_files_archive,
            )
            .await?;

        Ok(component)
    }

    async fn get_latest_component_version(
        &self,
        _component_id: &ComponentId,
    ) -> anyhow::Result<ComponentDto> {
        unimplemented!()
    }

    async fn update_component_with(
        &self,
        component_id: &ComponentId,
        previous_version: ComponentRevision,
        wasm_name: Option<&str>,
        component_type: Option<ComponentType>,
        new_files: Vec<IFSEntry>,
        removed_files: Vec<ComponentFilePath>,
        dynamic_linking: Option<HashMap<String, DynamicLinkedInstance>>,
        env: Option<BTreeMap<String, String>>,
    ) -> anyhow::Result<ComponentDto> {
        let component_directy = self.deps.component_directory();

        let updated_wasm = if let Some(wasm_name) = wasm_name {
            let source_path: PathBuf = component_directy.join(format!("{wasm_name}.wasm"));
            Some(File::open(source_path).await?)
        } else {
            None
        };

        let (_tmp_dir, maybe_new_files_archive) = if !new_files.is_empty() {
            let (tmp_dir, new_files_archive) =
                build_ifs_archive(component_directy, &new_files).await?;
            (Some(tmp_dir), Some(File::open(new_files_archive).await?))
        } else {
            (None, None)
        };

        let new_file_options = new_files
            .into_iter()
            .map(|f| {
                (
                    f.target_path,
                    ComponentFileOptions {
                        permissions: f.permissions,
                    },
                )
            })
            .apply(BTreeMap::from_iter);

        let client = self.deps.registry_service().client(&self.token).await;

        let component = client
            .update_component(
                &component_id.0,
                &ComponentUpdate {
                    current_revision: previous_version,
                    component_type,
                    new_file_options,
                    removed_files,
                    dynamic_linking,
                    env,
                    agent_types: None,
                    plugin_updates: Vec::new(),
                },
                updated_wasm,
                maybe_new_files_archive,
            )
            .await?;

        Ok(component)
    }

    async fn try_start_worker_with(
        &self,
        _component_id: &ComponentId,
        _name: &str,
        _args: Vec<String>,
        _env: HashMap<String, String>,
        _wasi_config_vars: Vec<(String, String)>,
    ) -> anyhow::Result<Result<WorkerId, WorkerExecutorError>> {
        unimplemented!()
    }

    async fn invoke_with_key(
        &self,
        _worker_id: &WorkerId,
        _idempotency_key: &IdempotencyKey,
        _function_name: &str,
        _params: Vec<ValueAndType>,
    ) -> anyhow::Result<Result<(), WorkerExecutorError>> {
        unimplemented!()
    }

    async fn invoke_and_await_with_key(
        &self,
        _worker_id: &WorkerId,
        _idempotency_key: &IdempotencyKey,
        _function_name: &str,
        _params: Vec<ValueAndType>,
    ) -> crate::Result<Result<Vec<Value>, WorkerExecutorError>> {
        unimplemented!()
    }

    async fn invoke_and_await_typed_with_key(
        &self,
        _worker_id: &WorkerId,
        _idempotency_key: &IdempotencyKey,
        _function_name: &str,
        _params: Vec<ValueAndType>,
    ) -> anyhow::Result<Result<Option<ValueAndType>, WorkerExecutorError>> {
        unimplemented!()
    }

    async fn revert(&self, _worker_id: &WorkerId, _target: RevertWorkerTarget) -> anyhow::Result<()> {
        unimplemented!()
    }

    async fn get_oplog(
        &self,
        _worker_id: &WorkerId,
        _from: OplogIndex,
    ) -> anyhow::Result<Vec<PublicOplogEntryWithIndex>> {
        unimplemented!()
    }

    async fn search_oplog(
        &self,
        _worker_id: &WorkerId,
        _query: &str,
    ) -> anyhow::Result<Vec<PublicOplogEntryWithIndex>> {
        unimplemented!()
    }

    async fn interrupt_with_optional_recovery(
        &self,
        _worker_id: &WorkerId,
        _recover_immediately: bool,
    ) -> anyhow::Result<()> {
        unimplemented!()
    }

    async fn resume(&self, _worker_id: &WorkerId, _force: bool) -> anyhow::Result<()> {
        unimplemented!()
    }

    async fn complete_promise(
        &self,
        _promise_id: &PromiseId,
        _data: Vec<u8>,
    ) -> anyhow::Result<()> {
        unimplemented!()
    }

    async fn capture_output(
        &self,
        _worker_id: &WorkerId,
    ) -> anyhow::Result<UnboundedReceiver<LogEvent>> {
        unimplemented!()
    }

    async fn capture_output_with_termination(
        &self,
        _worker_id: &WorkerId,
    ) -> anyhow::Result<UnboundedReceiver<Option<LogEvent>>> {
        unimplemented!()
    }

    async fn auto_update_worker(
        &self,
        _worker_id: &WorkerId,
        _target_version: ComponentRevision,
    ) -> anyhow::Result<()> {
        unimplemented!()
    }

    async fn manual_update_worker(
        &self,
        _worker_id: &WorkerId,
        _target_version: ComponentRevision,
    ) -> anyhow::Result<()> {
        unimplemented!()
    }

    async fn delete_worker(&self, _worker_id: &WorkerId) -> anyhow::Result<()> {
        unimplemented!()
    }

    async fn get_worker_metadata(
        &self,
        _worker_id: &WorkerId,
    ) -> anyhow::Result<WorkerMetadataDto> {
        unimplemented!()
    }

    async fn get_workers_metadata(
        &self,
        _component_id: &ComponentId,
        _filter: Option<WorkerFilter>,
        _cursor: ScanCursor,
        _count: u64,
        _precise: bool,
    ) -> anyhow::Result<(Option<ScanCursor>, Vec<WorkerMetadataDto>)> {
        unimplemented!()
    }

    async fn get_running_workers_metadata(
        &self,
        _component_id: &ComponentId,
        _filter: Option<WorkerFilter>,
    ) -> anyhow::Result<Vec<WorkerMetadataDto>> {
        unimplemented!()
    }

    async fn cancel_invocation(
        &self,
        _worker_id: &WorkerId,
        _idempotency_key: &IdempotencyKey,
    ) -> anyhow::Result<bool> {
        unimplemented!()
    }
}

#[async_trait]
impl<Deps: TestDependencies> TestDslExtended for TestDependenciesTestDsl<Deps> {
    fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    async fn registry_service_client(&self) -> RegistryServiceClientLive {
        self.deps.registry_service().client(&self.token).await
    }
}
