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

use crate::config::{CompileWorkerConfig, StaticRegistryServiceConfig};
use crate::metrics::record_compilation_time;
use crate::model::*;
use golem_common::model::environment::EnvironmentId;
use golem_service_base::clients::registry::GrpcRegistryServiceConfig;
use golem_service_base::clients::registry::{GrpcRegistryService, RegistryService};
use golem_service_base::service::compiled_component::CompiledComponentService;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{mpsc, Mutex};
use tokio::task::spawn_blocking;
use tracing::{info, warn, Instrument};
use wasmtime::component::Component;
use wasmtime::Engine;

// Single worker that compiles WASM components.
#[derive(Clone)]
pub struct CompileWorker {
    // Config
    config: CompileWorkerConfig,

    // Resources
    engine: Engine,
    compiled_component_service: Arc<dyn CompiledComponentService>,
    client: Arc<Mutex<Option<GrpcRegistryService>>>,
}

impl CompileWorker {
    pub async fn start(
        component_service_config: Option<StaticRegistryServiceConfig>,
        config: CompileWorkerConfig,

        engine: Engine,
        compiled_component_service: Arc<dyn CompiledComponentService>,

        sender: mpsc::Sender<CompiledComponent>,
        mut recv: mpsc::Receiver<CompilationRequest>,
    ) {
        let worker = Self {
            config,
            engine,
            compiled_component_service,
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

                    let result = worker
                        .compile_component(request.component, request.environment_id)
                        .await;
                    match result {
                        Err(error) => {
                            warn!(
                                component_id = request.component.id.to_string(),
                                component_revision = request.component.revision.to_string(),
                                error = error.to_string(),
                                "Failed to compile component"
                            );
                        }
                        Ok(component) => {
                            let send_result = sender
                                .send(CompiledComponent {
                                    component_and_revision: request.component,
                                    component,
                                    environment_id: request.environment_id,
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

    async fn set_client(&self, config: StaticRegistryServiceConfig) {
        info!(
            "Initializing registry service client for {}:{}",
            config.host, config.port
        );

        let client = GrpcRegistryService::new(&GrpcRegistryServiceConfig {
            host: config.host,
            port: config.port,
            max_message_size: self.config.max_message_size,
            client_config: self.config.client_config.clone(),
        });

        self.client.lock().await.replace(client);
    }

    async fn compile_component(
        &self,
        component_with_revision: ComponentIdAndRevision,
        environment_id: EnvironmentId,
    ) -> Result<Component, CompilationError> {
        let engine = self.engine.clone();

        // Ensure that the component hasn't already been compiled.
        let result = self
            .compiled_component_service
            .get(
                environment_id,
                component_with_revision.id,
                component_with_revision.revision,
                &engine,
            )
            .await;

        match result {
            Ok(Some(component)) => return Ok(component),
            Ok(_) => (),
            Err(err) => {
                warn!(
                    "Failed to download compiled component {:?}: {}",
                    component_with_revision, err
                );
            }
        };

        // TODO: we should download directly from blob store here.
        if let Some(client) = &*self.client.lock().await {
            let bytes = client
                .download_component(component_with_revision.id, component_with_revision.revision)
                .await
                .map_err(|e| CompilationError::ComponentDownloadFailed(e.to_string()))?;

            let start = Instant::now();
            let component = spawn_blocking({
                move || {
                    Component::from_binary(&engine, &bytes).map_err(|e| {
                        CompilationError::CompileFailure(format!(
                            "Failed to compile component {component_with_revision:?}: {e}"
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

            tracing::info!(
                component_id = component_with_revision.id.to_string(),
                component_revision = component_with_revision.revision.to_string(),
                compilation_time_ms = compilation_time.as_millis(),
                "Compiled component"
            );

            Ok(component)
        } else {
            Err(CompilationError::Unexpected(
                "Component service is not configured".to_string(),
            ))
        }
    }
}
