use crate::service::worker::{WorkerError, WorkerService};
use async_trait::async_trait;
use cloud_common::auth::{CloudAuthCtx, CloudNamespace};
use cloud_common::model::TokenSecret;
use futures_util::TryStreamExt;
use golem_common::model::{HasAccountId, TargetWorkerId, WorkerId};
use golem_common::SafeDisplay;
use golem_service_base::model::Component;
use golem_service_base::service::initial_component_files::InitialComponentFilesService;
use golem_worker_service_base::gateway_execution::file_server_binding_handler::{
    FileServerBindingDetails, FileServerBindingError, FileServerBindingHandler,
    FileServerBindingResult, FileServerBindingSuccess,
};
use golem_worker_service_base::gateway_execution::WorkerDetails;
use golem_worker_service_base::service::component::ComponentService;
use golem_worker_service_base::service::worker::WorkerServiceError;
use rib::RibResult;
use std::sync::Arc;
use uuid::Uuid;

pub struct CloudFileServerBindingHandler {
    component_service: Arc<dyn ComponentService<CloudAuthCtx> + Send + Sync>,
    component_service_access_token: TokenSecret,
    initial_component_files_service: Arc<InitialComponentFilesService>,
    worker_service: Arc<dyn WorkerService + Send + Sync>,
}

impl CloudFileServerBindingHandler {
    pub fn new(
        component_service: Arc<dyn ComponentService<CloudAuthCtx> + Send + Sync>,
        component_service_access_token: Uuid,
        initial_component_files_service: Arc<InitialComponentFilesService>,
        worker_service: Arc<dyn WorkerService + Send + Sync>,
    ) -> Self {
        CloudFileServerBindingHandler {
            component_service,
            component_service_access_token: TokenSecret::new(component_service_access_token),
            initial_component_files_service,
            worker_service,
        }
    }

    async fn get_component_metadata(
        &self,
        worker_detail: &WorkerDetails,
        namespace: &CloudNamespace,
        auth_ctx: &CloudAuthCtx,
    ) -> Result<Component, FileServerBindingError> {
        // Two cases, we either have an existing worker or not (either not configured or not existing).
        // If there is no worker we need use the lastest component version, if there is none we need to use the exact component version
        // the worker is using. Not doing that would make the blob_storage optimization for read-only files visible to users.

        let worker_id = worker_detail.worker_name.as_ref().map(|wn| WorkerId {
            component_id: worker_detail.component_id.clone(),
            worker_name: wn.clone(),
        });

        let component_version = if let Some(worker_id) = worker_id {
            let worker_metadata = self
                .worker_service
                .get_metadata(&worker_id, namespace.clone())
                .await;
            match worker_metadata {
                Ok(metadata) => Some(metadata.component_version),
                Err(WorkerError::Base(WorkerServiceError::WorkerNotFound(_))) => None,
                Err(other) => Err(FileServerBindingError::InternalError(format!(
                    "Failed looking up worker metadata: {other}"
                )))?,
            }
        } else {
            None
        };

        let component_metadata = if let Some(component_version) = component_version {
            self.component_service
                .get_by_version(&worker_detail.component_id, component_version, auth_ctx)
                .await
                .map_err(FileServerBindingError::ComponentServiceError)?
        } else {
            self.component_service
                .get_latest(&worker_detail.component_id, auth_ctx)
                .await
                .map_err(FileServerBindingError::ComponentServiceError)?
        };

        Ok(component_metadata)
    }
}

#[async_trait]
impl FileServerBindingHandler<CloudNamespace> for CloudFileServerBindingHandler {
    // TODO: try to merge implementation with OSS
    async fn handle_file_server_binding_result(
        &self,
        namespace: &CloudNamespace,
        worker_detail: &WorkerDetails,
        original_result: RibResult,
    ) -> FileServerBindingResult {
        let binding_details = FileServerBindingDetails::from_rib_result(original_result)
            .map_err(FileServerBindingError::InternalError)?;

        let auth = CloudAuthCtx::new(self.component_service_access_token.clone());

        let component_metadata = self
            .get_component_metadata(worker_detail, namespace, &auth)
            .await?;

        // if we are serving a read_only file, we can just go straight to the blob storage.
        let matching_ro_file = component_metadata
            .files
            .iter()
            .find(|file| file.path == binding_details.file_path && file.is_read_only());

        if let Some(file) = matching_ro_file {
            let data = self
                .initial_component_files_service
                .get(&namespace.account_id(), &file.key)
                .await
                .map_err(|e| {
                    FileServerBindingError::InternalError(format!(
                        "Failed looking up file in storage: {e}"
                    ))
                })?
                .ok_or(FileServerBindingError::InternalError(format!(
                    "File not found in file storage: {}",
                    file.key
                )))
                .map(|stream| {
                    let mapped =
                        stream.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));
                    Box::pin(mapped)
                })?;

            Ok(FileServerBindingSuccess {
                binding_details,
                data,
            })
        } else {
            // Read write files need to be fetched from a running worker.
            // Ask the worker service to get the file contents. If no worker is running, one will be started.
            let worker_name_opt_validated = worker_detail
                .worker_name
                .as_ref()
                .map(|w| WorkerId::validate_worker_name(w).map(|_| w.clone()))
                .transpose()
                .map_err(|e| {
                    FileServerBindingError::InternalError(format!("Invalid worker name: {}", e))
                })?;

            let component_id = worker_detail.component_id.clone();

            let worker_id = TargetWorkerId {
                component_id,
                worker_name: worker_name_opt_validated.clone(),
            };

            let stream = self
                .worker_service
                .get_file_contents(
                    &worker_id,
                    binding_details.file_path.clone(),
                    namespace.clone(),
                )
                .await
                .map_err(|err| match err {
                    WorkerError::Base(err) => FileServerBindingError::WorkerServiceError(err),
                    WorkerError::Unauthorized(_) => {
                        FileServerBindingError::InternalError(err.to_safe_string())
                    }
                    WorkerError::Forbidden(_) => {
                        FileServerBindingError::InternalError(err.to_safe_string())
                    }
                    WorkerError::ProjectNotFound(_) => {
                        FileServerBindingError::InternalError(err.to_safe_string())
                    }
                    WorkerError::InternalAuthServiceError(_) => {
                        FileServerBindingError::InternalError(err.to_safe_string())
                    }
                })?;

            let stream =
                stream.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()));

            Ok(FileServerBindingSuccess {
                binding_details,
                data: Box::pin(stream),
            })
        }
    }
}
