// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::config::{CompileWorkerConfig, StaticComponentServiceConfig};
use crate::model::*;
use futures_util::TryStreamExt;
use golem_api_grpc::proto::golem::component::v1::component_service_client::ComponentServiceClient;
use golem_api_grpc::proto::golem::component::v1::download_component_response;
use golem_api_grpc::proto::golem::component::v1::ComponentError;
use golem_api_grpc::proto::golem::component::v1::DownloadComponentRequest;
use golem_common::client::{GrpcClient, GrpcClientConfig};
use golem_common::metrics::external_calls::record_external_call_response_size_bytes;
use golem_common::model::ComponentId;
use golem_common::model::RetryConfig;
use golem_common::retries::with_retries;
use golem_worker_executor_base::grpc::authorised_grpc_request;
use golem_worker_executor_base::grpc::is_grpc_retriable;
use golem_worker_executor_base::grpc::GrpcError;
use golem_worker_executor_base::metrics::component::record_compilation_time;
use golem_worker_executor_base::services::compiled_component::CompiledComponentService;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{mpsc, Mutex};
use tokio::task::spawn_blocking;
use tonic::codec::CompressionEncoding;
use tonic::transport::Channel;
use tracing::{info, warn, Instrument};
use uuid::Uuid;
use wasmtime::component::Component;
use wasmtime::Engine;

// Single worker that compiles WASM components.
#[derive(Clone)]
pub struct CompileWorker {
    // Config
    config: CompileWorkerConfig,

    // Resources
    engine: Engine,
    compiled_component_service: Arc<dyn CompiledComponentService + Send + Sync>,
    client: Arc<Mutex<Option<ClientWithToken>>>,
}

impl CompileWorker {
    pub async fn start(
        component_service_config: Option<StaticComponentServiceConfig>,
        config: CompileWorkerConfig,

        engine: Engine,
        compiled_component_service: Arc<dyn CompiledComponentService + Send + Sync>,

        sender: mpsc::Sender<CompiledComponent>,
        mut recv: mpsc::Receiver<CompilationRequest>,
    ) {
        let worker = Self {
            engine,
            compiled_component_service,
            config: config.clone(),
            client: Arc::new(Mutex::new(None)),
        };

        if let Some(component_service_config) = component_service_config {
            worker.set_client(component_service_config).await;
        }

        tokio::spawn(
            async move {
                while let Some(request) = recv.recv().await {
                    crate::metrics::decrement_queue_length();

                    if let Some(sender) = request.sender {
                        if worker.client.lock().await.is_none() {
                            worker.set_client(sender).await;
                        }
                    }

                    let result = worker.compile_component(&request.component).await;
                    match result {
                        Err(error) => {
                            warn!(
                                "Failed to compile component {}: {}",
                                request.component, error
                            );
                        }
                        Ok(component) => {
                            info!("Compiled component {}", request.component);
                            let send_result = sender
                                .send(CompiledComponent {
                                    component_and_version: request.component,
                                    component,
                                })
                                .await;

                            if send_result.is_err() {
                                tracing::error!("Failed to send compiled component");
                                break;
                            }
                        }
                    };
                }
            }
            .in_current_span(),
        );
    }

    async fn set_client(&self, config: StaticComponentServiceConfig) {
        info!(
            "Initializing component service client for {}:{}",
            config.host, config.port
        );

        let access_token = config.access_token;
        let max_component_size = self.config.max_component_size;
        let client = GrpcClient::new(
            "component_service",
            move |channel| {
                ComponentServiceClient::new(channel)
                    .max_decoding_message_size(max_component_size)
                    .send_compressed(CompressionEncoding::Gzip)
                    .accept_compressed(CompressionEncoding::Gzip)
            },
            config.uri(),
            GrpcClientConfig {
                retries_on_unavailable: self.config.retries.clone(),
                connect_timeout: self.config.connect_timeout,
            },
        );
        self.client.lock().await.replace(ClientWithToken {
            client,
            access_token,
        });
    }

