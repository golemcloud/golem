use crate::config::CompileWorkerConfig;
use crate::model::*;
use crate::UriBackConversion;
use futures_util::TryStreamExt;
use golem_api_grpc::proto::golem::template::download_template_response;
use golem_api_grpc::proto::golem::template::template_service_client::TemplateServiceClient;
use golem_api_grpc::proto::golem::template::DownloadTemplateRequest;
use golem_api_grpc::proto::golem::template::TemplateError;
use golem_common::config::RetryConfig;
use golem_common::metrics::external_calls::record_external_call_response_size_bytes;
use golem_common::model::TemplateId;
use golem_common::retries::with_retries;
use golem_worker_executor_base::grpc::authorised_grpc_request;
use golem_worker_executor_base::grpc::is_grpc_retriable;
use golem_worker_executor_base::grpc::GrpcError;
use golem_worker_executor_base::metrics::template::record_compilation_time;
use golem_worker_executor_base::services::compiled_template::CompiledTemplateService;
use http::Uri;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;
use uuid::Uuid;
use wasmtime::component::Component;
use wasmtime::Engine;

// Single worker that compiles templates.
#[derive(Clone)]
pub struct CompileWorker {
    // Config
    uri: Uri,
    access_token: Uuid,
    config: CompileWorkerConfig,

    // Resources
    engine: Engine,
    compiled_template_service: Arc<dyn CompiledTemplateService + Send + Sync>,
}

impl CompileWorker {
    pub fn start(
        uri: Uri,
        access_token: Uuid,
        config: CompileWorkerConfig,

        engine: Engine,
        compiled_template_service: Arc<dyn CompiledTemplateService + Send + Sync>,

        sender: mpsc::Sender<CompiledTemplate>,
        mut recv: mpsc::Receiver<CompilationRequest>,
    ) {
        let worker = Self {
            uri,
            engine,
            compiled_template_service,
            config,
            access_token,
        };

        tokio::spawn(async move {
            while let Some(request) = recv.recv().await {
                crate::metrics::decrement_queue_length();
                let result = worker.compile_template(&request.template).await;
                match result {
                    Err(_) => {}
                    Ok(component) => {
                        tracing::info!("Compiled template {}", request.template);
                        let send_result = sender
                            .send(CompiledTemplate {
                                template: request.template,
                                component,
                            })
                            .await;

                        if send_result.is_err() {
                            tracing::error!("Failed to send compiled template");
                            break;
                        }
                    }
                };
            }
        });
    }

    async fn compile_template(
        &self,
        template: &TemplateWithVersion,
    ) -> Result<Component, CompilationError> {
        let engine = self.engine.clone();

        // Ensure that the template hasn't already been compiled.
        let result = self
            .compiled_template_service
            .get(&template.id, template.version, &engine)
            .await;

        match result {
            Ok(Some(component)) => return Ok(component),
            Ok(_) => (),
            Err(err) => {
                tracing::warn!(
                    "Failed to download compiled component {:?}: {}",
                    template,
                    err
                );
            }
        };

        let bytes = download_via_grpc(
            &self.uri,
            &self.access_token,
            &self.config.retries,
            &template.id,
            template.version,
            self.config.max_template_size,
        )
        .await?;

        let start = Instant::now();
        let component = Component::from_binary(&engine, &bytes).map_err(|e| {
            CompilationError::CompileFailure(format!(
                "Failed to compile template {:?}: {}",
                template, e
            ))
        })?;
        let end = Instant::now();

        let compilation_time = end.duration_since(start);

        record_compilation_time(compilation_time);

        tracing::debug!(
            "Compiled {template:?} in {}ms",
            compilation_time.as_millis(),
        );

        Ok(component)
    }
}

async fn download_via_grpc(
    endpoint: &Uri,
    access_token: &Uuid,
    retry_config: &RetryConfig,
    template_id: &TemplateId,
    template_version: u64,
    max_template_size: usize,
) -> Result<Vec<u8>, CompilationError> {
    let desc = format!("Downloading template {template_id}@{template_version}");
    tracing::debug!("{}", &desc);
    with_retries(
        &desc,
        "components",
        "download",
        retry_config,
        &(
            endpoint.clone(),
            template_id.clone(),
            access_token.to_owned(),
        ),
        |(endpoint, template_id, access_token)| {
            Box::pin(async move {
                let mut client = TemplateServiceClient::connect(endpoint.as_http_02())
                    .await?
                    .max_decoding_message_size(max_template_size);

                let request = authorised_grpc_request(
                    DownloadTemplateRequest {
                        template_id: Some(template_id.clone().into()),
                        version: Some(template_version),
                    },
                    access_token,
                );

                let response = client.download_template(request).await?.into_inner();

                let chunks = response.into_stream().try_collect::<Vec<_>>().await?;
                let bytes = chunks
                    .into_iter()
                    .map(|chunk| match chunk.result {
                        None => Err("Empty response".to_string().into()),
                        Some(download_template_response::Result::SuccessChunk(chunk)) => Ok(chunk),
                        Some(download_template_response::Result::Error(error)) => {
                            Err(GrpcError::Domain(error))
                        }
                    })
                    .collect::<Result<Vec<Vec<u8>>, GrpcError<TemplateError>>>()?;

                let bytes: Vec<u8> = bytes.into_iter().flatten().collect();

                record_external_call_response_size_bytes("components", "download", bytes.len());

                Ok(bytes)
            })
        },
        is_grpc_retriable::<TemplateError>,
    )
    .await
    .map_err(|error| {
        tracing::error!("Failed to download template {template_id}@{template_version}: {error}");
        CompilationError::TemplateDownloadFailed(error.to_string())
    })
}
