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

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    /// Replays the recorded invocations of the start. A failure says
    /// whether the replay after an application snapshot diverged from the oplog with a typed
    /// divergence while the start still replayed it.
    pub(in crate::durable_host) async fn replay(
        store: &mut Store<Ctx>,
        instance: &Instance,
        refresh_replay_target: bool,
    ) -> Result<Option<RetryDecision>, ReplayFailure> {
        let mut diverged = false;
        let result = async {
        let mut number_of_replayed_functions = 0;

        if refresh_replay_target {
            let new_target = store
                .as_context()
                .data()
                .durable_ctx()
                .public_state
                .worker()
                .oplog()
                .current_oplog_index()
                .await;

            store
                .as_context()
                .data()
                .durable_ctx()
                .state
                .replay_state
                .set_replay_target(new_target)
                .await?;
        }

        let agent_mode = store.as_context().data().agent_mode();

        let resume_result = loop {
            let context = store.as_context();
            let durable_ctx = context.data().durable_ctx();
            let cont = durable_ctx.state.is_replay() && should_replay_invocation(
                agent_mode,
                durable_ctx.owner_context(),
                number_of_replayed_functions,
            );

            if cont {
                let oplog_entry = store
                    .as_context_mut()
                    .data_mut()
                    .durable_ctx_mut()
                    .state
                    .replay_state
                    .get_oplog_entry_agent_invocation_started()
                    .await;

                match oplog_entry {
                    Err(error) => break Err(error),
                    Ok(None) => {
                        let mut store_context = store.as_context_mut();
                        let durable_ctx = store_context.data_mut().durable_ctx_mut();
                        durable_ctx.switch_to_live().await?;
                        break Ok(None);
                    }
                    Ok(Some(replay_state::AgentInvocationStartedEntry {
                        oplog_index,
                        idempotency_key,
                        invocation_payload,
                        invocation_context,
                        wallet_pin,
                    })) => {
                        let agent_invocation = AgentInvocation::from_parts(
                            idempotency_key.clone(),
                            invocation_payload,
                            invocation_context.clone(),
                        );
                        if agent_mode == AgentMode::Ephemeral
                            && !matches!(agent_invocation, AgentInvocation::AgentInitialization { .. })
                        {
                            break Err(WorkerExecutorError::unexpected_oplog_entry(
                                "AgentInitialization for ephemeral initialization replay",
                                format!("{:?}", agent_invocation.kind()),
                            ));
                        }
                        let scope_card = agent_invocation.scope_card().cloned();
                        let recorded_scope_card_id = wallet_pin.scope_card_id;
                        let payload_scope_card_id =
                            scope_card.as_ref().map(|card| card.scope_card_id);
                        if payload_scope_card_id != recorded_scope_card_id {
                            break Err(WorkerExecutorError::unexpected_oplog_entry(
                                "matching invocation scope-card payload and wallet pin",
                                format!(
                                    "payload scope-card ID {payload_scope_card_id:?}, recorded scope-card ID {recorded_scope_card_id:?}"
                                ),
                            ));
                        }

                        store
                            .as_context_mut()
                            .data_mut()
                            .durable_ctx_mut()
                            .install_invocation_scope_card(scope_card, Vec::new(), None)
                            .await;

                        let preparation = async {
                            store
                                .as_context_mut()
                                .data_mut()
                                .durable_ctx_mut()
                                .process_pending_replay_events()
                                .await?;

                            let component_metadata = store
                                .as_context()
                                .data()
                                .component_metadata()
                                .metadata
                                .clone();
                            let worker = store.as_context().data().get_public_state().worker();
                            let agent_id = store.as_context().data().parsed_agent_id();
                            let uses_streams = invocation_uses_streams(
                                &agent_invocation,
                                &component_metadata,
                                agent_id.as_ref(),
                            );
                            let agent_invocation = if uses_streams {
                                worker
                                    .rehydrate_durable_streaming_invocation(agent_invocation)
                                    .await?
                            } else {
                                agent_invocation
                            };
                            let lowered = lower_invocation(
                                agent_invocation,
                                &component_metadata,
                                agent_id.as_ref(),
                            )?;
                            Ok::<_, WorkerExecutorError>((worker, uses_streams, lowered))
                        }
                        .await;
                        let (worker, uses_streams, lowered) = match preparation {
                            Ok(preparation) => preparation,
                            Err(error) => {
                                store
                                    .as_context_mut()
                                    .data_mut()
                                    .durable_ctx_mut()
                                    .clear_invocation_scope_card()
                                    .await;
                                break Err(error);
                            }
                        };
                        let full_function_name = lowered.display_name.clone();

                        debug!("Replaying function {}", &full_function_name);
                        debug!(
                            "Replay state: {:?}",
                            store.as_context().data().durable_ctx().state.replay_state
                        );
                        let span = span!(
                            Level::INFO,
                            "replaying",
                            function = full_function_name.as_str()
                        );
                        store
                            .as_context_mut()
                            .data_mut()
                            .set_current_idempotency_key(idempotency_key.clone())
                            .await;

                        let (local_span_ids, inherited_span_ids) = invocation_context.span_ids();
                        store
                            .as_context_mut()
                            .data_mut()
                            .set_current_invocation_context(invocation_context)
                            .await?;
                        store
                            .as_context_mut()
                            .data_mut()
                            .durable_ctx_mut()
                            .primary_invocation_start_index = Some(oplog_index);
                        // An invocation retained only through its start has no recorded body.
                        // Publish live before pure guest code or logging runs, without waiting
                        // for a durable host call to observe the exhausted replay cursor.
                        if store.as_context().data().durable_ctx().state.replay_state.is_live()
                            && let Err(error) = store
                                .as_context_mut()
                                .data_mut()
                                .durable_ctx_mut()
                                .switch_to_live()
                                .await
                        {
                            store.as_context_mut().data_mut().durable_ctx_mut()
                                .clear_invocation_scope_card().await;
                            break Err(error);
                        }
                        let invoke_result = invoke_observed_and_traced(
                            lowered,
                            store,
                            instance,
                            InvocationMode::Replay,
                        )
                        .instrument(span)
                        .await;

                        let invoke_result = if uses_streams {
                            materialize_streaming_result(store, invoke_result, &full_function_name, &idempotency_key).await
                        } else {
                            invoke_result
                        };

                        // Invocation-owned spans are closed by AgentInvocationFinished in the
                        // oplog processor; removing their resident context records no transition.
                        for span_id in local_span_ids {
                            store.as_context_mut().data_mut().remove_span(&span_id)?;
                        }
                        for span_id in inherited_span_ids {
                            store.as_context_mut().data_mut().remove_span(&span_id)?;
                        }

                        match invoke_result {
                            Ok(InvokeResult::Succeeded {
                                result: invocation_result,
                                consumed_fuel,
                            }) => {
                                let component_revision =
                                    store.as_context().data().component_metadata().revision;
                                let mut output = AgentInvocationOutput {
                                    result: *invocation_result,
                                    consumed_fuel: Some(consumed_fuel),
                                    invocation_status: None,
                                    component_revision: Some(component_revision),
                                    agent_id: None,
                                    idempotency_key: None,
                                    oplog_index: None,
                                    agent_fingerprint: None,
                                };
                                if let Err(err) = store
                                    .as_context_mut()
                                    .data_mut()
                                    .on_agent_invocation_success(
                                        &full_function_name,
                                        consumed_fuel,
                                        &mut output,
                                    )
                                    .await
                                {
                                    break Err(err);
                                }
                                // An invocation that was interrupted by a crash reaches its end
                                // here, in live mode, instead of in the invocation loop, so its
                                // durable Stream Session must be finished here as well;
                                // otherwise resumed session clients never observe completion.
                                if uses_streams
                                    && store.as_context().data().durable_ctx().is_live()
                                {
                                    let worker = worker.clone();
                                    let idempotency_key = idempotency_key.clone();
                                    let result = store.run_concurrent(async move |_accessor| {
                                        worker.complete_durable_streaming_session(&idempotency_key).await
                                    }).await;
                                    if let Err(error) = result.map_err(|error| WorkerExecutorError::runtime(error.to_string())).and_then(|result| result) {
                                        error!(%error, "Failed to complete durable streaming session");
                                        break Err(error);
                                    }
                                }
                                if store.as_context().data().durable_ctx().is_live() {
                                    worker.reset_infrastructure_recovery_backoff();
                                }
                                number_of_replayed_functions += 1;
                                continue;
                            }
                            _ => {
                                let owner_failure = store
                                    .as_context()
                                    .data()
                                    .durable_ctx()
                                    .selected_tool_owner_failure();
                                if let Some(error) = selected_recovery_error(
                                    owner_failure.as_ref(),
                                    store.as_context().data().agent_mode(),
                                    &invoke_result,
                                ) {
                                    break Err(error.clone());
                                }
                                let details = format!("{invoke_result:?}");
                                let snapshot_divergence = match &invoke_result {
                                    Ok(result) => result.is_snapshot_replay_divergence(),
                                    Err(WorkerExecutorError::UnexpectedOplogEntry { .. }) => true,
                                    Err(_) => false,
                                };
                                let trap_type = match invoke_result {
                                    Ok(invoke_result) => invoke_result.as_trap_type::<Ctx>(),
                                    Err(error) => {
                                        Some(TrapType::from_worker_executor_error::<Ctx>(
                                            error,
                                            OplogIndex::INITIAL,
                                            store.as_context().data().agent_mode(),
                                        ))
                                    }
                                }
;
                                let decision = match trap_type {
                                    // A recorded invocation that fails while its entries are still
                                    // being replayed after an automatic snapshot load most likely
                                    // diverged from the recorded execution because the guest state
                                    // restored from the snapshot differs from the original one. The
                                    // recorded oplog is authoritative, so instead of committing the
                                    // failure the snapshot is rejected and the worker starts again
                                    // without it, from an earlier usable periodic record or from
                                    // its authoritative baseline.
                                    Some(TrapType::Error { error, .. })
                                        if snapshot_divergence
                                            && Self::rejects_diverged_record(
                                                store,
                                                &WorkerExecutorError::runtime(error.message()),
                                            ) =>
                                    {
                                        Some(Self::abandon_diverged_automatic_snapshot(
                                            store,
                                            &error.message(),
                                            true,
                                        ))
                                    }
                                    Some(trap_type)
                                        if store.as_context().data().durable_ctx().state.snapshot_replay_purpose
                                            != SnapshotReplayPurpose::None
                                            && !store.as_context().data().durable_ctx().is_live()
                                            && !store.as_context().data().durable_ctx().state.authoritative_replay_failure.load(Ordering::Acquire) =>
                                    {
                                        // Speculative reconstruction failures, after a snapshot or
                                        // in the replay for a pending automatic update, must not
                                        // append an authoritative invocation Error for already
                                        // recorded work: the start decides what the failure means.
                                        // The failure of the replay says whether it diverged.
                                        diverged = snapshot_divergence;
                                        match trap_type {
                                            // A trap that every invocation retries, such as an
                                            // out-of-memory under the memory admission, keeps its
                                            // retry: it says nothing about the replay.
                                            TrapType::Error { .. } if let Some(retry) = Self::speculative_retry(&trap_type) => Some(retry),
                                            TrapType::Error { error, .. } => break Err(WorkerExecutorError::InvocationFailed {
                                                error,
                                                stderr: store.as_context().data().get_public_state().event_service().get_last_invocation_errors(),
                                            }),
                                            TrapType::Interrupt(kind) => {
                                                // `on_invocation_failure` is skipped on this path,
                                                // so a lost shard is recorded here: its `None`
                                                // decision alone would stop the agent as an
                                                // ordinary one, left cached to restart in place.
                                                worker.retire_if_shard_lost(&WorkerExecutorError::Interrupted { kind });
                                                Self::fixed_decision_for_trap_type(&TrapType::Interrupt(kind))
                                            }
                                            TrapType::Exit => break Err(WorkerExecutorError::runtime("Process exited during a speculative replay")),
                                        }
                                    }
                                    Some(trap_type) => {
                                        let decision = match store
                                            .as_context_mut()
                                            .data_mut()
                                            .on_invocation_failure(&full_function_name, &trap_type)
                                            .await
                                        {
                                            InvocationFailureDecision::Retry(decision) => decision,
                                            // The start retries the replay when the storage
                                            // answers again.
                                            InvocationFailureDecision::Infrastructure(error) => {
                                                break Err(error);
                                            }
                                        };

                                        if decision == RetryDecision::None {
                                            // Like the invocation loop, permanently fail the
                                            // durable Stream Session of an invocation that was
                                            // interrupted by a crash and cannot be retried.
                                            // A fresh interrupt is authoritative even before live
                                            // publication; already-finished sessions stay unchanged.
                                            // Not for a lost shard, however the loss
                                            // reached the trap: the shard's new owner resumes that
                                            // invocation.
                                            let shard_lost = matches!(
                                                trap_type,
                                                TrapType::Interrupt(InterruptKind::ShardLost)
                                            );
                                            if uses_streams
                                                && !shard_lost
                                                && (matches!(trap_type, TrapType::Interrupt(_))
                                                    || store.as_context().data().durable_ctx().is_live()
                                                    || store.as_context().data().durable_ctx().state.authoritative_replay_failure.load(Ordering::Acquire))
                                            {
                                                // A refused write means the shard has moved:
                                                // record it before the startup result is published.
                                                if let Err(error) = worker
                                                    .fail_durable_streaming_session(
                                                        &idempotency_key,
                                                        details,
                                                    )
                                                    .await
                                                {
                                                    worker.retire_if_shard_lost(&error);
                                                }
                                            }
                                            // Cannot retry so we need to fail
                                            match trap_type {
                                                TrapType::Interrupt(_interrupt_kind) => {
                                                    // In case of an interrupt, we return with RetryDecision::None
                                                    // as it is not an error.
                                                }
                                                TrapType::Exit => {
                                                    break Err(
                                                        WorkerExecutorError::PreviousInvocationExited,
                                                    );
                                                }
                                                TrapType::Error { error, .. } => {
                                                    let stderr = store
                                                        .as_context()
                                                        .data()
                                                        .get_public_state()
                                                        .event_service()
                                                        .get_last_invocation_errors();
                                                    break Err(
                                                        WorkerExecutorError::PreviousInvocationFailed {
                                                            error,
                                                            stderr,
                                                        },
                                                    );
                                                }
                                            }
                                        }

                                        Some(decision)
                                    }
                                    None => None,
                                };

                                break Ok(decision);
                            }
                        }
                    }
                }
            } else {
                store
                    .as_context_mut()
                    .data_mut()
                    .durable_ctx_mut()
                    .process_pending_replay_events()
                    .await?;
                break Ok(None);
            }
        };

        record_number_of_replayed_functions(number_of_replayed_functions);

        if matches!(resume_result, Ok(None)) {
            tool::durability::finish_producer_disposals(
                store.as_context_mut().data_mut().durable_ctx_mut(),
            ).await?;
            store
                .as_context_mut()
                .data_mut()
                .durable_ctx_mut()
                .state
                .snapshot_replay_purpose = SnapshotReplayPurpose::None;
        }

        resume_result
        }.await;
        store.data().set_suspended();
        // Result validation can consume the final recorded entry before detecting a mismatch.
        // Its typed replay error, rather than the resulting live cursor, identifies divergence.
        if let Err(error @ WorkerExecutorError::UnexpectedOplogEntry { .. }) = &result
            && Self::rejects_diverged_record(store, error)
        {
            return Ok(Some(Self::abandon_diverged_automatic_snapshot(
                store, error, true,
            )));
        }
        result.map_err(|error| ReplayFailure { error, diverged })
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> ExternalOperations<Ctx> for DurableWorkerCtx<Ctx> {
    type ExtraDeps = Ctx::ExtraDeps;

    async fn get_last_error_and_retry_count<T: HasAll<Ctx> + Send + Sync>(
        this: &T,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        latest_worker_status: &AgentStatusRecord,
    ) -> Option<LastError> {
        last_error(this, owned_agent_id, agent_mode, latest_worker_status).await
    }

    async fn prepare_instance(
        agent_id: &AgentId,
        instance: &Instance,
        store: &mut Store<Ctx>,
    ) -> Result<Option<RetryDecision>, WorkerExecutorError> {
        debug!("Starting prepare_instance");
        let start = Instant::now();
        store.as_context_mut().data_mut().set_running();

        let prepare_result = if store.as_context().data().agent_mode() == AgentMode::Ephemeral {
            // Ephemeral workers cannot be recovered

            // Only typed agent owners have an initialize call to replay.
            let replay_decision = Self::replay(store, instance, false)
                .await
                .map_err(|failure| failure.error);
            record_resume_worker(start.elapsed());

            if replay_decision == Ok(None) {
                // Moving to the end of the oplog
                store
                    .as_context_mut()
                    .data_mut()
                    .durable_ctx_mut()
                    .switch_to_live()
                    .await?;

                // Appending a Restart marker
                store
                    .as_context_mut()
                    .data_mut()
                    .get_public_state()
                    .oplog()
                    .add(OplogEntry::restart())
                    .await?;

                Ok(None)
            } else {
                replay_decision
            }
        } else {
            let pending_update = store
                .as_context_mut()
                .data_mut()
                .durable_ctx_mut()
                .state
                .pending_update
                .lock()
                .await
                .clone();

            match pending_update {
                Some(hydrated) => match &hydrated.description {
                    UpdateDescription::SnapshotBased { .. } => {
                        // If a snapshot based update is pending, no replay should be necessary
                        if !store.as_context().data().durable_ctx().is_live() {
                            return Err(WorkerExecutorError::runtime(
                                "snapshot-based pending update expected replay state to already be live",
                            ));
                        }

                        Self::finalize_pending_snapshot_update(instance, store).await
                    }
                    UpdateDescription::Automatic { .. }
                    | UpdateDescription::SnapshotAssistedAutomatic { .. } => {
                        let assisted = matches!(
                            hydrated.description,
                            UpdateDescription::SnapshotAssistedAutomatic { .. }
                        );
                        let attempt = async {
                            match Self::try_load_snapshot(store, instance).await {
                                SnapshotRecoveryResult::Success => {}
                                SnapshotRecoveryResult::NotAttempted if !assisted => {}
                                failure => return Err(AttemptFailure::Load(failure)),
                            }
                            // An automatic update succeeds at the end of the replay.
                            let result = Self::replay(store, instance, assisted)
                                .await
                                .map_err(AttemptFailure::Replay)?;
                            record_resume_worker(start.elapsed());
                            Ok(result)
                        }
                        .await;

                        match attempt {
                            Ok(result) => Ok(result),
                            Err(failure) => {
                                // A failure before the update point fails the update or retries
                                // it, as the outcome table decides. A failure after the update
                                // point consumed the update: the update point performed the
                                // outcome of its own failure, and its error ends the start.
                                let pending = store
                                    .as_context_mut()
                                    .data_mut()
                                    .durable_ctx_mut()
                                    .state
                                    .pending_update
                                    .lock()
                                    .await
                                    .take();
                                match pending {
                                    Some(pending) => {
                                        let action = {
                                            let raw = match &failure {
                                                AttemptFailure::Load(result) => {
                                                    start_outcome::RawStartError::Load(result)
                                                }
                                                AttemptFailure::Replay(ReplayFailure {
                                                    error,
                                                    diverged,
                                                }) => start_outcome::RawStartError::Replay {
                                                    error,
                                                    diverged: *diverged,
                                                },
                                            };
                                            store
                                                .as_context()
                                                .data()
                                                .durable_ctx()
                                                .start_action(Some(&pending.reference), raw)
                                        };
                                        Self::perform_start_action(store, action).await
                                    }
                                    None => Err(failure.into_error()),
                                }
                            }
                        }
                    }
                },
                None => match Self::try_load_snapshot(store, instance).await {
                    SnapshotRecoveryResult::Success | SnapshotRecoveryResult::NotAttempted => {
                        let result = Self::replay(store, instance, false)
                            .await
                            .map_err(|failure| failure.error);
                        record_resume_worker(start.elapsed());
                        result
                    }
                    failure => {
                        let action = store
                            .as_context()
                            .data()
                            .durable_ctx()
                            .start_action(None, start_outcome::RawStartError::Load(&failure));
                        Self::perform_start_action(store, action).await
                    }
                },
            }
        };
        match prepare_result {
            Ok(None) => {
                store
                    .as_context()
                    .data()
                    .get_public_state()
                    .worker()
                    .settle_exclusions_after_prepare()
                    .await?;
                store.as_context_mut().data_mut().set_suspended();
                Ok(None)
            }
            Ok(other) => Ok(other),
            Err(error) => Err(prepare_failure(agent_id, error)),
        }
    }

    async fn on_shard_assignment_changed<T: HasAll<Ctx> + Send + Sync + 'static>(
        this: &T,
    ) -> Result<(), anyhow::Error> {
        this.oplog_processor_plugin()
            .on_shard_assignment_changed()
            .await?;
        let snapshots = this.agent_filesystem_snapshots();
        let on_stale = |agent: &OwnedAgentId, fingerprint| {
            // Only a durable agent has a member in the recovery index.
            snapshots.delete_all_snapshots(
                &crate::filesystem_snapshot::AgentSnapshots::agent(agent, fingerprint),
                AgentMode::Durable,
            )
        };
        let workers = this
            .worker_service()
            .get_running_workers_in_shards(&on_stale)
            .await?;

        debug!(workers = ?workers, "Recovering running workers");

        // Only a worker that is genuinely gone may be skipped; every other failure fails the
        // assignment. See `recovered_status` for the distinction and for what failing buys.
        for worker in workers {
            let owned_agent_id = worker.initial_worker_metadata.owned_agent_id();
            let agent_mode = worker.initial_worker_metadata.agent_mode;
            let Some(latest_worker_status) = recovered_status(
                &owned_agent_id,
                calculate_last_known_status_with_checkpoint(
                    this,
                    &owned_agent_id,
                    worker.initial_worker_metadata.fingerprint,
                    agent_mode,
                    worker.last_known_status,
                )
                .await,
            )?
            else {
                continue;
            };

            // TODO: there is probably a race here between assignment changing and a suspended worker getting woken up.
            if should_restart_after_shard_assignment_change(&latest_worker_status) {
                let expected_fingerprint = worker.initial_worker_metadata.fingerprint;
                let activation = Worker::get_existing_running_with_fingerprint(
                    this,
                    &owned_agent_id,
                    expected_fingerprint,
                )
                .await;
                match activation {
                    Ok(_) => {}
                    Err(error @ WorkerExecutorError::AgentNotFound { .. }) => {
                        if this
                            .worker_service()
                            .remove_if_stale(&owned_agent_id, expected_fingerprint, &on_stale)
                            .await?
                        {
                            continue;
                        }
                        return Err(anyhow!(
                            "failed to restart {owned_agent_id} during shard-assignment recovery: {error}"
                        ));
                    }
                    // A shard that left the assignment mid-recovery is skipped, not fatal.
                    Err(error) => recovered_restart(&owned_agent_id, Err::<(), _>(error))?,
                }
            }
        }

        Ok(())
    }
}

