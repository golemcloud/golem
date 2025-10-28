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

use super::TestWorkerExecutor;
use anyhow::anyhow;
use golem_api_grpc::proto::golem::worker::v1::worker_error::Error as WorkerGrpcError;
use golem_api_grpc::proto::golem::workerexecutor;
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    create_worker_response, get_oplog_response, CreateWorkerRequest, GetWorkerMetadataRequest,
};
use golem_common::model::component::{
    ComponentDto, ComponentFilePath, ComponentId, ComponentName, ComponentRevision, ComponentType,
    InitialComponentFile, PluginInstallation,
};
use golem_common::model::component_metadata::DynamicLinkedInstance;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::public_oplog::PublicOplogEntry;
use golem_common::model::worker::FlatWorkerMetadata;
use golem_common::model::IdempotencyKey;
use golem_common::model::{OplogIndex, WorkerId};
use golem_common::widen_infallible;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::PublicOplogEntryWithIndex;
use golem_service_base::replayable_stream::ReplayableStream;
use golem_test_framework::dsl::{rename_component_if_needed, TestDsl};
use golem_test_framework::model::IFSEntry;
use golem_wasm::{Value, ValueAndType};
use std::collections::{BTreeMap, HashMap};
use uuid::Uuid;

#[async_trait::async_trait]
impl TestDsl for TestWorkerExecutor {
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
        if !plugins.is_empty() {
            return Err(anyhow!(
                "Plugins aren not supported in worker executor tests"
            ));
        }

