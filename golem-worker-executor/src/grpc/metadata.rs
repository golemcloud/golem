// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::{
    AgentFilter, AgentMetadata, ComponentId, Cursor, EnvironmentId,
    GetRunningWorkersMetadataRequest, GetWorkersMetadataRequest, HasAll,
    HasRunningWorkerEnumerationService, HasWorkerEnumerationService, HashMap, Instrument,
    LastError, OplogErrorKind, ScanCursor, UpdateMode, UsesAllDeps, Worker, WorkerCtx,
    WorkerExecutorError, WorkerExecutorImpl, extract_owned_agent_id, golem, info_span,
    to_protobuf_resource_description,
};

impl<Ctx: WorkerCtx, Svcs: HasAll<Ctx> + UsesAllDeps<Ctx = Ctx> + Send + Sync + 'static>
    WorkerExecutorImpl<Ctx, Svcs>
{
    pub(super) async fn get_agent_metadata_internal(
        &self,
        request: golem::workerexecutor::v1::GetAgentMetadataRequest,
    ) -> Result<golem::worker::AgentMetadata, WorkerExecutorError> {
        let owned_agent_id =
            extract_owned_agent_id(&request, |r| &r.agent_id, |r| &r.environment_id)?;
        let owned_agent_id = self.canonicalize_owned_agent_id(&owned_agent_id).await?;
        self.ensure_worker_belongs_to_this_executor(&owned_agent_id)?;

        let metadata = Worker::<Ctx>::get_latest_metadata(self, &owned_agent_id)
            .await?
            .ok_or(WorkerExecutorError::worker_not_found(
                owned_agent_id.agent_id(),
            ))?;

        let last_error_and_retry_count = Ctx::get_last_error_and_retry_count(
            self,
            &owned_agent_id,
            metadata.agent_mode,
            &metadata.last_known_status,
        )
        .await;

        Self::create_proto_metadata(metadata, last_error_and_retry_count)
    }

    pub(super) async fn get_running_workers_metadata_internal(
        &self,
        request: GetRunningWorkersMetadataRequest,
    ) -> Result<Vec<golem::worker::AgentMetadata>, WorkerExecutorError> {
        let component_id: ComponentId = request
            .component_id
            .and_then(|t| t.try_into().ok())
            .ok_or(WorkerExecutorError::invalid_request("Invalid component id"))?;

        let filter: Option<AgentFilter> = match request.filter {
            Some(f) => Some(f.try_into().map_err(WorkerExecutorError::invalid_request)?),
            _ => None,
        };

        let workers = self
            .running_worker_enumeration_service()
            .get(&component_id, filter)
            .await?;

        let result: Vec<golem::worker::AgentMetadata> = workers
            .into_iter()
            .map(|worker_metadata| Self::create_proto_metadata(worker_metadata, None))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(result)
    }

    pub(super) async fn get_workers_metadata_internal(
        &self,
        request: GetWorkersMetadataRequest,
    ) -> Result<(Option<Cursor>, Vec<golem::worker::AgentMetadata>), WorkerExecutorError> {
        let component_id: ComponentId = request
            .component_id
            .and_then(|t| t.try_into().ok())
            .ok_or(WorkerExecutorError::invalid_request("Invalid component id"))?;

        let environment_id: EnvironmentId = request
            .environment_id
            .and_then(|t| t.try_into().ok())
            .ok_or(WorkerExecutorError::invalid_request("Invalid project id"))?;

        let filter: Option<AgentFilter> = match request.filter {
            Some(f) => Some(f.try_into().map_err(WorkerExecutorError::invalid_request)?),
            _ => None,
        };

        let (new_cursor, workers) = self
            .worker_enumeration_service()
            .get(
                &environment_id,
                &component_id,
                filter,
                request.cursor.map(ScanCursor::from).unwrap_or_default(),
                request.count,
                request.precise,
            )
            .instrument(info_span!("enumerate_workers"))
            .await?;

        let mut result = Vec::new();

        for worker_metadata in workers {
            let last_error_and_retry_count = Ctx::get_last_error_and_retry_count(
                self,
                &worker_metadata.owned_agent_id(),
                worker_metadata.agent_mode,
                &worker_metadata.last_known_status,
            )
            .await;
            let metadata =
                Self::create_proto_metadata(worker_metadata, last_error_and_retry_count)?;
            result.push(metadata);
        }

        Ok((new_cursor.map(Cursor::from), result))
    }

    fn create_proto_metadata(
        metadata: AgentMetadata,
        last_error_and_retry_count: Option<LastError>,
    ) -> Result<golem::worker::AgentMetadata, WorkerExecutorError> {
        let update_metadata = |pending: Option<&golem_common::model::PendingUpdateRef>| {
            use golem_common::model::PendingUpdateKind;

            let Some(pending) = pending else {
                return (None, UpdateMode::Manual as i32, None);
            };
            let (mode, assisted) = match &pending.kind {
                PendingUpdateKind::Automatic => (UpdateMode::Automatic, None),
                PendingUpdateKind::SnapshotBased { .. } => (UpdateMode::Manual, None),
                PendingUpdateKind::SnapshotAssistedAutomatic(selection) => (
                    UpdateMode::Automatic,
                    Some(golem::worker::SnapshotAssistedUpdateMetadata {
                        source_component_revision: selection.snapshot.component_revision.into(),
                        source_revision_start_index: selection.source_revision_start_index.into(),
                        snapshot_index: selection.snapshot.index.into(),
                        snapshot_revision: selection.snapshot.component_revision.into(),
                        filesystem_snapshot: selection
                            .snapshot
                            .filesystem_snapshot
                            .clone()
                            .map(String::from),
                    }),
                ),
            };
            (Some(pending.admission_index.into()), mode as i32, assisted)
        };

        let mut updates = Vec::new();

        let latest_status = metadata.last_known_status;
        for pending_invocation in &latest_status.pending_invocations {
            if let Some(target_revision) = pending_invocation.manual_update_target_revision {
                updates.push(golem::worker::UpdateRecord {
                    timestamp: Some(pending_invocation.timestamp.into()),
                    target_revision: target_revision.into(),
                    update: Some(golem::worker::update_record::Update::Pending(
                        golem::worker::PendingUpdate {},
                    )),
                    pending_update_index: Some(pending_invocation.oplog_index.into()),
                    mode: UpdateMode::Manual as i32,
                    snapshot_assisted_details: None,
                });
            }
        }
        for pending_update in &latest_status.pending_updates {
            let (pending_update_index, mode, snapshot_assisted_details) =
                update_metadata(Some(pending_update));
            updates.push(golem::worker::UpdateRecord {
                timestamp: Some(pending_update.timestamp.into()),
                target_revision: pending_update.target_revision.into(),
                update: Some(golem::worker::update_record::Update::Pending(
                    golem::worker::PendingUpdate {},
                )),
                pending_update_index,
                mode,
                snapshot_assisted_details,
            });
        }
        for successful_update in &latest_status.successful_updates {
            let (pending_update_index, mode, snapshot_assisted_details) =
                update_metadata(successful_update.pending_update.as_ref());
            updates.push(golem::worker::UpdateRecord {
                timestamp: Some(successful_update.timestamp.into()),
                target_revision: successful_update.target_revision.into(),
                update: Some(golem::worker::update_record::Update::Successful(
                    golem::worker::SuccessfulUpdate {},
                )),
                pending_update_index,
                mode,
                snapshot_assisted_details,
            });
        }
        for failed_update in &latest_status.failed_updates {
            let (pending_update_index, mode, snapshot_assisted_details) =
                update_metadata(failed_update.pending_update.as_ref());
            updates.push(golem::worker::UpdateRecord {
                timestamp: Some(failed_update.timestamp.into()),
                target_revision: failed_update.target_revision.into(),
                update: Some(golem::worker::update_record::Update::Failed(
                    golem::worker::FailedUpdate {
                        details: failed_update.details.clone(),
                    },
                )),
                pending_update_index,
                mode,
                snapshot_assisted_details,
            });
        }
        updates.sort_by_key(|record| {
            record.timestamp.as_ref().unwrap().seconds * 1_000_000_000
                + record.timestamp.as_ref().unwrap().nanos as i64
        });

        let mut owned_resources = Vec::new();
        for (resource_id, resource) in latest_status.owned_resources {
            owned_resources.push(to_protobuf_resource_description(resource_id, resource));
        }

        let active_plugins = latest_status.active_plugins;

        Ok(golem::worker::AgentMetadata {
            agent_id: Some(metadata.agent_id.into()),
            owner_kind: metadata.owner_kind.into(),
            environment_id: Some(metadata.environment_id.into()),
            env: HashMap::from_iter(metadata.env.iter().cloned()),
            config: metadata
                .config
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, String>>()
                .map_err(|error| {
                    WorkerExecutorError::unknown(format!(
                        "failed converting agent configuration: {error}"
                    ))
                })?,
            created_by: Some(metadata.created_by.into()),
            component_revision: latest_status.component_revision.into(),
            status: Into::<golem::worker::AgentStatus>::into(latest_status.status).into(),
            retry_count: last_error_and_retry_count
                .as_ref()
                .and_then(|last_error| {
                    latest_status
                        .current_retry_state
                        .get(&last_error.retry_from)
                        .map(|s| s.retry_count())
                })
                .unwrap_or_default(),
            pending_invocation_count: latest_status.pending_invocations.len() as u64,
            updates,
            created_at: Some(metadata.created_at.into()),
            last_error: last_error_and_retry_count
                .map(|last_error| last_error.error.to_string(&last_error.stderr)),
            last_error_kind: latest_status.last_error_kind.map(|kind| match kind {
                OplogErrorKind::Invocation => golem::worker::OplogErrorKind::Invocation as i32,
                OplogErrorKind::Recovery => golem::worker::OplogErrorKind::Recovery as i32,
            }),
            component_size: latest_status.component_size,
            total_linear_memory_size: latest_status.total_linear_memory_size,
            owned_resources,
            active_plugins: active_plugins.into_iter().map(|id| id.into()).collect(),
            skipped_regions: latest_status
                .skipped_regions
                .into_regions()
                .map(|region| region.into())
                .collect(),
            deleted_regions: latest_status
                .deleted_regions
                .into_regions()
                .map(|region| region.into())
                .collect(),
            oplog_idx: u64::from(latest_status.oplog_idx),
            fingerprint: Some(metadata.fingerprint.0.into()),
        })
    }
}
