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

use crate::model::TrapType;
use crate::services::golem_config::GolemConfig;
use crate::services::resource_limits::{MonthlyResourceCapacityError, MonthlyResourceExhaustion};
use crate::workerctx::WorkerCtx;
use golem_common::model::OplogIndex;
use golem_common::model::agent::AgentMode;
use golem_common::model::oplog::{AgentError, EphemeralCannotSuspendError};
use golem_service_base::error::worker_executor::{InterruptKind, WorkerExecutorError};

#[cfg(test)]
mod tests;

pub(super) fn monthly_failure<Ctx: WorkerCtx>(
    trap: &TrapType,
    mode: AgentMode,
    accepted: &Option<(InterruptKind, MonthlyResourceExhaustion)>,
    config: &GolemConfig,
    policy_unavailable: bool,
    retry_from: OplogIndex,
) -> Option<TrapType> {
    if mode == AgentMode::Ephemeral
        && policy_unavailable
        && matches!(trap, TrapType::Interrupt(InterruptKind::Suspend(_)))
    {
        return Some(TrapType::Error {
            error: AgentError::EphemeralCannotSuspend(EphemeralCannotSuspendError {
                reason: MonthlyResourceCapacityError::UNAVAILABLE_REASON.to_string(),
            }),
            retry_from,
            semantic_trap_retry_override: None,
        });
    }
    match trap {
        TrapType::Interrupt(kind) => {
            monthly_interrupt_error(*kind, mode, accepted, config).map(|error| {
                TrapType::from_worker_executor_error::<Ctx>(error, OplogIndex::INITIAL, mode)
            })
        }
        _ => None,
    }
}

pub(super) fn monthly_interrupt_error(
    kind: InterruptKind,
    mode: AgentMode,
    accepted: &Option<(InterruptKind, MonthlyResourceExhaustion)>,
    config: &GolemConfig,
) -> Option<WorkerExecutorError> {
    if mode != AgentMode::Ephemeral {
        return None;
    }
    accepted.as_ref().and_then(|(selected, exhaustion)| {
        (*selected == kind).then(|| {
            super::invocation_loop::monthly_resource_exhausted_invocation_error(config, *exhaustion)
        })
    })
}

use crate::worker::*;