impl AttemptFailure {
    fn into_error(self) -> WorkerExecutorError {
        match self {
            Self::Load(
                SnapshotRecoveryResult::Failed(error)
                | SnapshotRecoveryResult::Unavailable(error)
                | SnapshotRecoveryResult::Lost(error),
            )
            | Self::Replay(ReplayFailure { error, .. }) => error,
            Self::Load(_) => WorkerExecutorError::runtime(
                "Snapshot-assisted automatic update did not attempt its required snapshot",
            ),
        }
    }
}

/// What shard-assignment recovery does with one worker's computed status: `Ok(Some)` to restart
/// it, `Ok(None)` to skip it, `Err` to fail the whole assignment.
///
/// The same distinction `enum_workers_at_key` draws for the scan itself:
///
/// - `Ok(None)` from the status computation means the oplog is gone - a delete that raced the
///   index read. There is nothing to resume, so that one agent is skipped.
/// - `Err` is not evidence the worker is gone. Logging it and moving on would acknowledge the
///   assignment with the worker still stopped, and nothing would try again until an external
///   invocation happened to arrive: a running agent stranded silently for as long as this
///   executor owns the shard. Failing the assignment hands the retry to the caller instead. The
///   shard manager retries a failed `assign_shards`, and at startup the process exits and the
///   restart policy retries. Recovery is idempotent - a worker restarted on an earlier attempt is
///   simply found running on the next - so re-running the whole scan is safe. The restart that
///   follows a computed status is treated the same way by the caller.
///
/// Every storage read on this path has already had the key-value retry budget, which is sized to
/// outlast an AWS failover, so what arrives here as `Err` is what outlived that budget. A worker
/// that can never be restarted therefore keeps this executor from serving its shards, loudly,
/// rather than being dropped from recovery quietly. That is the intended trade: the loud failure
/// is visible and gets acted on, the quiet one is not. It is also the shape the oplog layers
/// already have - a bounded storage retry, then a fatal failure - at a lower cost, since a failed
/// assignment is one RPC rather than a process exit.
///
/// An oplog that does not read (`StatusError::Unreadable`) for good, because its data is corrupt or
/// missing, is not that kind of failure: no retry of the assignment reads it, so failing the
/// assignment would stop every other agent forever. That one agent is skipped and logged, and its
/// oplog is left as it is. An oplog whose storage did not answer
/// ([`UnreadableOplog::is_transient`]) is that kind of failure: a retry can read it.
pub(in crate::durable_host) fn recovered_status(
    owned_agent_id: &OwnedAgentId,
    computed: Result<Option<AgentStatusRecord>, StatusError>,
) -> Result<Option<AgentStatusRecord>, anyhow::Error> {
    match computed {
        Ok(Some(status)) => Ok(Some(status)),
        Ok(None) => {
            error!(agent_id = %owned_agent_id, "Worker oplog disappeared during shard-assignment recovery; skipping agent");
            Ok(None)
        }
        // No retry reads the oplog, so failing the assignment would keep every other agent from
        // being resumed. The agent is left as it is, with its oplog, for an operator to inspect.
        Err(StatusError::Unreadable(error)) if !error.is_transient() => {
            log_unreadable_recovered_agent(owned_agent_id, &error);
            Ok(None)
        }
        Err(error) => Err(anyhow!(
            "failed to calculate the status of {owned_agent_id} during shard-assignment recovery, so it cannot be resumed: {error}"
        )),
    }
}