        let component_directy = &self.deps.component_directory;

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
                self.deps.component_temp_directory.path(),
                &source_path,
                &component_name.0,
            )
            .expect("Failed to verify and change component metadata")
        } else {
            source_path
        };

        let mut converted_files = Vec::new();
        for entry in files {
            let data = tokio::fs::read(entry.source_path).await?;
            let key = self
                .deps
                .initial_component_files_service
                .put_if_not_exists(
                    &environment_id,
                    data.map_error(widen_infallible::<anyhow::Error>)
                        .map_item(|i| i.map_err(widen_infallible::<anyhow::Error>)),
                )
                .await?;
            converted_files.push(InitialComponentFile {
                key,
                path: entry.target_path,
                permissions: entry.permissions,
            });
        }

        let component = {
            if unique {
                self.deps
                    .component_writer
                    .add_component(
                        &source_path,
                        &component_name.0,
                        component_type,
                        converted_files,
                        dynamic_linking,
                        unverified,
                        env,
                        environment_id,
                        self.application_id.clone(),
                        self.account_id.clone(),
                        self.environment_roles_from_shares.clone(),
                    )
                    .await
                    .expect("Failed to add component")
            } else {
                self.deps
                    .component_writer
                    .get_or_add_component(
                        &source_path,
                        &component_name.0,
                        component_type,
                        converted_files,
                        dynamic_linking,
                        unverified,
                        env,
                        environment_id,
                        self.application_id.clone(),
                        self.account_id.clone(),
                        self.environment_roles_from_shares.clone(),
                    )
                    .await
            }
        };

        Ok(component)
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
        let latest_version = self
            .deps
            .component_writer
            .get_latest_component_metadata(component_id)
            .await?;

        if latest_version.revision != previous_version {
            return Err(anyhow!(
                "Unexpected previous version. wanted {previous_version} but found {}",
                latest_version.revision
            ));
        };

        let component_directy = &self.deps.component_directory;

        let source_path =
            wasm_name.map(|wasm_name| component_directy.join(format!("{wasm_name}.wasm")));

        let mut converted_new_files = Vec::new();
        for entry in new_files {
            let data = tokio::fs::read(entry.source_path).await?;
            let key = self
                .deps
                .initial_component_files_service
                .put_if_not_exists(
                    &latest_version.environment_id,
                    data.map_error(widen_infallible::<anyhow::Error>)
                        .map_item(|i| i.map_err(widen_infallible::<anyhow::Error>)),
                )
                .await?;
            converted_new_files.push(InitialComponentFile {
                key,
                path: entry.target_path,
                permissions: entry.permissions,
            });
        }

        let component = self
            .deps
            .component_writer
            .update_component(
                component_id,
                source_path.as_deref(),
                component_type,
                converted_new_files,
                removed_files,
                dynamic_linking,
                env,
            )
            .await?;

        Ok(component)
    }

    async fn try_start_worker_with(
        &self,
        component_id: &ComponentId,
        name: &str,
        args: Vec<String>,
        env: HashMap<String, String>,
        wasi_config_vars: Vec<(String, String)>,
    ) -> anyhow::Result<Result<WorkerId, WorkerGrpcError>> {
        let latest_version = self
            .deps
            .component_writer
            .get_latest_component_metadata(component_id)
            .await?;

        let worker_id = WorkerId {
            component_id: component_id.clone(),
            worker_name: name.to_string(),
        };

        let response = self
            .client
            .lock()
            .await
            .create_worker(CreateWorkerRequest {
                worker_id: Some(worker_id.clone().into()),
                component_version: latest_version.revision.0,
                component_owner_account_id: Some(latest_version.account_id.into()),
                account_limits: None,
                environment_id: Some(latest_version.environment_id.into()),
                args,
                env,
                wasi_config_vars: Some(BTreeMap::from_iter(wasi_config_vars).into()),
                ignore_already_existing: false,
                auth_ctx: Some(AuthCtx::System.into()),
            })
            .await?;

        let response = response.into_inner();

        match response.result {
            None => panic!("No response from create_worker"),
            Some(create_worker_response::Result::Success(_)) => Ok(Ok(worker_id)),
            Some(create_worker_response::Result::Failure(error)) => Ok(Err(
                golem_api_grpc::proto::golem::worker::v1::worker_error::Error::InternalError(error),
            )),
        }
    }

    async fn invoke_and_await_custom_with_key(
        &self,
        worker_id: &WorkerId,
        idempotency_key: &IdempotencyKey,
        function_name: &str,
        params: Vec<ValueAndType>,
    ) -> anyhow::Result<Result<Vec<Value>, WorkerGrpcError>> {
        let latest_version = self
            .deps
            .component_writer
            .get_latest_component_metadata(&worker_id.component_id)
            .await?;

        let result = self
            .client
            .lock()
            .await
            .invoke_and_await_worker(workerexecutor::v1::InvokeAndAwaitWorkerRequest {
                worker_id: Some(worker_id.clone().into()),
                environment_id: Some(latest_version.environment_id.into()),
                idempotency_key: Some(idempotency_key.clone().into()),
                name: function_name.to_string(),
                input: params
                    .clone()
                    .into_iter()
                    .map(|param| param.value.into())
                    .collect(),
                component_owner_account_id: Some(latest_version.account_id.into()),
                account_limits: None,
                context: None,
                auth_ctx: Some(AuthCtx::System.into()),
            })
            .await;

        let result = result?.into_inner();

        match result.result {
            None => Err(anyhow!(
                "No response from golem-worker-executor invoke call"
            )),
            Some(workerexecutor::v1::invoke_and_await_worker_response::Result::Success(result)) => {
                Ok(Ok(result
                    .output
                    .into_iter()
                    .map(|v| v.try_into())
                    .collect::<Result<Vec<Value>, String>>()
                    .map_err(|err| {
                        anyhow!("Invocation result had unexpected format: {err}")
                    })?))
            }
            Some(workerexecutor::v1::invoke_and_await_worker_response::Result::Failure(error)) => {
                Ok(Err(WorkerGrpcError::InternalError(error)))
            }
        }
    }

    async fn get_worker_metadata(
        &self,
        worker_id: &WorkerId,
    ) -> anyhow::Result<FlatWorkerMetadata> {
        let latest_version = self
            .deps
            .component_writer
            .get_latest_component_metadata(&worker_id.component_id)
            .await?;

        let response = self
            .client
            .lock()
            .await
            .get_worker_metadata(GetWorkerMetadataRequest {
                worker_id: Some(worker_id.clone().into()),
                environment_id: Some(latest_version.environment_id.into()),
                auth_ctx: Some(AuthCtx::System.into()),
            })
            .await?
            .into_inner();

        match response.result {
            None => Err(anyhow!(
                "No response from golem-worker-executor invoke call"
            )),
            Some(workerexecutor::v1::get_worker_metadata_response::Result::Success(result)) => {
                Ok(result
                    .try_into()
                    .map_err(|e| anyhow!("Failed converting worker metadata: {e}"))?)
            }
            Some(workerexecutor::v1::get_worker_metadata_response::Result::Failure(error)) => {
                Err(anyhow!("Failed getting worker metadata: {error:?}"))
            }
        }
    }

    async fn get_oplog(
        &self,
        worker_id: &WorkerId,
        from: OplogIndex,
    ) -> anyhow::Result<Vec<PublicOplogEntryWithIndex>> {
        let latest_version = self
            .deps
            .component_writer
            .get_latest_component_metadata(&worker_id.component_id)
            .await?;

        let mut result = Vec::new();
        let mut cursor = None;

        loop {
            let chunk = self
                .client
                .lock()
                .await
                .get_oplog(workerexecutor::v1::GetOplogRequest {
                    worker_id: Some(worker_id.clone().into()),
                    environment_id: Some(latest_version.environment_id.clone().into()),
                    from_oplog_index: from.into(),
                    cursor,
                    count: 100,
                    auth_ctx: Some(AuthCtx::System.into()),
                })
                .await?
                .into_inner();

            if let Some(chunk) = chunk.result {
                match chunk {
                    get_oplog_response::Result::Success(chunk) => {
                        if chunk.entries.is_empty() {
                            break;
                        } else {
                            result.extend(
                                chunk
                                    .entries
                                    .into_iter()
                                    .enumerate()
                                    .map(|(chunk_idx, entry)| {
                                        PublicOplogEntry::try_from(entry).map(
                                            |public_oplog_entry| PublicOplogEntryWithIndex {
                                                entry: public_oplog_entry,
                                                oplog_index: OplogIndex::from_u64(
                                                    chunk.first_index_in_chunk + chunk_idx as u64,
                                                ),
                                            },
                                        )
                                    })
                                    .collect::<Result<Vec<_>, _>>()
                                    .map_err(|err| {
                                        anyhow!("Failed to convert oplog entry: {err}")
                                    })?,
                            );
                            cursor = chunk.next;
                        }
                    }
                    get_oplog_response::Result::Failure(error) => {
                        return Err(anyhow!("Failed to get oplog: {error:?}"));
                    }
                }
            } else {
                break;
            }
        }

        Ok(result)
    }
}
