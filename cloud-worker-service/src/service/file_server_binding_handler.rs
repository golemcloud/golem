use crate::service::worker::{WorkerError, WorkerService};
use bytes::Bytes;
use cloud_common::auth::CloudNamespace;
use futures::stream::BoxStream;
use golem_common::model::{ComponentFilePath, TargetWorkerId, WorkerId};
use golem_common::SafeDisplay;
use golem_worker_service_base::gateway_execution::file_server_binding_handler::{
    FileServerBindingError, WorkerServiceAdapter,
};
use golem_worker_service_base::service::worker::{WorkerResult, WorkerServiceError};
use std::sync::Arc;

pub struct CloudWorkerServiceAdapter {
    worker_service: Arc<dyn WorkerService + Send + Sync>,
}

impl CloudWorkerServiceAdapter {
    pub fn new(worker_service: Arc<dyn WorkerService + Send + Sync>) -> Self {
        Self { worker_service }
    }
}

#[async_trait::async_trait]
impl WorkerServiceAdapter<CloudNamespace> for CloudWorkerServiceAdapter {
    async fn get_worker_version(
        &self,
        worker_id: &WorkerId,
        namespace: &CloudNamespace,
    ) -> Result<Option<u64>, FileServerBindingError> {
        let worker_metadata = self
            .worker_service
            .get_metadata(worker_id, namespace.clone())
            .await;
        let version = match worker_metadata {
            Ok(metadata) => Some(metadata.component_version),
            Err(WorkerError::Base(WorkerServiceError::WorkerNotFound(_))) => None,
            Err(other) => Err(FileServerBindingError::InternalError(format!(
                "Failed looking up worker metadata: {other}"
            )))?,
        };
        Ok(version)
    }

    async fn get_file_contents(
        &self,
        worker_id: &TargetWorkerId,
        path: ComponentFilePath,
        namespace: &CloudNamespace,
    ) -> Result<BoxStream<'static, WorkerResult<Bytes>>, FileServerBindingError> {
        self.worker_service
            .get_file_contents(worker_id, path, namespace.clone())
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
            })
    }
}
