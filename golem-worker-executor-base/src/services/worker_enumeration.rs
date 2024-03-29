use crate::error::GolemError;
use crate::services::active_workers::ActiveWorkers;
use crate::services::golem_config::GolemConfig;
use crate::services::oplog::{OplogService, OplogServiceDefault};
use crate::services::worker::{WorkerService, WorkerServiceInMemory, WorkerServiceRedis};
use crate::services::{golem_config, HasConfig, HasOplogService, HasWorkerService};
use crate::worker::calculate_last_known_status;
use crate::workerctx::WorkerCtx;
use async_trait::async_trait;
use golem_common::model::{TemplateId, WorkerFilter, WorkerId, WorkerMetadata, WorkerStatus};
use golem_common::redis::RedisPool;
use std::sync::Arc;
use tracing::info;

#[async_trait]
pub trait RunningWorkerEnumerationService {
    async fn get(
        &self,
        template_id: &TemplateId,
        filter: Option<WorkerFilter>,
    ) -> Result<Vec<WorkerMetadata>, GolemError>;
}

#[derive(Clone)]
pub struct RunningWorkerEnumerationServiceDefault<Ctx: WorkerCtx> {
    active_workers: Arc<ActiveWorkers<Ctx>>,
}

#[async_trait]
impl<Ctx: WorkerCtx> RunningWorkerEnumerationService
    for crate::services::worker_enumeration::RunningWorkerEnumerationServiceDefault<Ctx>
{
    async fn get(
        &self,
        template_id: &TemplateId,
        filter: Option<WorkerFilter>,
    ) -> Result<Vec<WorkerMetadata>, GolemError> {
        info!(
            "Get workers for template: {}, filter: {}",
            template_id,
            filter.is_some()
        );

        let active_workers = self.active_workers.enum_workers();

        let mut template_workers: Vec<WorkerMetadata> = vec![];
        for (worker_id, worker) in active_workers {
            let metadata = worker.get_metadata();
            if worker_id.template_id == *template_id
                && (metadata.last_known_status.status == WorkerStatus::Running)
                && filter.clone().map_or(true, |f| f.matches(&metadata))
            {
                template_workers.push(metadata);
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

#[cfg(any(feature = "mocks", test))]
pub struct RunningWorkerEnumerationServiceMock {}

#[cfg(any(feature = "mocks", test))]
impl Default for crate::services::worker_enumeration::RunningWorkerEnumerationServiceMock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(feature = "mocks", test))]
impl crate::services::worker_enumeration::RunningWorkerEnumerationServiceMock {
    pub fn new() -> Self {
        Self {}
    }
}

#[cfg(any(feature = "mocks", test))]
#[async_trait]
impl RunningWorkerEnumerationService
    for crate::services::worker_enumeration::RunningWorkerEnumerationServiceMock
{
    async fn get(
        &self,
        _template_id: &TemplateId,
        _filter: Option<WorkerFilter>,
    ) -> Result<Vec<WorkerMetadata>, GolemError> {
        unimplemented!()
    }
}

#[async_trait]
pub trait WorkerEnumerationService {
    async fn get(
        &self,
        template_id: &TemplateId,
        filter: Option<WorkerFilter>,
        cursor: u64,
        count: u64,
        precise: bool,
    ) -> Result<(Option<u64>, Vec<WorkerMetadata>), GolemError>;
}

#[derive(Clone)]
pub struct WorkerEnumerationServiceRedis {
    redis: RedisPool,
    worker_service: Arc<WorkerServiceRedis>,
    oplog_service: Arc<OplogServiceDefault>,
    golem_config: Arc<golem_config::GolemConfig>,
}

impl crate::services::worker_enumeration::WorkerEnumerationServiceRedis {
    pub fn new(
        redis: RedisPool,
        worker_service: Arc<WorkerServiceRedis>,
        oplog_service: Arc<OplogServiceDefault>,
        golem_config: Arc<golem_config::GolemConfig>,
    ) -> Self {
        Self {
            redis,
            worker_service,
            oplog_service,
            golem_config,
        }
    }

    async fn get_internal(
        &self,
        template_id: &TemplateId,
        filter: Option<WorkerFilter>,
        cursor: u64,
        count: u64,
        precise: bool,
    ) -> Result<(Option<u64>, Vec<WorkerMetadata>), GolemError> {
        let mut new_cursor: Option<u64> = None;
        let mut template_workers: Vec<WorkerMetadata> = vec![];

        let template_worker_redis_key = get_template_worker_redis_key(template_id);

        let (new_redis_cursor, worker_redis_keys) = self
            .redis
            .with("instance", "scan")
            .scan(template_worker_redis_key, cursor, count)
            .await
            .map_err(|e| GolemError::unknown(e.details()))?;

        for worker_redis_key in worker_redis_keys {
            let worker_id = get_worker_id_from_redis_key(&worker_redis_key, template_id)?;
            let worker_metadata = self.worker_service.get(&worker_id).await;

            if let Some(worker_metadata) = worker_metadata {
                let metadata = if precise {
                    let last_known_status = calculate_last_known_status(
                        self,
                        &worker_id,
                        &Some(worker_metadata.clone()),
                    )
                    .await?;
                    WorkerMetadata {
                        last_known_status,
                        ..worker_metadata
                    }
                } else {
                    worker_metadata
                };

                if filter.clone().map_or(true, |f| f.matches(&metadata)) {
                    template_workers.push(metadata);
                }
            }
        }

        if new_redis_cursor > 0 {
            new_cursor = Some(new_redis_cursor);
        }

        Ok((new_cursor, template_workers))
    }
}

impl HasOplogService for WorkerEnumerationServiceRedis {
    fn oplog_service(&self) -> Arc<dyn OplogService + Send + Sync> {
        self.oplog_service.clone()
    }
}

impl HasWorkerService for WorkerEnumerationServiceRedis {
    fn worker_service(&self) -> Arc<dyn WorkerService + Send + Sync> {
        self.worker_service.clone()
    }
}

impl HasConfig for WorkerEnumerationServiceRedis {
    fn config(&self) -> Arc<GolemConfig> {
        self.golem_config.clone()
    }
}

#[async_trait]
impl WorkerEnumerationService
    for crate::services::worker_enumeration::WorkerEnumerationServiceRedis
{
    async fn get(
        &self,
        template_id: &TemplateId,
        filter: Option<WorkerFilter>,
        cursor: u64,
        count: u64,
        precise: bool,
    ) -> Result<(Option<u64>, Vec<WorkerMetadata>), GolemError> {
        info!(
            "Get workers for template: {}, filter: {}, cursor: {}, count: {}, precise: {}",
            template_id,
            filter.is_some(),
            cursor,
            count,
            precise
        );
        let mut new_cursor: Option<u64> = Some(cursor);
        let mut template_workers: Vec<WorkerMetadata> = vec![];

        while new_cursor.is_some() && (template_workers.len() as u64) < count {
            let new_count = count - (template_workers.len() as u64);

            let (next_cursor, workers) = self
                .get_internal(
                    template_id,
                    filter.clone(),
                    new_cursor.unwrap_or(0),
                    new_count,
                    precise,
                )
                .await?;

            template_workers.extend(workers);

            new_cursor = next_cursor;
        }

        Ok((new_cursor, template_workers))
    }
}

fn get_template_worker_redis_key(template_id: &TemplateId) -> String {
    format!("instance:instance:{}*", template_id.0)
}

fn get_worker_id_from_redis_key(
    worker_redis_key: &str,
    template_id: &TemplateId,
) -> Result<WorkerId, GolemError> {
    let template_prefix = format!("instance:instance:{}:", template_id.0);
    if worker_redis_key.starts_with(&template_prefix) {
        let worker_name = &worker_redis_key[template_prefix.len()..];
        Ok(WorkerId {
            worker_name: worker_name.to_string(),
            template_id: template_id.clone(),
        })
    } else {
        Err(GolemError::unknown(
            "Failed to get worker id from redis key",
        ))
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
        filter: Option<WorkerFilter>,
        cursor: u64,
        count: u64,
        _precise: bool,
    ) -> Result<(Option<u64>, Vec<WorkerMetadata>), GolemError> {
        let workers = self.worker_service.enumerate().await;

        let all_workers_count = workers.len() as u64;

        if all_workers_count > cursor {
            let mut template_workers: Vec<WorkerMetadata> = vec![];
            let mut index = 0;
            for worker in workers {
                if index >= cursor
                    && worker.worker_id.worker_id.template_id == *template_id
                    && filter.clone().map_or(true, |f| f.matches(&worker))
                {
                    template_workers.push(worker);
                }

                index += 1;

                if (template_workers.len() as u64) == count {
                    break;
                }
            }
            if index >= all_workers_count {
                Ok((None, template_workers))
            } else {
                Ok((Some(index), template_workers))
            }
        } else {
            Ok((None, vec![]))
        }
    }
}

#[cfg(any(feature = "mocks", test))]
pub struct WorkerEnumerationServiceMock {}

#[cfg(any(feature = "mocks", test))]
impl Default for crate::services::worker_enumeration::WorkerEnumerationServiceMock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(feature = "mocks", test))]
impl crate::services::worker_enumeration::WorkerEnumerationServiceMock {
    pub fn new() -> Self {
        Self {}
    }
}

#[cfg(any(feature = "mocks", test))]
#[async_trait]
impl WorkerEnumerationService
    for crate::services::worker_enumeration::WorkerEnumerationServiceMock
{
    async fn get(
        &self,
        _template_id: &TemplateId,
        _filter: Option<WorkerFilter>,
        _cursor: u64,
        _count: u64,
        _precise: bool,
    ) -> Result<(Option<u64>, Vec<WorkerMetadata>), GolemError> {
        unimplemented!()
    }
}
