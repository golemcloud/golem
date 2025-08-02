use crate::services::worker_proxy::GetWorkerProject;
use crate::LastUniqueId;
use async_trait::async_trait;
use golem_common::model::{ProjectId, WorkerId};
use golem_worker_executor::services::worker_proxy::WorkerProxyError;
use std::sync::atomic::Ordering;
use std::sync::Arc;

#[derive(Clone)]
pub struct RegularExecutorTestContext {
    pub unique_id: u16,
    default_project_id: ProjectId,
}

impl RegularExecutorTestContext {
    pub fn new(last_unique_id: &LastUniqueId, default_project_id: &ProjectId) -> Self {
        let unique_id = last_unique_id.id.fetch_add(1, Ordering::Relaxed);
        Self {
            unique_id,
            default_project_id: default_project_id.clone(),
        }
    }

    pub fn redis_prefix(&self) -> String {
        format!("test-{}:", self.unique_id)
    }

    pub fn grpc_port(&self) -> u16 {
        9000 + (self.unique_id * 3)
    }

    pub fn http_port(&self) -> u16 {
        9001 + (self.unique_id * 3)
    }

    pub fn create_project_resolver(&self) -> Arc<dyn GetWorkerProject> {
        Arc::new(TestProjectResolver {
            default_project_id: self.default_project_id.clone(),
        })
    }
}

struct TestProjectResolver {
    default_project_id: ProjectId,
}

#[async_trait]
impl GetWorkerProject for TestProjectResolver {
    async fn get_worker_project(
        &self,
        _worker_id: &WorkerId,
    ) -> Result<ProjectId, WorkerProxyError> {
        Ok(self.default_project_id.clone())
    }
}
