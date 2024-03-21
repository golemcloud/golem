use golem_common::model::AccountId;
use golem_service_base::service::auth::{AuthError, AuthService};
use golem_worker_service_base::{
    auth::{CommonNamespace, EmptyAuthCtx},
    service::worker::{Metadata, NamespaceMetadata, TemplatePermission},
};

use async_trait::async_trait;

pub type WorkerServiceDefault =
    golem_worker_service_base::service::worker::WorkerServiceDefault<EmptyAuthCtx, WorkerNamespace>;

#[derive(Clone, Copy, Debug)]
pub struct NoopWorkerAuthService {}

#[async_trait]
impl AuthService<EmptyAuthCtx, WorkerNamespace, TemplatePermission> for NoopWorkerAuthService {
    async fn is_authorized(
        &self,
        _: TemplatePermission,
        _: &EmptyAuthCtx,
    ) -> Result<WorkerNamespace, AuthError> {
        Ok(WorkerNamespace(CommonNamespace::default()))
    }
}

#[derive(Clone, Default)]
pub struct WorkerNamespace(pub CommonNamespace);

#[async_trait]
impl Metadata for WorkerNamespace {
    async fn get_metadata(&self) -> anyhow::Result<NamespaceMetadata> {
        let metadata = NamespaceMetadata {
            account_id: Some(AccountId {
                value: "-1".to_string(),
            }),
            limits: None,
        };

        Ok(metadata)
    }
}
