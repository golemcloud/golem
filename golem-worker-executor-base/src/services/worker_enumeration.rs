use crate::error::GolemError;
use crate::services::active_workers::ActiveWorkers;
use crate::services::oplog::OplogServiceDefault;
use crate::services::worker::{WorkerService, WorkerServiceInMemory, WorkerServiceRedis};
use crate::workerctx::WorkerCtx;
use async_trait::async_trait;
use golem_common::model::{TemplateId, WorkerFilter, WorkerId, WorkerMetadata, WorkerStatus};
use golem_common::redis::RedisPool;
use std::sync::Arc;

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
        let active_workers = self.active_workers.enum_workers();

        let mut template_workers: Vec<WorkerMetadata> = vec![];
        for worker in active_workers {
            if worker.0.template_id == *template_id
                && worker.1.metadata.last_known_status.status == WorkerStatus::Running
                && filter
                    .clone()
                    .map_or(true, |f| f.matches(&worker.1.metadata))
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
        cursor: usize,
        count: usize,
        precise: bool,
    ) -> Result<(Option<usize>, Vec<WorkerMetadata>), GolemError>;
}

#[derive(Clone)]
pub struct WorkerEnumerationServiceRedis {
    redis: RedisPool,
    worker_service: Arc<WorkerServiceRedis>,
    oplog_service: Arc<OplogServiceDefault>,
}

impl crate::services::worker_enumeration::WorkerEnumerationServiceRedis {
    pub fn new(
        redis: RedisPool,
        worker_service: Arc<WorkerServiceRedis>,
        oplog_service: Arc<OplogServiceDefault>,
    ) -> Self {
        Self {
            redis,
            worker_service,
            oplog_service,
        }
    }

    async fn get_internal(
        &self,
        template_id: &TemplateId,
        filter: Option<WorkerFilter>,
        cursor: usize,
        count: usize,
        precise: bool,
    ) -> Result<(Option<usize>, Vec<WorkerMetadata>), GolemError> {
        // TODO implement precise
        let mut new_cursor: Option<usize> = None;
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
                if filter.clone().map_or(true, |f| f.matches(&worker_metadata)) {
                    template_workers.push(worker_metadata);
                }
            }
        }

        if new_redis_cursor > 0 {
            new_cursor = Some(new_redis_cursor);
        }

        Ok((new_cursor, template_workers))
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
        cursor: usize,
        count: usize,
        precise: bool,
    ) -> Result<(Option<usize>, Vec<WorkerMetadata>), GolemError> {
        let mut new_cursor: Option<usize> = Some(cursor);
        let mut template_workers: Vec<WorkerMetadata> = vec![];

        while new_cursor.is_some() && template_workers.len() < count {
            let new_count = template_workers.len() - count;

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
        cursor: usize,
        count: usize,
        _precise: bool,
    ) -> Result<(Option<usize>, Vec<WorkerMetadata>), GolemError> {
        let workers = self.worker_service.enumerate().await;

        let all_workers_count = workers.len();

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

                if template_workers.len() == count {
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
        _cursor: usize,
        _count: usize,
        _precise: bool,
    ) -> Result<(Option<usize>, Vec<WorkerMetadata>), GolemError> {
        unimplemented!()
    }
}
