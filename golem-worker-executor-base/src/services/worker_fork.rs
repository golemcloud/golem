use std::marker::PhantomData;
use std::sync::{Arc, RwLock};

use crate::error::GolemError;
use crate::metrics::workers::record_worker_call;
use crate::model::ExecutionStatus;
use crate::services::oplog::CommitLevel;
use crate::services::{HasAll, HasOplog};
use crate::worker::Worker;
use crate::workerctx::WorkerCtx;
use async_trait::async_trait;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::{
    AccountId, ComponentType, OwnedWorkerId, Timestamp, WorkerId, WorkerMetadata,
    WorkerStatusRecord,
};

#[async_trait]
pub trait WorkerFork {
    async fn fork(
        &self,
        source_worker_id: &OwnedWorkerId,
        target_worker_id: &WorkerId,
        oplog_index_cut_off: OplogIndex,
    ) -> Result<(), GolemError>;
}

#[derive(Clone)]
pub struct DefaultWorkerFork<Ctx: WorkerCtx, Svcs: HasAll<Ctx>> {
    all: Svcs,
    ctx: PhantomData<Ctx>,
}

impl<Ctx: WorkerCtx, Svcs: HasAll<Ctx>> DefaultWorkerFork<Ctx, Svcs> {
    pub fn new(all: Svcs) -> Self {
        Self {
            all,
            ctx: PhantomData,
        }
    }

    async fn validate_worker_forking(
        &self,
        account_id: &AccountId,
        source_worker_id: &WorkerId,
        target_worker_id: &WorkerId,
        oplog_index_cut_off: OplogIndex,
    ) -> Result<(OwnedWorkerId, OwnedWorkerId, WorkerMetadata), GolemError> {
        let second_index = OplogIndex::INITIAL.next();

        if oplog_index_cut_off < second_index {
            return Err(GolemError::invalid_request(
                "oplog_index_cut_off must be at least 2",
            ));
        }

        let owned_target_worker_id = OwnedWorkerId::new(account_id, target_worker_id);

        let target_metadata = self.all.worker_service().get(&owned_target_worker_id).await;

        // We allow forking only if the target worker does not exist
        if target_metadata.is_some() {
            return Err(GolemError::worker_already_exists(target_worker_id.clone()));
        }

        // We assume the source worker belongs to this executor
        self.all.shard_service().check_worker(source_worker_id)?;

        let owned_source_worker_id = OwnedWorkerId::new(account_id, source_worker_id);

        let metadata = self
            .all
            .worker_service()
            .get(&owned_source_worker_id)
            .await
            .ok_or(GolemError::worker_not_found(source_worker_id.clone()))?;

        Ok((owned_source_worker_id, owned_target_worker_id, metadata))
    }
}

#[async_trait]
impl<Ctx: WorkerCtx, Svcs: HasAll<Ctx> + Send + Sync + 'static> WorkerFork
    for DefaultWorkerFork<Ctx, Svcs>
{
    async fn fork(
        &self,
        source_worker_id: &OwnedWorkerId,
        target_worker_id: &WorkerId,
        oplog_index_cut_off: OplogIndex,
    ) -> Result<(), GolemError> {
        record_worker_call("fork");

        let (owned_source_worker_id, owned_target_worker_id, source_worker_metadata) = self
            .validate_worker_forking(
                &source_worker_id.account_id,
                &source_worker_id.worker_id,
                target_worker_id,
                oplog_index_cut_off,
            )
            .await?;

        let target_worker_id = owned_target_worker_id.worker_id.clone();
        let account_id = owned_target_worker_id.account_id.clone();

        // Not sure if we should copy the metadata or not, or stick on to just default
        let target_worker_metadata = WorkerMetadata {
            worker_id: target_worker_id.clone(),
            account_id,
            env: source_worker_metadata.env.clone(),
            args: source_worker_metadata.args.clone(),
            created_at: Timestamp::now_utc(),
            parent: source_worker_metadata.parent.clone(),
            last_known_status: WorkerStatusRecord::default(),
        };

        let source_worker_instance = Worker::get_or_create_suspended(
            &self.all,
            &owned_source_worker_id,
            Some(source_worker_metadata.args.clone()),
            Some(source_worker_metadata.env.clone()),
            None,
            None,
        )
        .await?;

        let source_oplog = source_worker_instance.oplog();

        let initial_oplog_entry = source_oplog.read(OplogIndex::INITIAL).await;

        // Update the oplog initial entry with the new worker
        let target_initial_oplog_entry = initial_oplog_entry
            .update_worker_id(&target_worker_id)
            .ok_or(GolemError::unknown(
                "Failed to update worker id in oplog entry",
            ))?;

        let new_oplog = self
            .all
            .oplog_service()
            .create(
                &owned_target_worker_id,
                target_initial_oplog_entry,
                target_worker_metadata,
                Arc::new(RwLock::new(ExecutionStatus::Suspended {
                    last_known_status: WorkerStatusRecord::default(), // default is idle, TODO: check if we need to update this or derive from the source last known status
                    component_type: ComponentType::Durable, // Probably forking should fail if component type is ephemeral, or not?
                    timestamp: Timestamp::now_utc(),
                })),
            )
            .await;

        let second_index = u64::from(OplogIndex::INITIAL.next());
        let cut_off_index = u64::from(oplog_index_cut_off);

        for index in second_index..=cut_off_index {
            let entry = source_oplog.read(OplogIndex::from_u64(index)).await;
            new_oplog.add(entry.clone()).await;
        }

        new_oplog.commit(CommitLevel::Always).await;

        // We go through worker proxy to resume the worker
        // as we need to make sure as it may live in another worker executor,
        // depending on sharding.
        // This will replay until the fork point in the forked worker
        self.all
            .worker_proxy()
            .resume(&target_worker_id)
            .await
            .map_err(|err| {
                GolemError::failed_to_resume_worker(target_worker_id.clone(), err.into())
            })?;

        Ok(())
    }
}