impl<Ctx: WorkerCtx> Worker<Ctx> {
    pub(crate) async fn record_recovery_failure(&self, error: &WorkerExecutorError) {
        // A recovery that failed because the shard moved is recorded by nobody: the agent retires
        // here and is recovered by the shard's new owner. The caller stops the worker right after
        // this, and that stop is where the agent is dropped.
        if self.retire_if_shard_lost(error) {
            return;
        }

        let latest_status = self.get_last_known_status().await;
        let previous_error = if latest_status.last_error_kind == Some(OplogErrorKind::Recovery) {
            Ctx::get_last_error_and_retry_count(
                self.all(),
                &self.owned_agent_id,
                self.agent_mode(),
                &latest_status,
            )
            .await
        } else {
            None
        };
        let retry_from = recovery_retry_from(error)
            .or_else(|| previous_error.as_ref().map(|error| error.retry_from))
            .unwrap_or(self.oplog.current_oplog_index().await);
        let infrastructure_failure = is_infrastructure_recovery_error(error);
        let error = recovery_agent_error(error);
        let retry_policy_state = (!infrastructure_failure && error != AgentError::OutOfMemory)
            .then_some(RetryPolicyState::Terminal);
        // A refusal records the lost shard, which the caller's stop acts on.
        let _ = self
            .add_and_commit_oplog(OplogEntry::error(
                None,
                OplogErrorKind::Recovery,
                error,
                retry_from,
                retry_policy_state,
            ))
            .await;
    }
    pub(crate) async fn await_invocation_result(
        self: Arc<Self>,
        idempotency_key: IdempotencyKey,
        result: ResultOrSubscription,
    ) -> Result<AgentInvocationOutput, WorkerExecutorError> {
        match result {
            ResultOrSubscription::Finished(Ok(output)) => Ok(output),
            ResultOrSubscription::Finished(Err(err)) => Err(err),
            ResultOrSubscription::Pending(subscription) => {
                // Cache miss / non-read-only path: ensure the wasmtime instance is
                // running so the queued invocation can be processed. The
                // `ResultOrSubscription::Finished` arm above short-circuits before
                // this, which is exactly what makes a read-only cache hit avoid
                // any agent loading.
                Worker::start_if_needed(self.clone()).await?;

                debug!("Waiting for idempotency key to complete",);

                // The caller's honest share of the work: the execution runs in the
                // worker's own trace, linked from here, and only its outcome comes
                // back. This span measures the waiting, which is what the request
                // actually spent its time on.

                let result = async {
                    self.wait_for_invocation_result(&idempotency_key, subscription)
                        .await
                }
                .instrument(span!(
                    Level::INFO,
                    "wait_for_invocation_result",
                    agent_id = %self.owned_agent_id.agent_id,
                    idempotency_key = %idempotency_key,
                ))
                .await;

                match result {
                    Ok(LookupResult::Complete(Ok(output))) => Ok(output),
                    Ok(LookupResult::Complete(Err(err))) => Err(err),
                    Ok(LookupResult::Interrupted) => {
                        Err(InterruptKind::Interrupt(Timestamp::now_utc()).into())
                    }
                    Ok(LookupResult::Pending) => Err(WorkerExecutorError::unknown(
                        "Unexpected pending result after invoke",
                    )),
                    Ok(LookupResult::New) => Err(WorkerExecutorError::unknown(
                        "Unexpected missing result after invoke",
                    )),
                    Err(recv_error) => Err(WorkerExecutorError::unknown(format!(
                        "Failed waiting for invocation result: {recv_error}"
                    ))),
                }
            }
        }
    }
    // should only be called from invocation loop
    pub async fn store_invocation_success(
        &self,
        key: &IdempotencyKey,
        output: AgentInvocationOutput,
    ) {
        let cached = InvocationResult::Cached {
            result: Ok(output.clone()),
        };
        let status = self.last_known_status.load_full();
        let mut map = self.hydrated_invocation_results.write().await;
        map.release_projected_completions(&status, self.last_known_status_detached());
        map.insert_unprojected_completion(
            key.clone(),
            cached,
            status.invocation_results.revert_generation(),
            output.oplog_index.unwrap_or(status.oplog_idx),
        );
        // `drop` before taking `origins`: `fail_pending_invocations` locks
        // origins -> hydrated_invocation_results, so holding `map` here would invert that
        // order and can deadlock. Not a scope tidy-up.
        drop(map);
        self.external_invocation_origins.write().await.remove(key);
        debug!("Stored invocation success for {key}");
        self.publish_completion(key, Ok(output));
    }
    pub(in crate::worker) fn publish_completion(
        &self,
        key: &IdempotencyKey,
        result: Result<AgentInvocationOutput, WorkerExecutorError>,
    ) {
        self.events().publish(Event::InvocationCompleted {
            agent_id: self.owned_agent_id.agent_id(),
            idempotency_key: key.clone(),
            result: Box::new(result),
        });
    }
    // Called by the invocation loop, or by the retiring owner after the loop has stopped.
    pub async fn store_invocation_failure(&self, key: &IdempotencyKey, trap_type: &TrapType) {
        let status = self.last_known_status.load_full();
        let keys_to_fail =
            invocation_keys_to_fail(&status, Some(key), !trap_type.is_invocation_rejection());
        let stderr = self.worker_event_service.get_last_invocation_errors();
        let golem_error = trap_type.as_golem_error(&stderr);
        // A lost shard is not the invocation's outcome: the new owner resumes it. The waiters are
        // sent there, and nothing is cached that a later poller here would read as its result.
        if matches!(trap_type, TrapType::Interrupt(InterruptKind::ShardLost)) {
            for key in &keys_to_fail {
                self.publish_completion(key, Err(WorkerExecutorError::ShardingNotReady));
            }
            return;
        }
        let mut map = self.hydrated_invocation_results.write().await;
        // Co-pending fail-fast results exist only in this bounded warm cache. Once evicted, a
        // poller sees the same `Pending` state that reconstructing this status from the oplog does.
        for key in &keys_to_fail {
            map.insert(
                key.clone(),
                InvocationResult::Cached {
                    result: Err(FailedInvocationResult {
                        trap_type: trap_type.clone(),
                        stderr: stderr.clone(),
                    }),
                },
                status.invocation_results.revert_generation(),
                status.oplog_idx,
            );
            if let Some(golem_error) = &golem_error {
                self.publish_completion(key, Err(golem_error.clone()));
            }
        }
        // See `store_invocation_success`: origins must not be taken while
        // `hydrated_invocation_results` is held.
        drop(map);
        let mut origins = self.external_invocation_origins.write().await;
        for key in &keys_to_fail {
            origins.remove(key);
        }
    }
    pub(crate) async fn store_invocation_resuming(&self, key: &IdempotencyKey) {
        let mut map = self.hydrated_invocation_results.write().await;
        map.remove(key);
    }
    pub(in crate::worker) async fn wait_for_invocation_result(
        &self,
        key: &IdempotencyKey,
        mut subscription: EventsSubscription,
    ) -> Result<LookupResult, RecvError> {
        // A deadline, so how often the check runs is a property of the check
        // rather than of how often this loop happens to restart. Both `continue`
        // paths below re-enter it, and a sleep started fresh each time would
        // measure the interval from the last restart instead of running every
        // interval. A receiver that keeps falling
        // `invocation_result_broadcast_capacity` events behind restarts this
        // loop every 100ms, and with a fresh sleep each time the check would
        // never run at all.
        let mut next_ownership_check =
            tokio::time::Instant::now() + INVOCATION_OWNERSHIP_RECHECK_INTERVAL;

        loop {
            match self.lookup_invocation_result(key).await {
                LookupResult::Interrupted => break Ok(LookupResult::Interrupted),
                LookupResult::New | LookupResult::Pending => {
                    let waiting = subscription.wait_for(|event| match event {
                        Event::InvocationCompleted {
                            agent_id,
                            idempotency_key,
                            result,
                        } if *agent_id == self.owned_agent_id.agent_id
                            && idempotency_key == key =>
                        {
                            Some(LookupResult::Complete(*result.clone()))
                        }
                        _ => None,
                    });

                    // The deadline is polled first, and `biased` makes that an
                    // order rather than a coin toss. A receiver that has fallen
                    // behind the bus is ready at once, with `Lagged`, and a
                    // select that polled it first would take that arm every
                    // time and never look at the timer. Under sustained lag
                    // that starves the ownership check for as long as the lag
                    // lasts, and a caller whose agent has moved is back to
                    // waiting out its own timeout. Polling the deadline first
                    // costs nothing while it is in the future, and when it is
                    // due, a result that arrived in the same instant is not
                    // lost: the check either re-enters the loop, whose lookup
                    // finds it, or reads it before rerouting.
                    let wait_result = tokio::select! {
                        biased;
                        () = tokio::time::sleep_until(next_ownership_check) => {
                            next_ownership_check = tokio::time::Instant::now()
                                + INVOCATION_OWNERSHIP_RECHECK_INTERVAL;

                            // An agent whose shard has moved is resumed by whoever owns
                            // it now, and its `InvocationCompleted` is published on that
                            // executor's bus. Nothing will ever arrive on ours, and the
                            // only other way out of this loop is the result turning up in
                            // this `Worker`'s own memory, which it never will either. So
                            // hand the caller the error that makes worker-service
                            // invalidate its routing table and retry against the new
                            // owner, rather than leave it to find out by timing out.
                            //
                            // Only `InvalidShardId` ends the wait. An executor whose
                            // shard assignment is not set yet fails this check too, with
                            // an `Unknown` from `sharding_not_ready_error`, and that one
                            // has to fall through and keep waiting: an assignment is on
                            // its way, and the agent may well still be ours.
                            if let Err(error @ WorkerExecutorError::InvalidShardId { .. }) =
                                self.shard_service().check_worker(&self.owned_agent_id.agent_id)
                            {
                                // The invocation can have finished while we were deciding
                                // that. `store_invocation_success` fills `invocation_results`
                                // before it publishes, so the result is already readable
                                // here, and a real result always beats a reroute.
                                match self.lookup_invocation_result(key).await {
                                    LookupResult::New | LookupResult::Pending => {
                                        debug!("Agent is no longer owned by this executor, ending the wait for its invocation result");
                                        break Ok(LookupResult::Complete(Err(error)));
                                    }
                                    settled => break Ok(settled),
                                }
                            }
                            continue;
                        }
                        result = waiting => result,
                    };
                    match wait_result {
                        Ok(result) => break Ok(result),
                        Err(RecvError::Lagged(_)) => {
                            tokio::time::sleep(Duration::from_millis(100)).await;
                            continue;
                        }
                        Err(RecvError::Closed) => break Err(RecvError::Closed),
                    }
                }
                LookupResult::Complete(result) => break Ok(LookupResult::Complete(result)),
            }
        }
    }
    pub async fn lookup_invocation_result(&self, key: &IdempotencyKey) -> LookupResult {
        // Kept as an `Arc` rather than cloned out of. The ordered actor read ensures a preceding
        // completion fold is published first. The record owns
        // `invocation_results`, which gains an entry per invocation, so deep-copying it to read
        // one key made each lookup cost more than the last.
        let status = self.state_actor.status().await;
        let cached = self
            .hydrated_invocation_results
            .read()
            .await
            .get_valid(key, &status)
            .map(|(result, oplog_idx)| (result.clone(), oplog_idx));
        let maybe_result = if cached.is_some() {
            crate::metrics::workers::record_invocation_result_resolution("memory_exact");
            cached
        } else if let Some(oplog_idx) = status.invocation_results.get(key) {
            crate::metrics::workers::record_invocation_result_resolution("memory_exact");
            Some((
                Arc::new(InvocationResult::Lazy {
                    oplog_idx: *oplog_idx,
                }),
                *oplog_idx,
            ))
        } else if status.invocation_results.is_exact_complete() {
            crate::metrics::workers::record_invocation_result_resolution("memory_exact_miss");
            None
        } else if !status.invocation_results.might_contain(key) {
            crate::metrics::workers::record_invocation_result_resolution("bloom_negative");
            None
        } else {
            // An oplog that does not read is not a miss: the key may well have a result there.
            match self.resolve_old_invocation_result_index(&status, key).await {
                Ok(oplog_idx) => oplog_idx
                    .map(|oplog_idx| (Arc::new(InvocationResult::Lazy { oplog_idx }), oplog_idx)),
                Err(error) => return LookupResult::Complete(Err(error.into())),
            }
        };
        if let Some((mut result, result_oplog_idx)) = maybe_result {
            if matches!(result.as_ref(), InvocationResult::Lazy { .. })
                && let Err(error) = Arc::make_mut(&mut result)
                    .cache(
                        &self.owned_agent_id,
                        self.agent_mode(),
                        self.initial_worker_metadata.fingerprint,
                        self,
                    )
                    .await
            {
                return LookupResult::Complete(Err(error.into()));
            }
            self.hydrated_invocation_results.write().await.insert(
                key.clone(),
                result.clone(),
                status.invocation_results.revert_generation(),
                result_oplog_idx,
            );
            lookup_result_from_cached_result(&status, key, Arc::unwrap_or_clone(result))
        } else {
            let is_pending = status
                .pending_invocations
                .iter()
                .any(|entry| entry.has_idempotency_key(key));
            let is_current = status.current_idempotency_key.as_ref() == Some(key);
            if is_pending || is_current {
                LookupResult::Pending
            } else {
                LookupResult::New
            }
        }
    }
    pub(in crate::worker) async fn resolve_old_invocation_result_index(
        &self,
        status: &AgentStatusRecord,
        key: &IdempotencyKey,
    ) -> Result<Option<OplogIndex>, UnreadableOplog> {
        let worker_service = self.deps.worker_service();
        let lookup = worker_service
            .lookup_invocation_result_index(
                &self.owned_agent_id,
                self.initial_worker_metadata.fingerprint,
                status,
                key,
            )
            .await;
        match lookup {
            Ok(InvocationResultIndexLookup::Found(index)) => {
                crate::metrics::workers::record_invocation_result_resolution("physical_hit");
                return Ok(Some(index));
            }
            Ok(InvocationResultIndexLookup::DefinitiveMiss) => {
                crate::metrics::workers::record_invocation_result_resolution("physical_miss");
                return Ok(None);
            }
            Ok(InvocationResultIndexLookup::Incomplete) | Err(_) => {
                crate::metrics::workers::record_invocation_result_resolution("physical_incomplete");
            }
        }

        if worker_service
            .catch_up_invocation_result_index(
                &self.owned_agent_id,
                self.agent_mode(),
                self.initial_worker_metadata.fingerprint,
                status,
            )
            .await
            .is_ok()
        {
            match worker_service
                .lookup_invocation_result_index(
                    &self.owned_agent_id,
                    self.initial_worker_metadata.fingerprint,
                    status,
                    key,
                )
                .await
            {
                Ok(InvocationResultIndexLookup::Found(index)) => {
                    crate::metrics::workers::record_invocation_result_resolution("physical_hit");
                    return Ok(Some(index));
                }
                Ok(InvocationResultIndexLookup::DefinitiveMiss) => {
                    crate::metrics::workers::record_invocation_result_resolution("physical_miss");
                    return Ok(None);
                }
                Ok(InvocationResultIndexLookup::Incomplete) | Err(_) => {}
            }
        }

        crate::metrics::workers::record_invocation_result_resolution("oplog_fallback");
        let mut current_idempotency_key = None;
        let mut cancelled_idempotency_key = None;
        let mut result = None;
        let mut first = OplogIndex::INITIAL;
        let chunk_size = self
            .deps
            .config()
            .invocation_results
            .physical_index_catch_up_chunk_size
            .max(1);
        while first <= status.oplog_idx {
            let count = (status.oplog_idx.as_u64() - first.as_u64() + 1).min(chunk_size);
            let entries = self
                .deps
                .oplog_service()
                .read_exact(&self.owned_agent_id, self.agent_mode(), first, count)
                .await?;
            if entries.is_empty() {
                break;
            }
            fold_invocation_result_entries(
                &mut current_idempotency_key,
                &mut cancelled_idempotency_key,
                &status.deleted_regions,
                &entries,
                |candidate, index| {
                    if candidate == key {
                        result = Some(index);
                    }
                },
            );
            first = entries.keys().max().unwrap().next();
        }
        Ok(result)
    }
}

