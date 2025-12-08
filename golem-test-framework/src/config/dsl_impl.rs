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
use crate::dsl::{
    build_ifs_archive, rename_component_if_needed, TestDsl, TestDslExtended, WorkerLogEventStream,
};
use crate::model::IFSEntry;
use anyhow::{anyhow, Context};
use applying::Apply;
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::SplitStream;
use futures::{SinkExt, StreamExt};
use golem_api_grpc::proto::golem::worker::LogEvent;
use golem_client::api::{
    RegistryServiceClient, RegistryServiceClientLive, WorkerClient, WorkerClientLive, WorkerError,
};
use golem_client::model::{
    CompleteParameters, InvokeParameters, UpdateWorkerRequest, WorkersMetadataRequest,
};
use golem_common::model::account::AccountId;
use golem_common::model::agent::extraction::extract_agent_types;
use golem_common::model::auth::TokenSecret;
use golem_common::model::component::{ComponentCreation, ComponentUpdate};
use golem_common::model::component::{
    ComponentDto, ComponentFileOptions, ComponentFilePath, ComponentId, ComponentName,
    ComponentRevision, PluginInstallation,
};
use golem_common::model::component_metadata::DynamicLinkedInstance;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::PublicOplogEntryWithIndex;
use golem_common::model::worker::RevertWorkerTarget;
use golem_common::model::worker::{
    FlatComponentFileSystemNode, WorkerCreationRequest, WorkerMetadataDto, WorkerUpdateMode,
};
use golem_common::model::{IdempotencyKey, WorkerEvent};
use golem_common::model::{OplogIndex, WorkerId};
use golem_common::model::{PromiseId, ScanCursor, WorkerFilter};
use golem_wasm::json::OptionallyValueAndTypeJson;
use golem_wasm::{Value, ValueAndType};
use std::borrow::Borrow;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::fs::File;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::frame::Payload;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{Connector, MaybeTlsStream, WebSocketStream};
use uuid::Uuid;

#[derive(Clone)]
pub struct TestUserContext<Deps> {
    pub deps: Deps,
    pub account_id: AccountId,
    pub account_email: String,
    pub token: TokenSecret,
    pub auto_deploy_enabled: bool,
}

impl<Deps> TestUserContext<Deps> {
    pub fn with_auto_deploy(self, enabled: bool) -> Self {
        Self {
            auto_deploy_enabled: enabled,
            ..self
        }
    }
}

#[async_trait]
impl<Deps: TestDependencies> TestDsl for TestUserContext<Deps> {
    type WorkerInvocationResult<T> = anyhow::Result<T>;

    fn redis(&self) -> Arc<dyn Redis> {
        self.deps.redis()
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
        let component_directory = self.deps.component_directory();
        let source_path = component_directory.join(format!("{wasm_name}.wasm"));
        let component_name = if unique {
            let uuid = Uuid::new_v4();
            ComponentName(format!("{name}---{uuid}"))
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
                self.deps.borrow().temp_directory(),
                &source_path,
                &component_name.0,
            )
            .expect("Failed to verify and change component metadata")
        } else {
            source_path
        };

        let (_tmp_dir, maybe_files_archive) = if !files.is_empty() {
            let (tmp_dir, files_archive) = build_ifs_archive(component_directory, &files).await?;
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

        let agent_types = extract_agent_types(&source_path, false, true).await?;

        let component = client
            .create_component(
                &environment_id.0,
                &ComponentCreation {
                    component_name,
                    file_options,
                    dynamic_linking,
                    env,
                    plugins,
                    agent_types,
                },
                File::open(source_path).await?,
                maybe_files_archive,
            )
            .await?;

        if self.auto_deploy_enabled {
            // deploy environment to make component visible
            self.deploy_environment(&component.environment_id).await?;
        }

        Ok(component)
    }

    async fn get_latest_component_version(
        &self,
        component_id: &ComponentId,
    ) -> anyhow::Result<ComponentDto> {
        let client = self.deps.registry_service().client(&self.token).await;
        let component = client.get_component(&component_id.0).await?;
        Ok(component)
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

        if self.auto_deploy_enabled {
            // deploy environment to make component visible
            self.deploy_environment(&component.environment_id).await?;
        }

        Ok(component)
    }

    async fn try_start_worker_with(
        &self,
        component_id: &ComponentId,
        name: &str,
        env: HashMap<String, String>,
        wasi_config_vars: Vec<(String, String)>,
    ) -> anyhow::Result<WorkerId> {
        let client = self
            .deps
            .worker_service()
            .worker_http_client(&self.token)
            .await;
        let wasi_config_vars: BTreeMap<String, String> = wasi_config_vars.into_iter().collect();
        let response = client
            .launch_new_worker(
                &component_id.0,
                &WorkerCreationRequest {
                    name: name.to_string(),
                    env,
                    config_vars: wasi_config_vars.into(),
                },
            )
            .await?;

        Ok(response.worker_id)
    }

