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
use applying::Apply;
use bytes::Bytes;
use golem_api_grpc::proto::golem::worker::{LogEvent, UpdateMode};
use golem_api_grpc::proto::golem::workerexecutor;
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    cancel_invocation_response, complete_promise_response, create_worker_response,
    delete_worker_response, get_oplog_response, get_workers_metadata_response,
    interrupt_worker_response, resume_worker_response, revert_worker_response,
    search_oplog_response, update_worker_response, CancelInvocationRequest, CompletePromiseRequest,
    ConnectWorkerRequest, CreateWorkerRequest, DeleteWorkerRequest, ForkWorkerRequest,
    GetFileContentsRequest, GetFileSystemNodeRequest, GetWorkerMetadataRequest,
    GetWorkersMetadataRequest, GetWorkersMetadataSuccessResponse, InterruptWorkerRequest,
    ResumeWorkerRequest, RevertWorkerRequest, SearchOplogRequest, UpdateWorkerRequest,
};
use golem_common::model::component::{
    ComponentDto, ComponentFilePath, ComponentId, ComponentName, ComponentRevision,
    InitialComponentFile, PluginInstallation,
};
use golem_common::model::component_metadata::DynamicLinkedInstance;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::{PublicOplogEntry, PublicOplogEntryWithIndex};
use golem_common::model::worker::RevertWorkerTarget;
use golem_common::model::worker::{FlatComponentFileSystemNode, WorkerMetadataDto};
use golem_common::model::PromiseId;
use golem_common::model::{IdempotencyKey, ScanCursor, WorkerFilter};
use golem_common::model::{OplogIndex, WorkerId};
use golem_common::widen_infallible;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::ComponentFileSystemNode;
use golem_service_base::replayable_stream::ReplayableStream;
use golem_test_framework::components::redis::Redis;
use golem_test_framework::dsl::{rename_component_if_needed, TestDsl, WorkerLogEventStream};
use golem_test_framework::model::IFSEntry;
use golem_wasm::{Value, ValueAndType};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use tonic::Streaming;
use uuid::Uuid;

#[async_trait::async_trait]
impl TestDsl for TestWorkerExecutor {
    type WorkerInvocationResult<T> = anyhow::Result<Result<T, WorkerExecutorError>>;

    fn redis(&self) -> Arc<dyn Redis> {
        self.deps.redis.clone()
    }

    async fn store_component_with(
        &self,
        wasm_name: &str,
        environment_id: EnvironmentId,
        name: &str,
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
            ComponentName(name.to_string())
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
            let full_source_path = component_directy.join(entry.source_path);
            let data = tokio::fs::read(full_source_path).await?;
            let content_hash = self
                .deps
                .initial_component_files_service
                .put_if_not_exists(
                    &environment_id,
                    data.map_error(widen_infallible::<anyhow::Error>)
                        .map_item(|i| i.map_err(widen_infallible::<anyhow::Error>)),
                )
                .await?;
            converted_files.push(InitialComponentFile {
                content_hash,
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
                        converted_files,
                        dynamic_linking,
                        unverified,
                        env,
                        environment_id,
                        self.context.application_id,
                        self.context.account_id,
                        HashSet::new(),
                    )
                    .await
                    .expect("Failed to add component")
            } else {
                self.deps
                    .component_writer
                    .get_or_add_component(
                        &source_path,
                        &component_name.0,
                        converted_files,
                        dynamic_linking,
                        unverified,
                        env,
                        environment_id,
                        self.context.application_id,
                        self.context.account_id,
                        HashSet::new(),
                    )
                    .await
            }
        };