pub(in crate::worker) fn recovery_agent_error(error: &WorkerExecutorError) -> AgentError {
    match error {
        WorkerExecutorError::FailedToResumeAgent { reason, .. } => recovery_agent_error(reason),
        WorkerExecutorError::InvalidRequest { details }
        | WorkerExecutorError::ParamTypeMismatch { details }
        | WorkerExecutorError::ValueMismatch { details } => {
            AgentError::InvalidRequest(details.clone())
        }
        WorkerExecutorError::PermissionDenied { details } => {
            AgentError::PermissionDenied(details.clone())
        }
        WorkerExecutorError::ReadOnlyViolation {
            method,
            host_function,
        } => AgentError::ReadOnlyViolation(ReadOnlyViolationError {
            method: method.clone(),
            host_function: host_function.clone(),
        }),
        WorkerExecutorError::InvocationFailed { error, .. }
        | WorkerExecutorError::PreviousInvocationFailed { error, .. } => error.clone(),
        WorkerExecutorError::UnexpectedOplogEntry { expected, got } => AgentError::InternalError(
            format!("Unexpected oplog entry during replay: expected {expected}, got {got}"),
        ),
        WorkerExecutorError::ComponentParseFailed { .. } => {
            AgentError::InternalError(error.to_string())
        }
        WorkerExecutorError::Runtime { .. }
        | WorkerExecutorError::RecoveryRequired { .. }
        | WorkerExecutorError::Unknown { .. }
        | WorkerExecutorError::AgentCreationFailed { .. }
        | WorkerExecutorError::ComponentNotFound { .. }
        | WorkerExecutorError::ComponentDownloadFailed { .. }
        | WorkerExecutorError::ComponentServiceUnavailable { .. }
        | WorkerExecutorError::ComponentServiceRefused { .. }
        | WorkerExecutorError::GetCurrentVersionOfComponentFailed { .. }
        | WorkerExecutorError::FileSystemError { .. }
        | WorkerExecutorError::InvalidShardId { .. }
        | WorkerExecutorError::ShardingNotReady => AgentError::Unknown(error.to_string()),
        _ => AgentError::InternalError(error.to_string()),
    }
}

