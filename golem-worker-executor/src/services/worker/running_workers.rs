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

use super::*;

/// Why a recovery-index member is stale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StaleReason {
    /// The agent has no oplog.
    Absent,
    /// The oplog of the agent belongs to another incarnation.
    FingerprintMismatch,
}

impl StaleReason {
    /// The label of the stale-member metric.
    fn label(self) -> &'static str {
        match self {
            Self::Absent => "absent",
            Self::FingerprintMismatch => "fingerprint_mismatch",
        }
    }
}

/// Logs that recovery leaves the agent as it is, because its oplog does not read. The member stays
/// in the recovery index, and the oplog stays as it is, so an operator can inspect it.
fn skip_unreadable_agent(owned_agent_id: &OwnedAgentId, error: &WorkerExecutorError) {
    error!(
        agent_id = %owned_agent_id,
        error = %error,
        "Skipping agent during recovery: its oplog is unreadable"
    );
}

/// Tells why the member of the incarnation `fingerprint` is stale, when `identity` is the stored
/// identity of its agent: no identity, or an identity with another fingerprint. `None` means that
/// the member is the stored incarnation.
pub(super) fn stale_reason(
    identity: Option<&ResolvedAgentIdentity>,
    fingerprint: AgentFingerprint,
) -> Option<StaleReason> {
    match identity {
        None => Some(StaleReason::Absent),
        Some(identity) if identity.fingerprint != fingerprint => {
            Some(StaleReason::FingerprintMismatch)
        }
        Some(_) => None,
    }
}

impl DefaultWorkerService {
    pub(super) async fn enum_workers_at_key(
        &self,
        key: &str,
        on_stale: OnStale<'_>,
    ) -> Result<Vec<GetWorkerMetadataResult>, WorkerExecutorError> {
        record_worker_call("enum");

        // The index itself is not per-worker: without it there is no list of workers to recover,
        // so this failure is reported to the caller rather than silently yielding an empty shard.
        let value: Vec<RunningWorker> = self
            .key_value_storage
            .with_entity("worker", "enum", "agent_id")
            .members_of_set(KeyValueStorageNamespace::RunningWorkers, key)
            .await
            .map_err(|err| {
                WorkerExecutorError::runtime(format!(
                    "failed to get worker ids from KV storage: {err}"
                ))
            })?;

        let mut workers = Vec::new();

        for running_worker in value {
            let owned_agent_id = &running_worker.owned_agent_id;
            // Only a worker that is genuinely gone may be skipped. The two outcomes are not the
            // same failure wearing different clothes:
            //
            // - `Ok(None)` means there is no oplog, so the index entry outlived the worker - a
            //   delete that raced this read. There is nothing to resume, and skipping is correct.
            // - `Err` means the entry may well exist and we simply could not read it. Skipping
            //   would leave a running agent suspended for as long as this executor owns the shard,
            //   which breaks the guarantee that a reshard or a crash does not stop a running
            //   worker - and it would do so silently. Failing the scan is the safe answer: a
            //   transient cause is already absorbed by the storage retry budget, and one that
            //   outlives it leaves this executor unable to do its job, so replacing it beats
            //   carrying on with an unknown number of agents stranded.
            //
            // An oplog that does not read is the exception: no retry reads it, so failing the scan
            // would strand every other agent of the shards behind it. That agent is left as it is.
            let identity = match self.resolve_agent_identity(owned_agent_id).await {
                Ok(identity) => identity,
                Err(error @ WorkerExecutorError::OplogFailed { .. }) => {
                    skip_unreadable_agent(owned_agent_id, &error);
                    continue;
                }
                Err(error) => return Err(error),
            };
            if let Some(reason) = stale_reason(identity.as_ref(), running_worker.fingerprint) {
                self.drop_stale_member(key, &running_worker, reason, on_stale)
                    .await?;
                debug!("Skipping {owned_agent_id} during recovery: stale recovery-index member");
                continue;
            }

            match self.get(owned_agent_id).await {
                Ok(Some(metadata))
                    if metadata.initial_worker_metadata.fingerprint
                        == running_worker.fingerprint =>
                {
                    workers.push(metadata)
                }
                // The metadata names another incarnation: the same comparison of fingerprints as
                // `stale_reason`, made on the metadata.
                Ok(Some(_)) => {
                    self.drop_stale_member(
                        key,
                        &running_worker,
                        StaleReason::FingerprintMismatch,
                        on_stale,
                    )
                    .await?;
                    debug!(
                        "Skipping {owned_agent_id} during recovery: stale recovery-index member"
                    );
                }
                Ok(None) => {
                    let current = self.resolve_agent_identity(owned_agent_id).await?;
                    if let Some(reason) = stale_reason(current.as_ref(), running_worker.fingerprint)
                    {
                        self.drop_stale_member(key, &running_worker, reason, on_stale)
                            .await?;
                    } else {
                        return Err(WorkerExecutorError::runtime(format!(
                            "failed to load metadata for existing {owned_agent_id} during recovery"
                        )));
                    }
                }
                Err(error @ WorkerExecutorError::OplogFailed { .. }) => {
                    skip_unreadable_agent(owned_agent_id, &error);
                }
                Err(err) => {
                    return Err(WorkerExecutorError::runtime(format!(
                        "failed to read {owned_agent_id} during recovery, so it cannot be resumed: {err}"
                    )));
                }
            }
        }

        Ok(workers)
    }

    /// Requests the delete of the snapshots of the stale member, then removes it from the index.
    /// A removal that fails leaves the member, so the next scan requests the delete again.
    pub(super) async fn drop_stale_member(
        &self,
        key: &str,
        running_worker: &RunningWorker,
        reason: StaleReason,
        on_stale: OnStale<'_>,
    ) -> Result<(), WorkerExecutorError> {
        on_stale(&running_worker.owned_agent_id, running_worker.fingerprint);
        self.remove_running_worker_member(key, running_worker)
            .await?;
        record_stale_running_worker(reason.label());
        Ok(())
    }

    pub(super) async fn remove_running_worker_member(
        &self,
        key: &str,
        running_worker: &RunningWorker,
    ) -> Result<(), WorkerExecutorError> {
        self.key_value_storage
            .with_entity("worker", "remove_stale", "agent_id")
            .remove_from_set(
                KeyValueStorageNamespace::RunningWorkers,
                key,
                running_worker,
            )
            .await
            .map_err(|err| {
                WorkerExecutorError::runtime(format!(
                    "failed to remove stale worker from the recovery index: {err}"
                ))
            })
    }

    pub(super) fn running_in_shard_key(shard_id: &ShardId) -> String {
        format!("worker:running_in_shard:{shard_id}")
    }

    pub(crate) fn should_track_for_assignment_recovery(status: &AgentStatusRecord) -> bool {
        matches!(
            status.status,
            AgentStatus::Running | AgentStatus::Retrying | AgentStatus::Interrupted
        ) || status.has_pending_work()
            || !status.pending_durable_stream_cancellations.is_empty()
    }
}
