use crate::error::GolemError;
use crate::services::active_workers::ActiveWorkers;
use crate::services::golem_config::GolemConfig;
use crate::services::oplog::OplogService;
use crate::services::worker::WorkerService;
use crate::services::{HasConfig, HasOplogService, HasWorkerService};
use crate::worker::calculate_last_known_status;
use crate::workerctx::WorkerCtx;
use async_trait::async_trait;
use golem_common::model::{
    AccountId, ComponentId, ScanCursor, WorkerFilter, WorkerMetadata, WorkerStatus,
};
use std::sync::Arc;
use tracing::info;

#[async_trait]
pub trait RunningWorkerEnumerationService {
    async fn get(
        &self,
        component_id: &ComponentId,
        filter: Option<WorkerFilter>,
    ) -> Result<Vec<WorkerMetadata>, GolemError>;
}

#[derive(Clone)]
pub struct RunningWorkerEnumerationServiceDefault<Ctx: WorkerCtx> {
    active_workers: Arc<ActiveWorkers<Ctx>>,
}

#[async_trait]
impl<Ctx: WorkerCtx> RunningWorkerEnumerationService
    for RunningWorkerEnumerationServiceDefault<Ctx>
{
    async fn get(
        &self,
        component_id: &ComponentId,
        filter: Option<WorkerFilter>,
    ) -> Result<Vec<WorkerMetadata>, GolemError> {
        info!(
            "Get workers for component: {}, filter: {}",
            component_id,
            filter
                .clone()
                .map(|f| f.to_string())
                .unwrap_or("N/A".to_string())
        );

        let active_workers = self.active_workers.enum_workers();

        let mut workers: Vec<WorkerMetadata> = vec![];
        for (worker_id, worker) in active_workers {
            let metadata = worker.get_metadata();
            if worker_id.component_id == *component_id
                && (metadata.last_known_status.status == WorkerStatus::Running)
                && filter.clone().map_or(true, |f| f.matches(&metadata))
            {
                workers.push(metadata);
            }
        }

        Ok(workers)
    }
}

impl<Ctx: WorkerCtx> RunningWorkerEnumerationServiceDefault<Ctx> {
    pub fn new(active_workers: Arc<ActiveWorkers<Ctx>>) -> Self {
        Self { active_workers }
    }
}

#[cfg(any(feature = "mocks", test))]
pub struct RunningWorkerEnumerationServiceMock {}

#[cfg(any(feature = "mocks", test))]
impl Default for RunningWorkerEnumerationServiceMock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(feature = "mocks", test))]
impl RunningWorkerEnumerationServiceMock {
    pub fn new() -> Self {
        Self {}
    }
}

#[cfg(any(feature = "mocks", test))]
#[async_trait]
impl RunningWorkerEnumerationService for RunningWorkerEnumerationServiceMock {
    async fn get(
        &self,
        _component_id: &ComponentId,
        _filter: Option<WorkerFilter>,
    ) -> Result<Vec<WorkerMetadata>, GolemError> {
        unimplemented!()
    }
}

#[async_trait]
pub trait WorkerEnumerationService {
    async fn get(
        &self,
        account_id: &AccountId,
        component_id: &ComponentId,
        filter: Option<WorkerFilter>,
        cursor: ScanCursor,
        count: u64,
        precise: bool,
    ) -> Result<(Option<ScanCursor>, Vec<WorkerMetadata>), GolemError>;
}

#[derive(Clone)]
pub struct DefaultWorkerEnumerationService {
    worker_service: Arc<dyn WorkerService + Send + Sync>,
    oplog_service: Arc<dyn OplogService + Send + Sync>,
    golem_config: Arc<GolemConfig>,
}

impl DefaultWorkerEnumerationService {
    pub fn new(
        worker_service: Arc<dyn WorkerService + Send + Sync>,
        oplog_service: Arc<dyn OplogService + Send + Sync>,
        golem_config: Arc<GolemConfig>,
    ) -> Self {
        Self {
            worker_service,
            oplog_service,
            golem_config,
        }
    }