pub(in crate::worker) fn recovery_retry_from(error: &WorkerExecutorError) -> Option<OplogIndex> {
    match error {
        WorkerExecutorError::RecoveryRequired { retry_from, .. } => *retry_from,
        WorkerExecutorError::FailedToResumeAgent { reason, .. } => recovery_retry_from(reason),
        _ => None,
    }
}

pub(in crate::worker) fn is_infrastructure_recovery_error(error: &WorkerExecutorError) -> bool {
    match error {
        WorkerExecutorError::FailedToResumeAgent { reason, .. } => {
            is_infrastructure_recovery_error(reason)
        }
        WorkerExecutorError::Runtime { .. }
        | WorkerExecutorError::RecoveryRequired { .. }
        | WorkerExecutorError::Unknown { .. }
        | WorkerExecutorError::AgentCreationFailed { .. }
        | WorkerExecutorError::ComponentNotFound { .. }
        | WorkerExecutorError::ComponentDownloadFailed { .. }
        | WorkerExecutorError::ComponentServiceUnavailable { .. }
        | WorkerExecutorError::ComponentServiceRefused { .. }
        | WorkerExecutorError::GetCurrentVersionOfComponentFailed { .. }
        | WorkerExecutorError::FileSystemError { .. }
        | WorkerExecutorError::InvalidShardId { .. }
        | WorkerExecutorError::ShardingNotReady => true,
        _ => false,
    }
}