/// The outcome of restarting one recovered agent, drawing the same line as [`recovered_status`].
///
/// `ShardingNotReady` means the agent's shard left this executor's assignment while recovery was
/// running: a later delivery revoked it, and the worker was refused an epoch rather than opened
/// unfenced. `OplogFenced` means another executor claimed the agent's oplog at a newer epoch
/// before this one wrote to it. Either way the agent belongs to the shard's new owner, which
/// recovers it there. This is a skip, like an oplog that is gone. Failing the whole assignment for
/// it would stop every other agent in the scan from being resumed. An agent whose oplog does not
/// read (`OplogFailed`, for example from rebuilding its stream session index) is skipped too:
/// no retry of the assignment reads that oplog. Any other restart failure still fails the
/// assignment.
pub(in crate::durable_host) fn recovered_restart<W>(
    owned_agent_id: &OwnedAgentId,
    restarted: Result<W, WorkerExecutorError>,
) -> Result<(), anyhow::Error> {
    match restarted {
        Ok(_) => Ok(()),
        Err(WorkerExecutorError::ShardingNotReady | WorkerExecutorError::OplogFenced { .. }) => {
            debug!(agent_id = %owned_agent_id, "Worker's shard left the assignment during shard-assignment recovery; skipping agent");
            Ok(())
        }
        // As for a status that does not fold: no retry of the assignment reads the oplog.
        Err(error) if is_failed_oplog(&error) => {
            log_unreadable_recovered_agent(owned_agent_id, &error);
            Ok(())
        }
        Err(error) => Err(anyhow!(
            "failed to restart {owned_agent_id} during shard-assignment recovery: {error}"
        )),
    }
}

