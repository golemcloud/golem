use crate::config::CompileWorkerConfig;
use crate::model::*;
use crate::UriBackConversion;
use futures_util::TryStreamExt;
use golem_api_grpc::proto::golem::component::download_component_response;
use golem_api_grpc::proto::golem::component::component_service_client::ComponentServiceClient;
use golem_api_grpc::proto::golem::component::DownloadComponentRequest;
use golem_api_grpc::proto::golem::component::ComponentError;
use golem_common::config::RetryConfig;
use golem_common::metrics::external_calls::record_external_call_response_size_bytes;
use golem_common::model::ComponentId;
use golem_common::retries::with_retries;
use golem_worker_executor_base::grpc::authorised_grpc_request;
use golem_worker_executor_base::grpc::is_grpc_retriable;
use golem_worker_executor_base::grpc::GrpcError;
use golem_worker_executor_base::metrics::component::record_compilation_time;
use golem_worker_executor_base::services::compiled_component::CompiledComponentService;
use http::Uri;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;
use uuid::Uuid;
use wasmtime::component::Component;
use wasmtime::Engine;

// Single worker that compiles WASM components.
#[derive(Clone)]
pub struct CompileWorker {
    // Config
    uri: Uri,
    access_token: Uuid,
    config: CompileWorkerConfig,

    // Resources
    engine: Engine,
    compiled_component_service: Arc<dyn CompiledComponentService + Send + Sync>,
}

impl CompileWorker {
    pub fn start(
        uri: Uri,
        access_token: Uuid,
        config: CompileWorkerConfig,

        engine: Engine,
        compiled_component_service: Arc<dyn CompiledComponentService + Send + Sync>,

        sender: mpsc::Sender<CompiledComponent>,
        mut recv: mpsc::Receiver<CompilationRequest>,
    ) {
        let worker = Self {
            uri,
            engine,
            compiled_component_service,
            config,
            access_token,
        };

        tokio::spawn(async move {
            while let Some(request) = recv.recv().await {
                crate::metrics::decrement_queue_length();
                let result = worker.compile_component(&request.component).await;
                match result {
                    Err(_) => {}
                    Ok(component) => {
                        tracing::info!("Compiled component {}", request.component);
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
            .get(&component_with_version.id, component_with_version.version, &engine)
            .await;

        match result {
            Ok(Some(component)) => return Ok(component),
            Ok(_) => (),
            Err(err) => {
                tracing::warn!(
                    "Failed to download compiled component {:?}: {}",
                    component_with_version,
                    err
                );
            }
        };

        let bytes = download_via_grpc(
            &self.uri,
            &self.access_token,
            &self.config.retries,
            &component_with_version.id,
            component_with_version.version,
            self.config.max_component_size,
        )
        .await?;

        let start = Instant::now();
        let component = Component::from_binary(&engine, &bytes).map_err(|e| {
            CompilationError::CompileFailure(format!(
                "Failed to compile component {:?}: {}",
                component_with_version, e
            ))
        })?;
        let end = Instant::now();

        let compilation_time = end.duration_since(start);

        record_compilation_time(compilation_time);

        tracing::debug!(
            "Compiled {component_with_version:?} in {}ms",
            compilation_time.as_millis(),
        );

        Ok(component)
    }
}

async fn download_via_grpc(
    endpoint: &Uri,
    access_token: &Uuid,
    retry_config: &RetryConfig,
    component_id: &ComponentId,
    component_version: u64,
    max_component_size: usize,
) -> Result<Vec<u8>, CompilationError> {
    let desc = format!("Downloading component {component_id}@{component_version}");
    tracing::debug!("{}", &desc);
    with_retries(
        &desc,
        "components",
        "download",
        retry_config,
        &(
            endpoint.clone(),
            component_id.clone(),
            access_token.to_owned(),
        ),
        |(endpoint, component_id, access_token)| {
            Box::pin(async move {
                let mut client = ComponentServiceClient::connect(endpoint.as_http_02())
                    .await?
                    .max_decoding_message_size(max_component_size);

                let request = authorised_grpc_request(
                    DownloadComponentRequest {
                        component_id: Some(component_id.clone().into()),
                        version: Some(component_version),
                    },
                    access_token,
                );

                let response = client.download_component(request).await?.into_inner();

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