pub(in crate::worker) fn lookup_result_from_cached_result(
    status: &AgentStatusRecord,
    key: &IdempotencyKey,
    result: InvocationResult,
) -> LookupResult {
    match result {
        InvocationResult::Cached {
            result: Ok(values), ..
        } => LookupResult::Complete(Ok(values)),
        InvocationResult::Cached {
            result:
                Err(FailedInvocationResult {
                    // Retry marker error entries are persisted before the invocation has
                    // actually finished. While the same idempotency key is still current
                    // and there is no terminal invocation outcome, report it as pending
                    // so lookup callers can observe the eventual result. A recovery
                    // failure makes the agent unavailable but does not finish this invocation.
                    trap_type: TrapType::Error { .. },
                    ..
                }),
        } if status.current_idempotency_key.as_ref() == Some(key)
            && (status.status != AgentStatus::Failed
                || status.last_error_kind == Some(OplogErrorKind::Recovery))
            && status.status != AgentStatus::Exited =>
        {
            LookupResult::Pending
        }
        InvocationResult::Cached {
            result:
                Err(FailedInvocationResult {
                    trap_type: TrapType::Interrupt(InterruptKind::Interrupt(_)),
                    ..
                }),
            ..
        } => LookupResult::Interrupted,
        InvocationResult::Cached {
            result:
                Err(FailedInvocationResult {
                    trap_type: TrapType::Interrupt(_),
                    ..
                }),
            ..
        } => LookupResult::Pending,
        InvocationResult::Cached {
            result:
                Err(FailedInvocationResult {
                    trap_type:
                        TrapType::Error {
                            error: AgentError::PermissionDenied(details),
                            ..
                        },
                    ..
                }),
            ..
        } => LookupResult::Complete(Err(WorkerExecutorError::permission_denied(details))),
        InvocationResult::Cached {
            result:
                Err(FailedInvocationResult {
                    trap_type: TrapType::Error { error, .. },
                    stderr,
                }),
            ..
        } => LookupResult::Complete(Err(WorkerExecutorError::InvocationFailed { error, stderr })),
        InvocationResult::Cached {
            result:
                Err(FailedInvocationResult {
                    trap_type: TrapType::Exit,
                    ..
                }),
            ..
        } => LookupResult::Complete(Err(WorkerExecutorError::runtime("Process exited"))),
        InvocationResult::Lazy { .. } => {
            panic!("Unexpected lazy result after InvocationResult.cache")
        }
    }
}

