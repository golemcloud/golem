use crate::services::active_workers::ActiveWorkers;
use crate::services::golem_config::GolemConfig;
use crate::services::oplog::OplogService;
use crate::services::worker::WorkerService;
use crate::services::{HasConfig, HasOplogService, HasWorkerService};
use crate::worker::status::calculate_last_known_status_for_existing_worker;
use crate::workerctx::WorkerCtx;
use async_trait::async_trait;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::{ScanCursor, WorkerFilter, WorkerMetadata, WorkerStatus};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::sync::Arc;
use tracing::{info, Instrument};

#[async_trait]
pub trait RunningWorkerEnumerationService: Send + Sync {
    async fn get(
        &self,
        component_id: &ComponentId,
        filter: Option<WorkerFilter>,
    ) -> Result<Vec<WorkerMetadata>, WorkerExecutorError>;
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
    ) -> Result<Vec<WorkerMetadata>, WorkerExecutorError> {
        info!(
            "Get workers - filter: {}",
            filter
                .clone()
                .map(|f| f.to_string())
                .unwrap_or("N/A".to_string())
        );

        let active_workers = self.active_workers.snapshot().await;

        let mut workers: Vec<WorkerMetadata> = vec![];
        for (worker_id, worker) in active_workers {
            let metadata = worker.get_latest_worker_metadata().await;
            if worker_id.component_id == *component_id
                && (metadata.last_known_status.status == WorkerStatus::Running)
                && filter.clone().is_none_or(|f| f.matches(&metadata))
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

#[async_trait]
pub trait WorkerEnumerationService: Send + Sync {
    async fn get(
        &self,
        environment_id: &EnvironmentId,
        component_id: &ComponentId,
        filter: Option<WorkerFilter>,
        cursor: ScanCursor,
        count: u64,
        precise: bool,
    ) -> Result<(Option<ScanCursor>, Vec<WorkerMetadata>), WorkerExecutorError>;
}

#[derive(Clone)]
pub struct DefaultWorkerEnumerationService {
    worker_service: Arc<dyn WorkerService>,
    oplog_service: Arc<dyn OplogService>,
    golem_config: Arc<GolemConfig>,
}

impl DefaultWorkerEnumerationService {
    pub fn new(
        worker_service: Arc<dyn WorkerService>,
        oplog_service: Arc<dyn OplogService>,
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
        environment_id: &EnvironmentId,
        component_id: &ComponentId,
        filter: Option<WorkerFilter>,
        cursor: ScanCursor,
        count: u64,
        precise: bool,
    ) -> Result<(Option<ScanCursor>, Vec<WorkerMetadata>), WorkerExecutorError> {
        let mut workers: Vec<WorkerMetadata> = vec![];

        let (new_cursor, keys) = self
            .oplog_service
            .scan_for_component(environment_id, component_id, cursor, count)
            .instrument(tracing::info_span!("scan_for_component"))
            .await?;

        for owned_worker_id in keys {
            let worker_metadata = self
                .worker_service
                .get(&owned_worker_id)
                .instrument(tracing::info_span!("get_worker_metadata"))
                .await;

            if let Some(worker_metadata) = worker_metadata {
                let metadata = if precise {
                    let last_known_status = calculate_last_known_status_for_existing_worker(
                        self,
                        &owned_worker_id,
                        worker_metadata.last_known_status,
                    )
                    .instrument(tracing::info_span!("calculate_last_known_status"))
                    .await;

                    WorkerMetadata {
                        last_known_status,
                        ..worker_metadata.initial_worker_metadata
                    }
                } else {
                    WorkerMetadata {
                        last_known_status: worker_metadata.last_known_status.unwrap_or_default(),
                        ..worker_metadata.initial_worker_metadata
                    }
                };

                if filter.clone().is_none_or(|f| f.matches(&metadata)) {
                    workers.push(metadata);
                }
            }
        }

        Ok((new_cursor.into_option(), workers))
    }
}

impl HasOplogService for DefaultWorkerEnumerationService {
    fn oplog_service(&self) -> Arc<dyn OplogService> {
        self.oplog_service.clone()
    }
}

impl HasWorkerService for DefaultWorkerEnumerationService {
    fn worker_service(&self) -> Arc<dyn WorkerService> {
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
        environment_id: &EnvironmentId,
        component_id: &ComponentId,
        filter: Option<WorkerFilter>,
        cursor: ScanCursor,
        count: u64,
        precise: bool,
    ) -> Result<(Option<ScanCursor>, Vec<WorkerMetadata>), WorkerExecutorError> {
        info!(
            environment_id = %environment_id,
            component_id = %component_id,
            filter = filter
                .clone()
                .map(|f| f.to_string())
                .unwrap_or("N/A".to_string()),
            cursor = %cursor,
            count = %count,
            precise = %precise,
            "Enumerating workers"
        );
        let mut new_cursor: Option<ScanCursor> = Some(cursor);
        let mut workers: Vec<WorkerMetadata> = vec![];

        while new_cursor.is_some() && (workers.len() as u64) < count {
            let new_count = count - (workers.len() as u64);

            let (next_cursor, workers_page) = self
                .get_internal(
                    environment_id,
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