        Ok(component)
    }

    async fn get_latest_component_version(
        &self,
        component_id: &ComponentId,
    ) -> anyhow::Result<ComponentDto> {
        self.deps
            .component_writer
            .get_latest_component_metadata(component_id)
            .await
    }

    async fn update_component_with(
        &self,
        component_id: &ComponentId,
        previous_version: ComponentRevision,
        wasm_name: Option<&str>,
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
            let full_source_path = component_directy.join(entry.source_path);
            let data = tokio::fs::read(full_source_path).await?;
            let content_hash = self
                .deps
                .initial_component_files_service
                .put_if_not_exists(
                    &latest_version.environment_id,
                    data.map_error(widen_infallible::<anyhow::Error>)
                        .map_item(|i| i.map_err(widen_infallible::<anyhow::Error>)),
                )
                .await?;
            converted_new_files.push(InitialComponentFile {
                content_hash,
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
        env: HashMap<String, String>,
        wasi_config_vars: Vec<(String, String)>,
    ) -> anyhow::Result<Result<WorkerId, WorkerExecutorError>> {
        let latest_version = self.get_latest_component_version(component_id).await?;

        let worker_id = WorkerId {
            component_id: *component_id,
            worker_name: name.to_string(),
        };

        let response = self
            .client
            .clone()
            .create_worker(CreateWorkerRequest {
                worker_id: Some(worker_id.clone().into()),
                component_version: latest_version.revision.0,
                component_owner_account_id: Some(latest_version.account_id.into()),
                environment_id: Some(latest_version.environment_id.into()),
                env,
                wasi_config_vars: Some(BTreeMap::from_iter(wasi_config_vars).into()),
                ignore_already_existing: false,
                auth_ctx: Some(self.auth_ctx().into()),
            })
            .await?;

        let response = response.into_inner();

        match response.result {
            None => panic!("No response from create_worker"),
            Some(create_worker_response::Result::Success(_)) => Ok(Ok(worker_id)),
            Some(create_worker_response::Result::Failure(error)) => Ok(Err(error
                .try_into()
                .map_err(|e| anyhow!("Failed converting error: {e}"))?)),
        }
    }

    async fn start_worker_with(
        &self,
        component_id: &ComponentId,
        name: &str,
        env: HashMap<String, String>,
        wasi_config_vars: Vec<(String, String)>,
    ) -> anyhow::Result<WorkerId> {
        let result = self
            .try_start_worker_with(component_id, name, env, wasi_config_vars)
            .await??;
        Ok(result)
    }

    async fn invoke_with_key(
        &self,
        worker_id: &WorkerId,
        idempotency_key: &IdempotencyKey,
        function_name: &str,
        params: Vec<ValueAndType>,
    ) -> anyhow::Result<Result<(), WorkerExecutorError>> {
        let latest_version = self
            .get_latest_component_version(&worker_id.component_id)
            .await?;

        let result = self
            .client
            .clone()
            .invoke_worker(workerexecutor::v1::InvokeWorkerRequest {
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
                context: None,
                auth_ctx: Some(self.auth_ctx().into()),
            })
            .await;

        let result = result?.into_inner();

        match result.result {
            None => Err(anyhow!(
                "No response from golem-worker-executor invoke call"
            )),
            Some(workerexecutor::v1::invoke_worker_response::Result::Success(_)) => Ok(Ok(())),
            Some(workerexecutor::v1::invoke_worker_response::Result::Failure(error)) => {
                Ok(Err(error
                    .try_into()
                    .map_err(|e| anyhow!("Failed converting error: {e}"))?))
            }
        }
    }

    async fn invoke_and_await_with_key(
        &self,
        worker_id: &WorkerId,
        idempotency_key: &IdempotencyKey,
        function_name: &str,
        params: Vec<ValueAndType>,
    ) -> anyhow::Result<Result<Vec<Value>, WorkerExecutorError>> {
        let latest_version = self
            .get_latest_component_version(&worker_id.component_id)
            .await?;

        let result = self
            .client
            .clone()
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
                context: None,
                auth_ctx: Some(self.auth_ctx().into()),
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
                Ok(Err(error
                    .try_into()
                    .map_err(|e| anyhow!("Failed converting error: {e}"))?))
            }
        }
    }

    async fn invoke_and_await_typed_with_key(
        &self,
        worker_id: &WorkerId,
        idempotency_key: &IdempotencyKey,
        function_name: &str,
        params: Vec<ValueAndType>,
    ) -> anyhow::Result<Result<Option<ValueAndType>, WorkerExecutorError>> {
        let latest_version = self
            .get_latest_component_version(&worker_id.component_id)
            .await?;

        let result = self
            .client
            .clone()
            .invoke_and_await_worker_typed(workerexecutor::v1::InvokeAndAwaitWorkerRequest {
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
                context: None,
                auth_ctx: Some(self.auth_ctx().into()),
            })
            .await;

        let result = result?.into_inner();

        match result.result {
            None => Err(anyhow!(
                "No response from golem-worker-executor invoke call"
            )),
            Some(workerexecutor::v1::invoke_and_await_worker_response_typed::Result::Success(
                result,
            )) => match result.output {
                None => Ok(Ok(None)),
                Some(response) => {
                    let response: ValueAndType = response
                        .try_into()
                        .map_err(|err| anyhow!("Invocation result had unexpected format: {err}"))?;
                    Ok(Ok(Some(response)))
                }
            },
            Some(workerexecutor::v1::invoke_and_await_worker_response_typed::Result::Failure(
                error,
            )) => Ok(Err(error
                .try_into()
                .map_err(|e| anyhow!("Failed converting error: {e}"))?)),
        }
    }

    async fn invoke_and_await_json_with_key(
        &self,
        worker_id: &WorkerId,
        idempotency_key: &IdempotencyKey,
        function_name: &str,
        params: Vec<serde_json::Value>,
    ) -> anyhow::Result<Result<Option<ValueAndType>, WorkerExecutorError>> {
        let latest_version = self
            .get_latest_component_version(&worker_id.component_id)
            .await?;

        let result = self
            .client
            .clone()
            .invoke_and_await_worker_json(workerexecutor::v1::InvokeAndAwaitWorkerJsonRequest {
                worker_id: Some(worker_id.clone().into()),
                environment_id: Some(latest_version.environment_id.into()),
                idempotency_key: Some(idempotency_key.clone().into()),
                name: function_name.to_string(),
                input: params
                    .clone()
                    .into_iter()
                    .map(|param| {
                        serde_json::to_string(&param).expect("Failed serializing param to json")
                    })
                    .collect(),
                component_owner_account_id: Some(latest_version.account_id.into()),
                context: None,
                auth_ctx: Some(self.auth_ctx().into()),
            })
            .await;

        let result = result?.into_inner();

        match result.result {
            None => Err(anyhow!(
                "No response from golem-worker-executor invoke call"
            )),
            Some(workerexecutor::v1::invoke_and_await_worker_response_typed::Result::Success(
                result,
            )) => match result.output {
                None => Ok(Ok(None)),
                Some(response) => {
                    let response: ValueAndType = response
                        .try_into()
                        .map_err(|err| anyhow!("Invocation result had unexpected format: {err}"))?;
                    Ok(Ok(Some(response)))
                }
            },
            Some(workerexecutor::v1::invoke_and_await_worker_response_typed::Result::Failure(
                error,
            )) => Ok(Err(error
                .try_into()
                .map_err(|e| anyhow!("Failed converting error: {e}"))?)),
        }
    }

    async fn revert(&self, worker_id: &WorkerId, target: RevertWorkerTarget) -> anyhow::Result<()> {
        let latest_version = self
            .get_latest_component_version(&worker_id.component_id)
            .await?;

        let response = self
            .client
            .clone()
            .revert_worker(RevertWorkerRequest {
                worker_id: Some(worker_id.clone().into()),
                environment_id: Some(latest_version.environment_id.into()),
                target: Some(target.into()),
                auth_ctx: Some(self.auth_ctx().into()),
            })
            .await?
            .into_inner();

        match response.result {
            Some(revert_worker_response::Result::Success(_)) => Ok(()),
            Some(revert_worker_response::Result::Failure(error)) => {
                Err(anyhow!("Failed to revert worker: {error:?}"))
            }
            _ => Err(anyhow!("Failed to revert worker: unknown error")),
        }
    }

    async fn get_oplog(
        &self,
        worker_id: &WorkerId,
        from: OplogIndex,
    ) -> anyhow::Result<Vec<PublicOplogEntryWithIndex>> {
        let latest_version = self
            .get_latest_component_version(&worker_id.component_id)
            .await?;

        let mut result = Vec::new();
        let mut cursor = None;

        loop {
            let chunk = self
                .client
                .clone()
                .get_oplog(workerexecutor::v1::GetOplogRequest {
                    worker_id: Some(worker_id.clone().into()),
                    environment_id: Some(latest_version.environment_id.into()),
                    from_oplog_index: from.into(),
                    cursor,
                    count: 100,
                    auth_ctx: Some(self.auth_ctx().into()),
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

    async fn search_oplog(
        &self,
        worker_id: &WorkerId,
        query: &str,
    ) -> anyhow::Result<Vec<PublicOplogEntryWithIndex>> {
        let latest_version = self
            .get_latest_component_version(&worker_id.component_id)
            .await?;

        let mut result = Vec::new();
        let mut cursor = None;

        loop {
            let chunk = self
                .client
                .clone()
                .search_oplog(SearchOplogRequest {
                    worker_id: Some(worker_id.clone().into()),
                    environment_id: Some(latest_version.environment_id.into()),
                    cursor,
                    count: 100,
                    query: query.to_string(),
                    auth_ctx: Some(self.auth_ctx().into()),
                })
                .await?
                .into_inner();

            if let Some(chunk) = chunk.result {
                match chunk {
                    search_oplog_response::Result::Success(chunk) => {
                        if chunk.entries.is_empty() {
                            break;
                        } else {
                            result.extend(
                                chunk
                                    .entries
                                    .into_iter()
                                    .map(|entry| entry.try_into())
                                    .collect::<Result<Vec<_>, _>>()
                                    .map_err(|err| {
                                        anyhow!("Failed to convert oplog entry: {err}")
                                    })?,
                            );
                            cursor = chunk.next;
                        }
                    }
                    search_oplog_response::Result::Failure(error) => {
                        return Err(anyhow!("Failed to search oplog: {error:?}"));
                    }
                }
            } else {
                break;
            }
        }

        Ok(result)
    }

    async fn interrupt_with_optional_recovery(
        &self,
        worker_id: &WorkerId,
        recover_immediately: bool,
    ) -> anyhow::Result<()> {
        let latest_version = self
            .get_latest_component_version(&worker_id.component_id)
            .await?;

        let response = self
            .client
            .clone()
            .interrupt_worker(InterruptWorkerRequest {
                worker_id: Some(worker_id.clone().into()),
                environment_id: Some(latest_version.environment_id.into()),
                recover_immediately,
                auth_ctx: Some(self.auth_ctx().into()),
            })
            .await?
            .into_inner();

        match response.result {
            Some(interrupt_worker_response::Result::Success(_)) => Ok(()),
            Some(interrupt_worker_response::Result::Failure(error)) => {
                panic!("Failed to interrupt worker: {error:?}")
            }
            _ => panic!("Failed to interrupt worker: unknown error"),
        }
    }

    async fn resume(&self, worker_id: &WorkerId, force: bool) -> anyhow::Result<()> {
        let latest_version = self
            .get_latest_component_version(&worker_id.component_id)
            .await?;

        let response = self
            .client
            .clone()
            .resume_worker(ResumeWorkerRequest {
                worker_id: Some(worker_id.clone().into()),
                environment_id: Some(latest_version.environment_id.into()),
                force: Some(force),
                auth_ctx: Some(self.auth_ctx().into()),
            })
            .await?
            .into_inner();

        match response.result {
            Some(resume_worker_response::Result::Success(_)) => Ok(()),
            Some(resume_worker_response::Result::Failure(error)) => {
                Err(anyhow!("Failed to resume worker: {error:?}"))
            }
            None => Err(anyhow!("No response from resume worker")),
        }
    }

    async fn complete_promise(&self, promise_id: &PromiseId, data: Vec<u8>) -> anyhow::Result<()> {
        let latest_version = self
            .get_latest_component_version(&promise_id.worker_id.component_id)
            .await?;

        let response = self
            .client
            .clone()
            .complete_promise(CompletePromiseRequest {
                promise_id: Some(promise_id.clone().into()),
                environment_id: Some(latest_version.environment_id.into()),
                data,
                auth_ctx: Some(self.auth_ctx().into()),
            })
            .await?
            .into_inner();

        match response.result {
            Some(complete_promise_response::Result::Success(_)) => Ok(()),
            Some(complete_promise_response::Result::Failure(error)) => {
                Err(anyhow!("Failed to resume worker: {error:?}"))
            }
            None => Err(anyhow!("No response from resume worker")),
        }
    }

    async fn make_worker_log_event_stream(
        &self,
        worker_id: &WorkerId,
    ) -> anyhow::Result<impl WorkerLogEventStream> {
        let latest_version = self
            .get_latest_component_version(&worker_id.component_id)
            .await?;

        let stream = self
            .client
            .clone()
            .connect_worker(ConnectWorkerRequest {
                worker_id: Some(worker_id.clone().into()),
                environment_id: Some(latest_version.environment_id.into()),
                component_owner_account_id: Some(latest_version.account_id.into()),
                auth_ctx: Some(self.auth_ctx().into()),
            })
            .await?
            .into_inner();

        Ok(GrpcWorkerLogEventStream(stream))
    }

    async fn auto_update_worker(
        &self,
        worker_id: &WorkerId,
        target_version: ComponentRevision,
    ) -> anyhow::Result<()> {
        let latest_version = self
            .get_latest_component_version(&worker_id.component_id)
            .await?;

        let response = self
            .client
            .clone()
            .update_worker(UpdateWorkerRequest {
                worker_id: Some(worker_id.clone().into()),
                environment_id: Some(latest_version.environment_id.into()),
                target_version: target_version.0,
                mode: UpdateMode::Automatic.into(),
                auth_ctx: Some(self.auth_ctx().into()),
            })
            .await?
            .into_inner();

        match response.result {
            Some(update_worker_response::Result::Success(_)) => Ok(()),
            Some(update_worker_response::Result::Failure(error)) => {
                Err(anyhow!("Failed to update worker: {error:?}"))
            }
            _ => Err(anyhow!("Failed to update worker: unknown error")),
        }
    }

    async fn manual_update_worker(
        &self,
        worker_id: &WorkerId,
        target_version: ComponentRevision,
    ) -> anyhow::Result<()> {
        let latest_version = self
            .get_latest_component_version(&worker_id.component_id)
            .await?;

        let response = self
            .client
            .clone()
            .update_worker(UpdateWorkerRequest {
                worker_id: Some(worker_id.clone().into()),
                environment_id: Some(latest_version.environment_id.into()),
                target_version: target_version.0,
                mode: UpdateMode::Manual.into(),
                auth_ctx: Some(self.auth_ctx().into()),
            })
            .await?
            .into_inner();

        match response.result {
            Some(update_worker_response::Result::Success(_)) => Ok(()),
            Some(update_worker_response::Result::Failure(error)) => {
                Err(anyhow!("Failed to update worker: {error:?}"))
            }
            _ => Err(anyhow!("Failed to update worker: unknown error")),
        }
    }

    async fn delete_worker(&self, worker_id: &WorkerId) -> anyhow::Result<()> {
        let latest_version = self
            .get_latest_component_version(&worker_id.component_id)
            .await?;

        let response = self
            .client
            .clone()
            .delete_worker(DeleteWorkerRequest {
                worker_id: Some(worker_id.clone().into()),
                environment_id: Some(latest_version.environment_id.into()),
                auth_ctx: Some(self.auth_ctx().into()),
            })
            .await?
            .into_inner();

        match response.result {
            Some(delete_worker_response::Result::Success(_)) => Ok(()),
            Some(delete_worker_response::Result::Failure(error)) => {
                Err(anyhow!("Failed to delete worker: {error:?}"))
            }
            _ => Err(anyhow!("Failed to delete worker: unknown error")),
        }
    }

    async fn get_worker_metadata_opt(
        &self,
        worker_id: &WorkerId,
    ) -> anyhow::Result<Option<WorkerMetadataDto>> {
        let latest_version = self
            .get_latest_component_version(&worker_id.component_id)
            .await?;

        let response = self
            .client
            .clone()
            .get_worker_metadata(GetWorkerMetadataRequest {
                worker_id: Some(worker_id.clone().into()),
                environment_id: Some(latest_version.environment_id.into()),
                auth_ctx: Some(self.auth_ctx().into()),
            })
            .await?
            .into_inner();

        match response.result {
            None => Err(anyhow!(
                "No response from golem-worker-executor invoke call"
            )),
            Some(workerexecutor::v1::get_worker_metadata_response::Result::Success(result)) => {
                Ok(Some(result
                    .try_into()
                    .map_err(|e| anyhow!("Failed converting worker metadata: {e}"))?))
            }
            Some(workerexecutor::v1::get_worker_metadata_response::Result::Failure(error)) => {
                match error {
                    golem_api_grpc::proto::golem::worker::v1::WorkerExecutionError {
                        error: Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::WorkerNotFound(_)),
                    } => Ok(None),
                    _ => Err(anyhow!("Failed getting worker metadata: {error:?}")),
                }
            }
        }
    }

    async fn get_workers_metadata(
        &self,
        component_id: &ComponentId,
        filter: Option<WorkerFilter>,
        cursor: ScanCursor,
        count: u64,
        precise: bool,
    ) -> anyhow::Result<(Option<ScanCursor>, Vec<WorkerMetadataDto>)> {
        let latest_version = self.get_latest_component_version(component_id).await?;

        let response = self
            .client
            .clone()
            .get_workers_metadata(GetWorkersMetadataRequest {
                component_id: Some((*component_id).into()),
                environment_id: Some(latest_version.environment_id.into()),
                filter: filter.map(|f| f.into()),
                cursor: Some(cursor.into()),
                count,
                precise,
                auth_ctx: Some(self.auth_ctx().into()),
            })
            .await?
            .into_inner();

        match response.result {
            None => Err(anyhow!("No response from get_workers_metadata")),
            Some(get_workers_metadata_response::Result::Success(
                GetWorkersMetadataSuccessResponse { workers, cursor },
            )) => Ok((
                cursor.map(|c| c.into()),
                workers
                    .into_iter()
                    .map(|w| w.try_into())
                    .collect::<Result<_, _>>()
                    .map_err(|e| anyhow!("Failed converting worker metadata: {e}"))?,
            )),
            Some(get_workers_metadata_response::Result::Failure(error)) => {
                Err(anyhow!("Failed to get workers metadata: {error:?}"))
            }
        }
    }

    async fn cancel_invocation(
        &self,
        worker_id: &WorkerId,
        idempotency_key: &IdempotencyKey,
    ) -> anyhow::Result<bool> {
        let latest_version = self
            .get_latest_component_version(&worker_id.component_id)
            .await?;

        let response = self
            .client
            .clone()
            .cancel_invocation(CancelInvocationRequest {
                worker_id: Some(worker_id.clone().into()),
                idempotency_key: Some(idempotency_key.clone().into()),
                environment_id: Some(latest_version.environment_id.into()),
                auth_ctx: Some(self.auth_ctx().into()),
            })
            .await?
            .into_inner();

        match response.result {
            Some(cancel_invocation_response::Result::Success(canceled)) => Ok(canceled),
            Some(cancel_invocation_response::Result::Failure(error)) => {
                Err(anyhow!("Failed to cancel invocation: {error:?}"))
            }
            _ => Err(anyhow!("Failed to cancel invocation: unknown error")),
        }
    }

    async fn get_file_system_node(
        &self,
        worker_id: &WorkerId,
        path: &str,
    ) -> anyhow::Result<Vec<FlatComponentFileSystemNode>> {
        let latest_version = self
            .get_latest_component_version(&worker_id.component_id)
            .await?;

        let response = self
            .client
            .clone()
            .get_file_system_node(GetFileSystemNodeRequest {
                worker_id: Some(worker_id.clone().into()),
                path: path.to_string(),
                environment_id: Some(latest_version.environment_id.into()),
                component_owner_account_id: Some(latest_version.account_id.into()),
                auth_ctx: Some(self.auth_ctx().into()),
            })
            .await?
            .into_inner();

        match response.result {
            Some(workerexecutor::v1::get_file_system_node_response::Result::DirSuccess(data)) => {
                data.nodes
                    .into_iter()
                    .map(|v| {
                        let converted: ComponentFileSystemNode = v
                            .try_into()
                            .map_err(|_| anyhow!("Failed to convert node"))?;
                        Ok::<_, anyhow::Error>(converted.into())
                    })
                    .collect::<Result<Vec<_>, _>>()
            }
            Some(workerexecutor::v1::get_file_system_node_response::Result::FileSuccess(data)) => {
                let file_node = data
                    .file
                    .ok_or(anyhow!("Missing file data in response"))?
                    .apply(ComponentFileSystemNode::try_from)
                    .map_err(|_| anyhow!("Failed to convert file node"))?
                    .into();
                Ok(vec![file_node])
            }
            Some(workerexecutor::v1::get_file_system_node_response::Result::NotFound(_)) => {
                Ok(Vec::new())
            }
            Some(workerexecutor::v1::get_file_system_node_response::Result::Failure(error)) => {
                Err(anyhow!("Error getting file system node: {error:?}"))
            }
            None => Err(anyhow!(
                "No response from golem-worker-executor list-directory call"
            )),
        }
    }

    async fn get_file_contents(&self, worker_id: &WorkerId, path: &str) -> anyhow::Result<Bytes> {
        let latest_version = self
            .get_latest_component_version(&worker_id.component_id)
            .await?;

        let mut stream = self
            .client
            .clone()
            .get_file_contents(GetFileContentsRequest {
                worker_id: Some(worker_id.clone().into()),
                file_path: path.to_string(),
                environment_id: Some(latest_version.environment_id.into()),
                component_owner_account_id: Some(latest_version.account_id.into()),
                auth_ctx: Some(self.auth_ctx().into()),
            })
            .await?
            .into_inner();

        let mut bytes = Vec::new();
        while let Some(chunk) = stream.message().await? {
            match chunk.result {
                Some(workerexecutor::v1::get_file_contents_response::Result::Success(data)) => {
                    bytes.extend_from_slice(&data);
                }
                Some(workerexecutor::v1::get_file_contents_response::Result::Header(header)) => {
                    match header.result {
                        Some(
                            workerexecutor::v1::get_file_contents_response_header::Result::Success(
                                _,
                            ),
                        ) => {}
                        _ => {
                            return Err(anyhow!("Unexpected header from get_file_contents"));
                        }
                    }
                }
                Some(workerexecutor::v1::get_file_contents_response::Result::Failure(err)) => {
                    return Err(anyhow!("Error from get_file_contents: {err:?}"));
                }
                None => {
                    return Err(anyhow!("Unexpected response from get_file_contents"));
                }
            }
        }
        Ok(Bytes::from(bytes))
    }

    async fn fork_worker(
        &self,
        source_worker_id: &WorkerId,
        target_worker_name: &str,
        oplog_index: OplogIndex,
    ) -> anyhow::Result<()> {
        let latest_version = self
            .get_latest_component_version(&source_worker_id.component_id)
            .await?;

        let target_worker_id = WorkerId {
            component_id: source_worker_id.component_id,
            worker_name: target_worker_name.to_string(),
        };

        let response = self
            .client
            .clone()
            .fork_worker(ForkWorkerRequest {
                source_worker_id: Some(source_worker_id.clone().into()),
                target_worker_id: Some(target_worker_id.into()),
                oplog_index_cutoff: oplog_index.into(),
                environment_id: Some(latest_version.environment_id.into()),
                component_owner_account_id: Some(latest_version.account_id.into()),
                auth_ctx: Some(self.auth_ctx().into()),
            })
            .await?
            .into_inner();

        match response.result {
            Some(workerexecutor::v1::fork_worker_response::Result::Success(_)) => Ok(()),
            Some(workerexecutor::v1::fork_worker_response::Result::Failure(error)) => {
                Err(anyhow!("Error forking worker: {error:?}"))
            }
            None => Err(anyhow!(
                "No response from golem-worker-executor fork-worker call"
            )),
        }
    }
}

struct GrpcWorkerLogEventStream(Streaming<LogEvent>);

#[async_trait::async_trait]
impl WorkerLogEventStream for GrpcWorkerLogEventStream {
    async fn message(&mut self) -> anyhow::Result<Option<LogEvent>> {
        self.0
            .message()
            .await
            .map_err(|e| anyhow!("Failed to receive log event: {e}"))
    }
}