    async fn start_worker_with(
        &self,
        component_id: &ComponentId,
        name: &str,
        env: HashMap<String, String>,
        wasi_config_vars: Vec<(String, String)>,
    ) -> anyhow::Result<WorkerId> {
        self.try_start_worker_with(component_id, name, env, wasi_config_vars)
            .await
    }

    async fn invoke_with_key(
        &self,
        worker_id: &WorkerId,
        idempotency_key: &IdempotencyKey,
        function_name: &str,
        params: Vec<ValueAndType>,
    ) -> anyhow::Result<()> {
        let client = self
            .deps
            .worker_service()
            .worker_http_client(&self.token)
            .await;
        client
            .invoke_function(
                &worker_id.component_id.0,
                &worker_id.worker_name,
                Some(&idempotency_key.value),
                function_name,
                &InvokeParameters {
                    params: params
                        .into_iter()
                        .map(|p| p.try_into())
                        .collect::<Result<_, _>>()
                        .map_err(|e| anyhow!("Failed converting params: {e}"))?,
                },
            )
            .await?;
        Ok(())
    }

    async fn invoke_and_await_with_key(
        &self,
        worker_id: &WorkerId,
        idempotency_key: &IdempotencyKey,
        function_name: &str,
        params: Vec<ValueAndType>,
    ) -> anyhow::Result<Vec<Value>> {
        Ok(self
            .invoke_and_await_typed_with_key(worker_id, idempotency_key, function_name, params)
            .await?
            .into_iter()
            .map(|v| v.value)
            .collect())
    }

    async fn invoke_and_await_typed_with_key(
        &self,
        worker_id: &WorkerId,
        idempotency_key: &IdempotencyKey,
        function_name: &str,
        params: Vec<ValueAndType>,
    ) -> anyhow::Result<Option<ValueAndType>> {
        let client = self
            .deps
            .worker_service()
            .worker_http_client(&self.token)
            .await;
        let result = client
            .invoke_and_await_function(
                &worker_id.component_id.0,
                &worker_id.worker_name,
                Some(&idempotency_key.value),
                function_name,
                &InvokeParameters {
                    params: params
                        .into_iter()
                        .map(|p| p.try_into())
                        .collect::<Result<_, _>>()
                        .map_err(|e| anyhow!("Failed converting params: {e}"))?,
                },
            )
            .await?;
        Ok(result.result)
    }

    async fn invoke_and_await_json_with_key(
        &self,
        worker_id: &WorkerId,
        idempotency_key: &IdempotencyKey,
        function_name: &str,
        params: Vec<serde_json::Value>,
    ) -> anyhow::Result<Option<ValueAndType>> {
        let client = self
            .deps
            .worker_service()
            .worker_http_client(&self.token)
            .await;
        let result = client
            .invoke_and_await_function(
                &worker_id.component_id.0,
                &worker_id.worker_name,
                Some(&idempotency_key.value),
                function_name,
                &InvokeParameters {
                    params: params
                        .into_iter()
                        .map(|value| OptionallyValueAndTypeJson { typ: None, value })
                        .collect(),
                },
            )
            .await?;
        Ok(result.result)
    }

    async fn revert(&self, worker_id: &WorkerId, target: RevertWorkerTarget) -> anyhow::Result<()> {
        let client = self
            .deps
            .worker_service()
            .worker_http_client(&self.token)
            .await;
        client
            .revert_worker(&worker_id.component_id.0, &worker_id.worker_name, &target)
            .await?;
        Ok(())
    }

    async fn get_oplog(
        &self,
        worker_id: &WorkerId,
        from: OplogIndex,
    ) -> anyhow::Result<Vec<PublicOplogEntryWithIndex>> {
        let client = self
            .deps
            .worker_service()
            .worker_http_client(&self.token)
            .await;

        let mut result = Vec::new();
        let mut cursor = None;

        loop {
            let response = client
                .get_oplog(
                    &worker_id.component_id.0,
                    &worker_id.worker_name,
                    Some(from.as_u64()),
                    100,
                    cursor.as_ref(),
                    None,
                )
                .await?;

            result.extend(response.entries);
            match response.next {
                None => break,
                Some(next_cursor) => cursor = Some(next_cursor),
            }
        }

        Ok(result)
    }