/// Logs an agent that shard-assignment recovery skips because its oplog does not read.
fn log_unreadable_recovered_agent(owned_agent_id: &OwnedAgentId, error: &dyn std::fmt::Display) {
    error!(agent_id = %owned_agent_id, error = %error, "Worker oplog is unreadable during shard-assignment recovery; skipping agent");
}

/// Whether `error` is an oplog that failed, also when a failed resume carries it.
fn is_failed_oplog(error: &WorkerExecutorError) -> bool {
    match error {
        WorkerExecutorError::OplogFailed { .. } => true,
        WorkerExecutorError::FailedToResumeAgent { reason, .. } => is_failed_oplog(reason),
        _ => false,
    }
}

pub(in crate::durable_host) fn should_restart_after_shard_assignment_change(
    status: &AgentStatusRecord,
) -> bool {
    status.status != AgentStatus::Interrupted
        && (matches!(
            status.status,
            AgentStatus::Running | AgentStatus::Idle | AgentStatus::Retrying
        ) || status.has_pending_work())
}

pub(in crate::durable_host) fn should_replay_invocation(
    mode: AgentMode,
    owner: &ResolvedOwnerContext,
    replayed_invocations: usize,
) -> bool {
    mode == AgentMode::Durable
        || (matches!(owner, ResolvedOwnerContext::Agent(_)) && replayed_invocations == 0)
}