impl HydratedInvocationResultCache {
    pub(in crate::worker) fn new(capacity: usize) -> Self {
        Self {
            values: HashMap::new(),
            insertion_order: VecDeque::new(),
            capacity,
            unprojected_completions: HashMap::new(),
        }
    }

    /// Whether `key` has a result on the branch of `status`, cached or committed but not yet
    /// folded into it.
    pub(in crate::worker) fn is_complete(
        &self,
        key: &IdempotencyKey,
        status: &AgentStatusRecord,
    ) -> bool {
        self.get_valid(key, status).is_some()
            || self
                .unprojected_completions
                .get(key)
                .is_some_and(|completion| {
                    completion.revert_generation == status.invocation_results.revert_generation()
                        && !status
                            .deleted_regions
                            .is_in_deleted_region(completion.oplog_idx)
                })
    }

    pub(in crate::worker) fn insert_unprojected_completion(
        &mut self,
        key: IdempotencyKey,
        value: impl Into<Arc<InvocationResult>>,
        revert_generation: u64,
        oplog_idx: OplogIndex,
    ) {
        self.unprojected_completions.insert(
            key.clone(),
            UnprojectedCompletion {
                revert_generation,
                oplog_idx,
            },
        );
        self.insert(key, value, revert_generation, oplog_idx);
    }

