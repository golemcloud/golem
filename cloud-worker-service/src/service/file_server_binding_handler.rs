use crate::service::worker::{WorkerError, WorkerService};
use async_trait::async_trait;
use cloud_common::auth::{CloudAuthCtx, CloudNamespace};
use cloud_common::model::TokenSecret;
use futures_util::TryStreamExt;
use golem_common::model::{HasAccountId, TargetWorkerId};
use golem_common::SafeDisplay;
use golem_service_base::model::validate_worker_name;
use golem_service_base::service::initial_component_files::InitialComponentFilesService;
use golem_worker_service_base::gateway_execution::file_server_binding_handler::{
    FileServerBindingDetails, FileServerBindingError, FileServerBindingHandler,
    FileServerBindingResult, FileServerBindingSuccess,
};
use golem_worker_service_base::gateway_execution::gateway_binding_resolver::WorkerDetail;
use golem_worker_service_base::service::component::ComponentService;
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
}

#[async_trait]
impl FileServerBindingHandler<CloudNamespace> for CloudFileServerBindingHandler {
    // TODO: try to merge implementation with OSS
    async fn handle_file_server_binding_result(
        &self,
        namespace: &CloudNamespace,
        worker_detail: &WorkerDetail,
        original_result: RibResult,
    ) -> FileServerBindingResult {
        let binding_details = FileServerBindingDetails::from_rib_result(original_result)
            .map_err(FileServerBindingError::InternalError)?;

        let auth = CloudAuthCtx::new(self.component_service_access_token.clone());
        let component_metadata = self
            .component_service
            .get_by_version(
                &worker_detail.component_id.component_id,
                worker_detail.component_id.version,
                &auth,
            )
            .await
            .map_err(FileServerBindingError::ComponentServiceError)?;

        // if we are serving a read_only file, we can just go straight to the blob storage.
        let matching_file = component_metadata
            .files
            .iter()
            .find(|file| file.path == binding_details.file_path && file.is_read_only());

        if let Some(file) = matching_file {
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
                .map(|w| validate_worker_name(w).map(|_| w.clone()))
                .transpose()
                .map_err(|e| {
                    FileServerBindingError::InternalError(format!("Invalid worker name: {}", e))
                })?;

            let component_id = worker_detail.component_id.component_id.clone();

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