/// The infrastructure error that ends an invocation or a replay of a durable agent outside the
/// guest's failure handling: a reconstruction that is required, or an oplog that failed.
///
/// The winner of the tool-owner arbitration comes first. An infrastructure winner is that error;
/// a trap or a lifecycle winner keeps its own handling, so nothing is selected. Without a winner,
/// the error is the one of the invocation.
pub(crate) fn selected_recovery_error<'a>(
    owner_failure: Option<&'a crate::durable_host::tool::operation::OwnerFailureWinner>,
    agent_mode: AgentMode,
    result: &'a Result<InvokeResult, WorkerExecutorError>,
) -> Option<&'a WorkerExecutorError> {
    use crate::durable_host::tool::operation::OwnerFailureWinner;
    if agent_mode != AgentMode::Durable {
        return None;
    }
    let error = match owner_failure {
        Some(OwnerFailureWinner::Infrastructure(error)) => error,
        Some(OwnerFailureWinner::Trap(_) | OwnerFailureWinner::Lifecycle(_)) => return None,
        None => result.as_ref().err()?,
    };
    matches!(
        error,
        WorkerExecutorError::RecoveryRequired { .. } | WorkerExecutorError::OplogFailed { .. }
    )
    .then_some(error)
}

/// The error of a failed `prepare_instance`. A failure of the previous invocation and an
/// interrupt keep their form: the invocation loop handles an interrupt as an interrupted start and
/// records no recovery failure for it. Every other error is a failure to resume the agent.
pub(in crate::durable_host) fn prepare_failure(
    agent_id: &AgentId,
    error: WorkerExecutorError,
) -> WorkerExecutorError {
    match error {
        error @ (WorkerExecutorError::PreviousInvocationFailed { .. }
        | WorkerExecutorError::PreviousInvocationExited
        | WorkerExecutorError::Interrupted { .. }) => error,
        error => WorkerExecutorError::failed_to_resume_worker(agent_id.clone(), error),
    }
}
