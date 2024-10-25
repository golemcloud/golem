// Copyright 2024 Golem Cloud
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

use std::sync::Arc;
use futures_util::TryStreamExt;
use http::Uri;
use golem_worker_executor_base::services::compiled_component::CompiledComponentService;
use tokio::sync::mpsc;
use tokio::sync::mpsc::Sender;
use tonic::codegen::tokio_stream::StreamExt;
use tonic::transport::Channel;
use tracing::log::{error, info};
use uuid::Uuid;
use golem_api_grpc::proto::golem::common::ErrorBody;
use golem_api_grpc::proto::golem::component::v1::component_service_client::ComponentServiceClient;
use golem_api_grpc::proto::golem::component::v1::{download_ifs_response, ComponentError, DownloadIfsRequest};
use golem_api_grpc::proto::golem::component::v1::component_error::Error;
use golem_api_grpc::proto::golem::component::v1::ifs_service_client::IfsServiceClient;
use golem_common::client::{GrpcClient, GrpcClientConfig};
use golem_common::config::RetryConfig;
use golem_common::metrics::external_calls::record_external_call_response_size_bytes;
use golem_common::model::ComponentId;
use golem_common::retries::with_retries;
use golem_common::tracing::directive::default::info;
use golem_worker_executor_base::grpc::{authorised_grpc_request, is_grpc_retriable, GrpcError, UriBackConversion};
use golem_worker_executor_base::services::ifs::InitialFileSystem;
use crate::config::{CompileWorkerConfig, IFSWorkerConfig};
use crate::model::*;

// Worker that uploads compiled components to the cloud.
#[derive(Clone)]
pub struct UploadWorker {
    // Config
    access_token: Uuid,
    config: CompileWorkerConfig,

    // Resources
    compiled_component_service: Arc<dyn CompiledComponentService + Send + Sync>,
    client: GrpcClient<IfsServiceClient<Channel>>,
    ifs_tx: Sender<InitialFileSystemToUpload>

}

impl UploadWorker {
    pub fn start(
        uri: Uri,
        access_token: Uuid,
        compiled_component_service: Arc<dyn CompiledComponentService + Send + Sync>,
        config: CompileWorkerConfig,
        mut recv: mpsc::Receiver<CompiledComponent>,
        ifs_tx: mpsc::Sender<InitialFileSystemToUpload>,
    ) {
        info!("Uploader started -------------------------------------------");
        let worker = Self {
            compiled_component_service,
            access_token,
            config: config.clone(),
            client: GrpcClient::new(
                move |channel| {
                    IfsServiceClient::new(channel)
                },
                uri.as_http_02(),
                    GrpcClientConfig{
                        retries_on_unavailable: config.clone().retries.clone(),
                        ..Default::default()
                    }
            ),
            ifs_tx
        };

        tokio::spawn(async move {
            loop {
                while let Some(request) = recv.recv().await {
                    worker.upload_component(request).await
                }
            }
        });
    }

    // Don't need retries because they're baked into CompiledComponentService.
    async fn upload_component(&self, compiled_component: CompiledComponent) {
        let CompiledComponent {
            component_and_version,
            component,
        } = compiled_component;

        info!("Uploading the worker content -------------------");



        let upload_result = self
            .compiled_component_service
            .put(
                &component_and_version.id,
                component_and_version.version,
                &component,
            )
            .await
            .map_err(|err| CompilationError::ComponentUploadFailed(err.to_string()));

        if let Err(ref err) = upload_result {
            tracing::warn!("Failed to upload compiled component {component_and_version}: {err:?}");
        } else {
            tracing::info!("Successfully uploaded compiled component {component_and_version}");
            if let Err(err) = self.download_and_process_ifs(&component_and_version).await {
                tracing::error!("Failed to process IFS for component {component_and_version}: {err:?}");
            }
            //     Now I want to download the ifs system
        }
    }


    async fn download_and_process_ifs(
        &self,
        component_and_version: &ComponentWithVersion
    ) -> Result<(), ComponentError> {

        info!("Triggered initial sending -----------------------------------");

        let ifs_data = Self::download_ifs_via_grpc(
            &self.client,
            &self.access_token,
            &self.config.retries,
            &component_and_version.id,
            component_and_version.version
        ).await.expect("download_ifs_via_grpc failed");

        let ifs_upload_request = InitialFileSystemToUpload {
            component_and_version: component_and_version.clone(),
            initial_file_system: ifs_data,
        };

        if let Err(err) = self.ifs_tx.send(ifs_upload_request).await {
            error!("Failed to send IFS upload request: {err:?}");
            return Err(ComponentError {
                error: Some(Error::InternalError(ErrorBody{
                    error: format!("Failed to send IFS upload request: {err:?}")
                }))
            });
        }
        Ok(())

    }

    async fn download_ifs_via_grpc(
        client: &GrpcClient<IfsServiceClient<Channel>>,
        access_token: &Uuid,
        retry_config: &RetryConfig,
        component_id: &ComponentId,
        component_version: u64,
    ) -> Result<Vec<u8>, CompilationError>{
        info!("downloading ifs via grpc --------------------------------------");
        with_retries(
            "ifs",
            "download",
            Some(format!("{component_id}@{component_version}")),
            retry_config,
            &(
                client.clone(),
                component_id.clone(),
                access_token.to_owned(),
                ),
            |(client, component_id, access_token)|{
                Box::pin(async move {
                    let component_id = component_id.clone();
                    let access_token = *access_token;
                    let response = client.call(move |client| {

                        let request = authorised_grpc_request(
                            DownloadIfsRequest{
                                    component_id : Some(component_id.clone().into()),
                                    version: Some(component_version),
                            },
                            &access_token,
                        );

                        info!("&&&&&&&&&&&&&&&&&&&&&& Authorized --------------");
                        let t = Box::pin(client.download_ifs(request));
                        info!("&&&&&&&&&&&&&&&&&&&&&& download ifs --------------");
                        t
                    })
                        .await?.into_inner();


                    let chunks = response.into_stream().try_collect::<Vec<_>>().await?;
                    info!("Chunks -----------------------");

                    let bytes = chunks
                        .into_iter()
                        .map(|chunk| match chunk.result {
                            None => Err("Empty response".to_string().into()),
                            Some(download_ifs_response::Result::SuccessChunk(chunk)) => Ok(chunk),
                            Some(download_ifs_response::Result::Error(error)) => {
                                Err(GrpcError::Domain(error))
                            }
                        })
                        .collect::<Result<Vec<Vec<u8>>, GrpcError<ComponentError>>>()?;
                    info!("vbyte Chunks -----------------------");

                    let bytes = bytes.into_iter().flatten().collect::<Vec<u8>>();

                    info!("bytes collected  -----------------------");
                    record_external_call_response_size_bytes("components","download",bytes.len());
                    info!("{:?}", bytes);
                    Ok(bytes)

                })
            },
            is_grpc_retriable::<ComponentError>,
        )
            .await
            .map_err(|error|{
                tracing::error!("Failed to download ifs {component_id}@{component_version}: {error}");
                CompilationError::ComponentDownloadFailed(error.to_string())
            })
    }
}
