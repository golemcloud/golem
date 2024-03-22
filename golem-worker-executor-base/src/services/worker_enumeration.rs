use crate::error::GolemError;
use crate::services::active_workers::ActiveWorkers;
use crate::services::shard::ShardService;
use crate::services::worker::{WorkerService, WorkerServiceInMemory, WorkerServiceRedis};
use crate::workerctx::WorkerCtx;
use async_trait::async_trait;
use golem_common::model::{TemplateId, WorkerMetadata, WorkerStatus};
use golem_common::redis::RedisPool;
use std::sync::Arc;

#[async_trait]
pub trait RunningWorkerEnumerationService {
    async fn get(&self, template_id: &TemplateId) -> Result<Vec<WorkerMetadata>, GolemError>;
}

#[derive(Clone)]
pub struct RunningWorkerEnumerationServiceDefault<Ctx: WorkerCtx> {
    active_workers: Arc<ActiveWorkers<Ctx>>,
}

#[async_trait]
impl<Ctx: WorkerCtx> RunningWorkerEnumerationService
    for crate::services::worker_enumeration::RunningWorkerEnumerationServiceDefault<Ctx>
{
    async fn get(&self, template_id: &TemplateId) -> Result<Vec<WorkerMetadata>, GolemError> {
        let active_workers = self.active_workers.enum_workers();

        let mut template_workers: Vec<WorkerMetadata> = vec![];
        for worker in active_workers {
            if worker.0.template_id == *template_id
                && worker.1.metadata.last_known_status.status == WorkerStatus::Running
            {
                template_workers.push(worker.1.metadata.clone());
            }
        }

        Ok(template_workers)
    }
}

impl<Ctx: WorkerCtx>
    crate::services::worker_enumeration::RunningWorkerEnumerationServiceDefault<Ctx>
{
    pub fn new(active_workers: Arc<ActiveWorkers<Ctx>>) -> Self {
        Self { active_workers }
    }
}

#[async_trait]
pub trait WorkerEnumerationService {
    async fn get(
        &self,
        template_id: &TemplateId,
        precise: bool,
    ) -> Result<Vec<WorkerMetadata>, GolemError>;
}

#[derive(Clone)]
pub struct WorkerEnumerationServiceRedis {
    redis: RedisPool,
    shard_service: Arc<dyn ShardService + Send + Sync>,
    worker_service: Arc<WorkerServiceRedis>,
}

impl crate::services::worker_enumeration::WorkerEnumerationServiceRedis {
    pub fn new(
        redis: RedisPool,
        shard_service: Arc<dyn ShardService + Send + Sync>,
        worker_service: Arc<WorkerServiceRedis>,
    ) -> Self {
        Self {
            redis,
            shard_service,
            worker_service,
        }
    }
}

#[async_trait]
impl WorkerEnumerationService
    for crate::services::worker_enumeration::WorkerEnumerationServiceRedis
{
    async fn get(
        &self,
        template_id: &TemplateId,
        precise: bool,
    ) -> Result<Vec<WorkerMetadata>, GolemError> {
        todo!()
    }
}

#[derive(Clone)]
pub struct WorkerEnumerationServiceInMemory {
    worker_service: Arc<WorkerServiceInMemory>,
}

impl crate::services::worker_enumeration::WorkerEnumerationServiceInMemory {
    pub fn new(worker_service: Arc<WorkerServiceInMemory>) -> Self {
        Self { worker_service }
    }
}

#[async_trait]
impl WorkerEnumerationService
    for crate::services::worker_enumeration::WorkerEnumerationServiceInMemory
{
    async fn get(
        &self,
        template_id: &TemplateId,
        precise: bool,
    ) -> Result<Vec<WorkerMetadata>, GolemError> {
        let workers = self.worker_service.enumerate().await;

        let mut template_workers: Vec<WorkerMetadata> = vec![];
        for worker in workers {
            if worker.worker_id.worker_id.template_id == *template_id {
                template_workers.push(worker);
            }
        }

        Ok(template_workers)
    }
}