    async fn search_oplog(
        &self,
        worker_id: &WorkerId,
        query: &str,
    ) -> anyhow::Result<Vec<PublicOplogEntryWithIndex>> {
        let client = self
            .deps
            .worker_service()
            .worker_http_client(&self.token)
            .await;

        let mut result = Vec::new();
        let mut cursor = None;

        loop {
            let response = client
                .get_oplog(
                    &worker_id.component_id.0,
                    &worker_id.worker_name,
                    None,
                    100,
                    cursor.as_ref(),
                    Some(query),
                )
                .await?;

            result.extend(response.entries);
            match response.next {
                None => break,
                Some(next_cursor) => cursor = Some(next_cursor),
            }
        }

        Ok(result)
    }

    async fn interrupt_with_optional_recovery(
        &self,
        worker_id: &WorkerId,
        recover_immediately: bool,
    ) -> anyhow::Result<()> {
        let client = self
            .deps
            .worker_service()
            .worker_http_client(&self.token)
            .await;
        client
            .interrupt_worker(
                &worker_id.component_id.0,
                &worker_id.worker_name,
                Some(recover_immediately),
            )
            .await?;
        Ok(())
    }

    async fn resume(&self, worker_id: &WorkerId, _force: bool) -> anyhow::Result<()> {
        let client = self
            .deps
            .worker_service()
            .worker_http_client(&self.token)
            .await;
        client
            .resume_worker(&worker_id.component_id.0, &worker_id.worker_name)
            .await?;
        Ok(())
    }

    async fn complete_promise(&self, promise_id: &PromiseId, data: Vec<u8>) -> anyhow::Result<()> {
        let client = self
            .deps
            .worker_service()
            .worker_http_client(&self.token)
            .await;
        client
            .complete_promise(
                &promise_id.worker_id.component_id.0,
                &promise_id.worker_id.worker_name,
                &CompleteParameters {
                    oplog_idx: promise_id.oplog_idx.as_u64(),
                    data,
                },
            )
            .await?;
        Ok(())
    }

    async fn make_worker_log_event_stream(
        &self,
        worker_id: &WorkerId,
    ) -> anyhow::Result<impl WorkerLogEventStream> {
        let client = self
            .deps
            .worker_service()
            .worker_http_client(&self.token)
            .await;
        let stream = HttpWorkerLogEventStream::new(Arc::new(client), worker_id).await?;
        Ok(stream)
    }

    async fn auto_update_worker(
        &self,
        worker_id: &WorkerId,
        target_revision: ComponentRevision,
    ) -> anyhow::Result<()> {
        let client = self
            .deps
            .worker_service()
            .worker_http_client(&self.token)
            .await;
        client
            .update_worker(
                &worker_id.component_id.0,
                &worker_id.worker_name,
                &UpdateWorkerRequest {
                    mode: WorkerUpdateMode::Automatic,
                    target_revision: target_revision.0,
                },
            )
            .await?;
        Ok(())
    }

    async fn manual_update_worker(
        &self,
        worker_id: &WorkerId,
        target_revision: ComponentRevision,
    ) -> anyhow::Result<()> {
        let client = self
            .deps
            .worker_service()
            .worker_http_client(&self.token)
            .await;
        client
            .update_worker(
                &worker_id.component_id.0,
                &worker_id.worker_name,
                &UpdateWorkerRequest {
                    mode: WorkerUpdateMode::Manual,
                    target_revision: target_revision.0,
                },
            )
            .await?;
        Ok(())
    }

    async fn delete_worker(&self, worker_id: &WorkerId) -> anyhow::Result<()> {
        let client = self
            .deps
            .worker_service()
            .worker_http_client(&self.token)
            .await;
        client
            .delete_worker(&worker_id.component_id.0, &worker_id.worker_name)
            .await?;
        Ok(())
    }

