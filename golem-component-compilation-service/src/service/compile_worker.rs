// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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
use golem_service_base::clients::registry::GrpcRegistryServiceConfig;
use golem_service_base::clients::registry::{GrpcRegistryService, RegistryService};
use golem_service_base::service::compiled_component::CompiledComponentService;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::task::spawn_blocking;
use tracing::{Instrument, info, warn};
use wasmtime::Engine;
use wasmtime::component::Component;

// Single worker that compiles WASM components.
#[derive(Clone)]
pub struct CompileWorker {
    // Config
    config: CompileWorkerConfig,

    // Resources
    engine: Engine,
    client: Arc<Mutex<Option<GrpcRegistryService>>>,
}

impl CompileWorker {
    pub async fn start(
        component_service_config: Option<StaticRegistryServiceConfig>,
        config: CompileWorkerConfig,

        engine: Engine,
        compiled_component_service: Arc<dyn CompiledComponentService>,

        sender: mpsc::Sender<CompiledComponent>,
        recv: mpsc::Receiver<CompilationRequest>,
    ) {
        let worker = Self {
            config,
            engine,
            client: Arc::new(Mutex::new(None)),
        };

        if let Some(component_service_config) = component_service_config {
            worker.set_client(component_service_config).await;
        }

        let engine = worker.engine.clone();
        Self::start_queue(
            engine,
            compiled_component_service,
            sender,
            recv,
            move |request| {
                let worker = worker.clone();
                async move {
                    if let Some(sender) = request.sender.clone()
                        && worker.client.lock().await.is_none()
                    {
                        worker.set_client(sender).await;
                    }
                    worker.compile_component(&request).await
                }
            },
        );
    }

    pub(super) fn start_queue<F>(
        engine: Engine,
        compiled_component_service: Arc<dyn CompiledComponentService>,
        sender: mpsc::Sender<CompiledComponent>,
        mut recv: mpsc::Receiver<CompilationRequest>,
        compile: impl Fn(CompilationRequest) -> F + Send + 'static,
    ) where
        F: Future<Output = Result<Component, CompilationError>> + Send,
    {
        tokio::spawn(async move {
                let mut uploads: HashMap<_, oneshot::Receiver<()>> = HashMap::new();
                while let Some(request) = recv.recv().await {
                    crate::metrics::decrement_queue_length();

                    uploads.retain(|_, upload| matches!(upload.try_recv(), Err(oneshot::error::TryRecvError::Empty)));
                    // Precompilation is best effort. Do not block unrelated requests on a duplicate.
                    if uploads.contains_key(&request.wasm_hash) {
                        tracing::debug!(wasm_hash = %request.wasm_hash, "Compiled component upload already pending");
                        continue;
                    }

                    match compiled_component_service.exists(request.wasm_hash, &engine).await {
                        Ok(true) => {
                            tracing::debug!(wasm_hash = %request.wasm_hash, "Compiled component already cached");
                            continue;
                        }
                        Ok(false) => (),
                        Err(error) => warn!(wasm_hash = %request.wasm_hash, %error, "Failed to check for compiled component"),
                    }

                    let result = compile(request.clone()).await;
                    match result {
                        Err(error) => {
                            warn!(
                                component_id = request.component.id.to_string(),
                                component_revision = request.component.revision.to_string(),
                                error = error.to_string(),
                                "Failed to compile component"
                            );
                        }
                        Ok(component) => match sender.reserve().await {
                            Ok(permit) => {
                                let (upload_finished, upload) = oneshot::channel();
                                uploads.insert(request.wasm_hash, upload);
                                crate::metrics::increment_upload_queue_length();
                                permit.send(CompiledComponent {
                                    component_and_revision: request.component,
                                    component,
                                    wasm_hash: request.wasm_hash,
                                    upload_finished,
                                });
                            }
                            Err(_) => {
                                tracing::error!("Failed to send compiled component");
                                break;
                            }
                        },
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
            invalidation_event_subscriber: Default::default(),
            client_config: self.config.client_config.clone(),
        });

        self.client.lock().await.replace(client);
    }

    async fn compile_component(
        &self,
        request: &CompilationRequest,
    ) -> Result<Component, CompilationError> {
        let component_with_revision = request.component;

        // TODO: we should download directly from blob store here.
        if let Some(client) = &*self.client.lock().await {
            let bytes = client
                .download_component(component_with_revision.id, component_with_revision.revision)
                .await
                .map_err(|e| CompilationError::ComponentDownloadFailed(e.to_string()))?;

            Self::compile_downloaded(self.engine.clone(), request, bytes).await
        } else {
            Err(CompilationError::Unexpected(
                "Component service is not configured".to_string(),
            ))
        }
    }

    pub(super) async fn compile_downloaded(
        engine: Engine,
        request: &CompilationRequest,
        bytes: Vec<u8>,
    ) -> Result<Component, CompilationError> {
        let component_with_revision = request.component;
        let wasm_hash = request.wasm_hash;
        let start = Instant::now();
        let component = spawn_blocking(move || {
            // The request can only populate the cache entry for the bytes we downloaded.
            if blake3::hash(&bytes) != *wasm_hash.as_blake3_hash() {
                return Err(CompilationError::ComponentDownloadFailed(format!(
                    "Wasm hash mismatch for {component_with_revision}"
                )));
            }
            Component::from_binary(&engine, &bytes).map_err(|e| {
                CompilationError::CompileFailure(format!(
                    "Failed to compile component {component_with_revision:?}: {e}"
                ))
            })
        })
        .instrument(tracing::Span::current())
        .await
        .map_err(|join_err| CompilationError::Unexpected(join_err.to_string()))??;
        let compilation_time = start.elapsed();

        record_compilation_time(compilation_time);

        tracing::info!(
            component_id = component_with_revision.id.to_string(),
            component_revision = component_with_revision.revision.to_string(),
            compilation_time_ms = compilation_time.as_millis(),
            "Compiled component"
        );

        Ok(component)
    }
}