    async fn compile_component(
        &self,
        component_with_version: &ComponentWithVersion,
    ) -> Result<Component, CompilationError> {
        let engine = self.engine.clone();

        // Ensure that the component hasn't already been compiled.
        let result = self
            .compiled_component_service
            .get(
                &component_with_version.id,
                component_with_version.version,
                &engine,
            )
            .await;

        match result {
            Ok(Some(component)) => return Ok(component),
            Ok(_) => (),
            Err(err) => {
                warn!(
                    "Failed to download compiled component {:?}: {}",
                    component_with_version, err
                );
            }
        };

        if let Some(client) = &*self.client.lock().await {
            let bytes = download_via_grpc(
                &client.client,
                &client.access_token,
                &self.config.retries,
                &component_with_version.id,
                component_with_version.version,
            )
            .await?;

            let start = Instant::now();
            let component = spawn_blocking({
                let component_with_version = component_with_version.clone();
                move || {
                    Component::from_binary(&engine, &bytes).map_err(|e| {
                        CompilationError::CompileFailure(format!(
                            "Failed to compile component {:?}: {}",
                            component_with_version, e
                        ))
                    })
                }
            })
            .instrument(tracing::Span::current())
            .await
            .map_err(|join_err| CompilationError::Unexpected(join_err.to_string()))??;
            let end = Instant::now();

            let compilation_time = end.duration_since(start);

            record_compilation_time(compilation_time);

            tracing::debug!(
                "Compiled {component_with_version:?} in {}ms",
                compilation_time.as_millis(),
            );

            Ok(component)
        } else {
            Err(CompilationError::Unexpected(
                "Component service is not configured".to_string(),
            ))
        }
    }
}

async fn download_via_grpc(
    client: &GrpcClient<ComponentServiceClient<Channel>>,
    access_token: &Uuid,
    retry_config: &RetryConfig,
    component_id: &ComponentId,
    component_version: u64,
) -> Result<Vec<u8>, CompilationError> {
    with_retries(
        "components",
        "download",
        Some(format!("{component_id}@{component_version}")),
        retry_config,
        &(
            client.clone(),
            component_id.clone(),
            access_token.to_owned(),
        ),
        |(client, component_id, access_token)| {
            Box::pin(async move {
                let component_id = component_id.clone();
                let access_token = *access_token;
                let response = client
                    .call("download_component", move |client| {
                        let request = authorised_grpc_request(
                            DownloadComponentRequest {
                                component_id: Some(component_id.clone().into()),
                                version: Some(component_version),
                            },
                            &access_token,
                        );
                        Box::pin(client.download_component(request))
                    })
                    .await?
                    .into_inner();

                let chunks = response.into_stream().try_collect::<Vec<_>>().await?;
                let bytes = chunks
                    .into_iter()
                    .map(|chunk| match chunk.result {
                        None => Err("Empty response".to_string().into()),
                        Some(download_component_response::Result::SuccessChunk(chunk)) => Ok(chunk),
                        Some(download_component_response::Result::Error(error)) => {
                            Err(GrpcError::Domain(error))
                        }
                    })
                    .collect::<Result<Vec<Vec<u8>>, GrpcError<ComponentError>>>()?;

                let bytes: Vec<u8> = bytes.into_iter().flatten().collect();

                record_external_call_response_size_bytes("components", "download", bytes.len());

                Ok(bytes)
            })
        },
        is_grpc_retriable::<ComponentError>,
    )
    .await
    .map_err(|error| {
        tracing::error!("Failed to download component {component_id}@{component_version}: {error}");
        CompilationError::ComponentDownloadFailed(error.to_string())
    })
}

struct ClientWithToken {
    client: GrpcClient<ComponentServiceClient<Channel>>,
    access_token: Uuid,
}