    async fn get_worker_metadata_opt(
        &self,
        worker_id: &WorkerId,
    ) -> anyhow::Result<Option<WorkerMetadataDto>> {
        let client = self
            .deps
            .worker_service()
            .worker_http_client(&self.token)
            .await;
        match client
            .get_worker_metadata(&worker_id.component_id.0, &worker_id.worker_name)
            .await
        {
            Ok(worker_metadata) => Ok(Some(worker_metadata)),
            Err(golem_client::Error::Item(WorkerError::Error404(_))) => Ok(None),
            Err(err) => Err(err.into()),
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
        let client = self
            .deps
            .worker_service()
            .worker_http_client(&self.token)
            .await;
        let result = client
            .find_workers_metadata(
                &component_id.0,
                &WorkersMetadataRequest {
                    filter,
                    cursor: Some(cursor),
                    count: Some(count),
                    precise: Some(precise),
                },
            )
            .await?;
        Ok((result.cursor, result.workers))
    }

    async fn cancel_invocation(
        &self,
        worker_id: &WorkerId,
        idempotency_key: &IdempotencyKey,
    ) -> anyhow::Result<bool> {
        let client = self
            .deps
            .worker_service()
            .worker_http_client(&self.token)
            .await;
        let result = client
            .cancel_invocation(
                &worker_id.component_id.0,
                &worker_id.worker_name,
                &idempotency_key.value,
            )
            .await?;
        Ok(result.canceled)
    }

    async fn get_file_system_node(
        &self,
        worker_id: &WorkerId,
        path: &str,
    ) -> anyhow::Result<Vec<FlatComponentFileSystemNode>> {
        let client = self
            .deps
            .worker_service()
            .worker_http_client(&self.token)
            .await;
        let result = client
            .get_files(&worker_id.component_id.0, &worker_id.worker_name, path)
            .await?;
        Ok(result.nodes)
    }

    async fn get_file_contents(&self, worker_id: &WorkerId, path: &str) -> anyhow::Result<Bytes> {
        let client = self
            .deps
            .worker_service()
            .worker_http_client(&self.token)
            .await;
        let result = client
            .get_file_content(&worker_id.component_id.0, &worker_id.worker_name, path)
            .await?;
        Ok(result)
    }

    async fn fork_worker(
        &self,
        source_worker_id: &WorkerId,
        target_worker_name: &str,
        oplog_index: OplogIndex,
    ) -> anyhow::Result<()> {
        let client = self
            .deps
            .worker_service()
            .worker_http_client(&self.token)
            .await;

        client
            .fork_worker(
                &source_worker_id.component_id.0,
                &source_worker_id.worker_name,
                &golem_client::model::ForkWorkerRequest {
                    target_worker_id: WorkerId {
                        component_id: source_worker_id.component_id,
                        worker_name: target_worker_name.to_string(),
                    },
                    oplog_index_cutoff: oplog_index.as_u64(),
                },
            )
            .await?;

        Ok(())
    }
}

#[async_trait]
impl<Deps: TestDependencies> TestDslExtended for TestUserContext<Deps> {
    fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    fn custom_request_port(&self) -> u16 {
        self.deps.worker_service().custom_request_port()
    }

    async fn registry_service_client(&self) -> RegistryServiceClientLive {
        self.deps.registry_service().client(&self.token).await
    }
}

struct HttpWorkerLogEventStream {
    read: SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>,
}

impl HttpWorkerLogEventStream {
    async fn new(client: Arc<WorkerClientLive>, worker_id: &WorkerId) -> anyhow::Result<Self> {
        let url = format!(
            "ws://{}:{}/v1/components/{}/workers/{}/connect",
            client.context.base_url.host().unwrap(),
            client.context.base_url.port_or_known_default().unwrap(),
            worker_id.component_id.0,
            worker_id.worker_name,
        );

        let mut connection_request = url
            .into_client_request()
            .context("Failed to create request")?;

        {
            let headers = connection_request.headers_mut();

            if let Some(bearer_token) = client.context.bearer_token() {
                headers.insert("Authorization", format!("Bearer {bearer_token}").parse()?);
            }
        }

        let (stream, _) = tokio_tungstenite::connect_async_tls_with_config(
            connection_request,
            None,
            false,
            Some(Connector::Plain),
        )
        .await?;
        let (mut write, read) = stream.split();

        static PING_HELLO: &str = "hello";
        tokio::task::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            loop {
                interval.tick().await;
                match write
                    .send(Message::Ping(Payload::from(PING_HELLO.as_bytes())))
                    .await
                {
                    Ok(_) => {}
                    Err(error) => break error,
                };
            }
        });

        Ok(Self { read })
    }
}

#[async_trait]
impl WorkerLogEventStream for HttpWorkerLogEventStream {
    async fn message(&mut self) -> anyhow::Result<Option<LogEvent>> {
        match self.read.next().await {
            Some(Ok(message)) => match message {
                Message::Text(payload) => Ok(Some(
                    serde_json::from_str::<WorkerEvent>(payload.as_str())?
                        .try_into()
                        .map_err(|error: String| anyhow!(error))?,
                )),
                Message::Binary(payload) => Ok(Some(
                    serde_json::from_slice::<WorkerEvent>(payload.as_slice())?
                        .try_into()
                        .map_err(|error: String| anyhow!(error))?,
                )),
                Message::Ping(_) => Box::pin(self.message()).await,
                Message::Pong(_) => Box::pin(self.message()).await,
                Message::Close(_) => Ok(None),
                Message::Frame(_) => {
                    panic!("Raw frames should not be received")
                }
            },
            Some(Err(error)) => Err(anyhow!(error)),
            None => Ok(None),
        }
    }
}