    async fn get_internal(
        &self,
        account_id: &AccountId,
        component_id: &ComponentId,
        filter: Option<WorkerFilter>,
        cursor: ScanCursor,
        count: u64,
        precise: bool,
    ) -> Result<(Option<ScanCursor>, Vec<WorkerMetadata>), GolemError> {
        let mut new_cursor: Option<ScanCursor> = None;
        let mut workers: Vec<WorkerMetadata> = vec![];

        let (new_scan_cursor, keys) = self
            .oplog_service
            .scan_for_component(account_id, component_id, cursor, count)
            .await?;

        for owned_worker_id in keys {
            let worker_metadata = self.worker_service.get(&owned_worker_id).await;

            if let Some(worker_metadata) = worker_metadata {
                let metadata = if precise {
                    let last_known_status = calculate_last_known_status(
                        self,
                        &owned_worker_id,
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
                    workers.push(metadata);
                }
            }
        }

        if !new_scan_cursor.is_finished() {
            new_cursor = Some(new_scan_cursor);
        }

        Ok((new_cursor, workers))
    }
}

impl HasOplogService for DefaultWorkerEnumerationService {
    fn oplog_service(&self) -> Arc<dyn OplogService + Send + Sync> {
        self.oplog_service.clone()
    }
}

impl HasWorkerService for DefaultWorkerEnumerationService {
    fn worker_service(&self) -> Arc<dyn WorkerService + Send + Sync> {
        self.worker_service.clone()
    }
}

impl HasConfig for DefaultWorkerEnumerationService {
    fn config(&self) -> Arc<GolemConfig> {
        self.golem_config.clone()
    }
}

#[async_trait]
impl WorkerEnumerationService for DefaultWorkerEnumerationService {
    async fn get(
        &self,
        account_id: &AccountId,
        component_id: &ComponentId,
        filter: Option<WorkerFilter>,
        cursor: ScanCursor,
        count: u64,
        precise: bool,
    ) -> Result<(Option<ScanCursor>, Vec<WorkerMetadata>), GolemError> {
        info!(
            "Get workers for component: {}, filter: {}, cursor: {}, count: {}, precise: {}",
            component_id,
            filter
                .clone()
                .map(|f| f.to_string())
                .unwrap_or("N/A".to_string()),
            cursor,
            count,
            precise
        );
        let mut new_cursor: Option<ScanCursor> = Some(cursor);
        let mut workers: Vec<WorkerMetadata> = vec![];

        while new_cursor.is_some() && (workers.len() as u64) < count {
            let new_count = count - (workers.len() as u64);

            let (next_cursor, workers_page) = self
                .get_internal(
                    account_id,
                    component_id,
                    filter.clone(),
                    new_cursor.unwrap_or_default(),
                    new_count,
                    precise,
                )
                .await?;

            workers.extend(workers_page);

            new_cursor = next_cursor;
        }

        Ok((new_cursor, workers))
    }
}

#[cfg(any(feature = "mocks", test))]
pub struct WorkerEnumerationServiceMock {}

#[cfg(any(feature = "mocks", test))]
impl Default for WorkerEnumerationServiceMock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(feature = "mocks", test))]
impl WorkerEnumerationServiceMock {
    pub fn new() -> Self {
        Self {}
    }
}

#[cfg(any(feature = "mocks", test))]
#[async_trait]
impl WorkerEnumerationService for WorkerEnumerationServiceMock {
    async fn get(
        &self,
        _account_id: &AccountId,
        _component_id: &ComponentId,
        _filter: Option<WorkerFilter>,
        _cursor: ScanCursor,
        _count: u64,
        _precise: bool,
    ) -> Result<(Option<ScanCursor>, Vec<WorkerMetadata>), GolemError> {
        unimplemented!()
    }
}