    /// Forgets the unprojected completions that an attached `status` has folded, or that belong to
    /// another oplog branch.
    pub(in crate::worker) fn release_projected_completions(
        &mut self,
        status: &AgentStatusRecord,
        detached: bool,
    ) {
        if detached {
            return;
        }
        self.unprojected_completions.retain(|_, completion| {
            completion.revert_generation == status.invocation_results.revert_generation()
                && completion.oplog_idx > status.oplog_idx
        });
    }

    pub(in crate::worker) fn get_valid(
        &self,
        key: &IdempotencyKey,
        status: &AgentStatusRecord,
    ) -> Option<(&Arc<InvocationResult>, OplogIndex)> {
        self.values.get(key).and_then(|entry| {
            (entry.revert_generation == status.invocation_results.revert_generation()
                && !status.deleted_regions.is_in_deleted_region(entry.oplog_idx))
            .then_some((&entry.result, entry.oplog_idx))
        })
    }

    #[cfg(test)]
    pub(in crate::worker) fn contains_key(&self, key: &IdempotencyKey) -> bool {
        self.values.contains_key(key)
    }

    pub(in crate::worker) fn insert(
        &mut self,
        key: IdempotencyKey,
        value: impl Into<Arc<InvocationResult>>,
        revert_generation: u64,
        oplog_idx: OplogIndex,
    ) {
        if !self.values.contains_key(&key) {
            self.insertion_order.push_back(key.clone());
        }
        self.values.insert(
            key,
            HydratedInvocationResultCacheEntry {
                result: value.into(),
                revert_generation,
                oplog_idx,
            },
        );
        while self.values.len() > self.capacity {
            if let Some(oldest) = self.insertion_order.pop_front() {
                self.values.remove(&oldest);
            }
        }
    }

    pub(in crate::worker) fn remove(
        &mut self,
        key: &IdempotencyKey,
    ) -> Option<HydratedInvocationResultCacheEntry> {
        self.unprojected_completions.remove(key);
        let result = self.values.remove(key);
        if result.is_some() {
            self.insertion_order.retain(|entry| entry != key);
        }
        result
    }

    pub(in crate::worker) fn clear(&mut self) {
        self.values.clear();
        self.insertion_order.clear();
        self.unprojected_completions.clear();
    }
}

impl InvocationResult {
    pub async fn cache<T: HasOplog + HasOplogService + HasConfig + HasComponentService>(
        &mut self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        agent_fingerprint: AgentFingerprint,
        services: &T,
    ) -> Result<(), UnreadableOplog> {
        if let Self::Lazy { oplog_idx } = self {
            let oplog_idx = *oplog_idx;
            let entry = services.oplog().read(oplog_idx).await?;

            let result = match entry {
                OplogEntry::AgentInvocationFinished {
                    result,
                    consumed_fuel,
                    component_revision,
                    ..
                } => {
                    let invocation_result: AgentInvocationResult = services
                        .oplog()
                        .download_payload(result)
                        .await
                        .expect("failed to deserialize function response payload");
                    Ok(AgentInvocationOutput {
                        result: invocation_result,
                        consumed_fuel: Some(consumed_fuel as u64),
                        invocation_status: None,
                        component_revision: Some(component_revision),
                        agent_id: None,
                        idempotency_key: None,
                        // `oplog_idx` is the index of the matched
                        // `AgentInvocationFinished` entry. The fingerprint is
                        // the current worker's per-instance fingerprint: the
                        // oplog is owned by a single worker instance, so any
                        // `AgentInvocationFinished` we read from it was
                        // necessarily produced by that instance.
                        oplog_index: Some(oplog_idx),
                        agent_fingerprint: Some(agent_fingerprint),
                    })
                }
                OplogEntry::Error {
                    error, retry_from, ..
                } => {
                    let stderr =
                        recover_stderr_logs(services, owned_agent_id, agent_mode, oplog_idx).await;
                    Err(FailedInvocationResult {
                        trap_type: TrapType::Error {
                            error,
                            retry_from,
                            semantic_trap_retry_override: None,
                        },
                        stderr,
                    })
                }
                OplogEntry::Interrupted { .. } => Err(FailedInvocationResult {
                    trap_type: TrapType::Interrupt(InterruptKind::Interrupt(Timestamp::now_utc())),
                    stderr: "".to_string(),
                }),
                OplogEntry::Exited { .. } => Err(FailedInvocationResult {
                    trap_type: TrapType::Exit,
                    stderr: "".to_string(),
                }),
                _ => panic!(
                    "Unexpected oplog entry pointed by invocation result at index {oplog_idx} for {owned_agent_id:?}"
                ),
            };

            *self = Self::Cached { result }
        }
        Ok(())
    }
}
