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

use crate::worker::*;

impl<Ctx: WorkerCtx> Worker<Ctx> {
    /// Invocation entry point. Returns `Finished(...)` on read-only cache hit,
    /// otherwise `Pending(subscription)`. `Arc<Self>` is needed to spawn the
    /// detached observer that fills the read-only cache on completion.
    pub async fn invoke(
        self: Arc<Self>,
        invocation: AgentInvocation,
    ) -> Result<ResultOrSubscription, WorkerExecutorError> {
        if !matches!(invocation, AgentInvocation::ExternalTool { .. }) {
            self.ensure_component_agent_ingress("regular invocation")?;
        }
        let idempotency_key = Self::require_idempotency_key(&invocation)?;

        // Classification uses the in-memory component snapshot - no metadata
        // fetch on the hot path.
        let read_only_ctx = self.read_only_context_for(&invocation);

        let effect = if read_only_ctx.is_some() {
            read_only_cache::InvocationEffect::ReadOnly
        } else {
            read_only_cache::InvocationEffect::Mutating
        };

        // Cache HIT: still refuse on deleting / startup-failed worker.
        if let Some(ro) = &read_only_ctx {
            let no_cache = !ro.cacheable || is_no_cache(&ro.cfg.cache_policy);
            if !no_cache {
                let cur_epoch = self.read_only_cache_epoch.load(Ordering::SeqCst);
                let principal_ref = if ro.cfg.uses_principal {
                    Some(&ro.principal)
                } else {
                    None
                };
                let key = read_only_cache::build_read_only_cache_key(
                    &ro.method_name,
                    &ro.input,
                    principal_ref,
                    ro.component_revision,
                    cur_epoch,
                );
                if let Some(entry) = self.read_only_cache.try_get(&key).await {
                    if !entry.is_expired(tokio::time::Instant::now()) {
                        let instance_guard = self.lock_non_stopping_worker().await;
                        if instance_guard.ensure_not_deleting().is_err() {
                            return Err(WorkerExecutorError::invalid_request(
                                "Cannot enqueue invocation to a deleting worker",
                            ));
                        }
                        if let Some(err) = instance_guard.startup_failure() {
                            return Err(err.clone());
                        }
                        drop(instance_guard);
                        return Ok(ResultOrSubscription::Finished(Ok(entry.output.clone())));
                    } else {
                        // Only evict if the stored entry is still ours.
                        let me = entry.clone();
                        let _ = self
                            .read_only_cache
                            .remove_if_cached(&key, move |current| Arc::ptr_eq(current, &me))
                            .await;
                    }
                }
            }
        }

        // Subscribe before enqueue/lookup to avoid missing the completion event.
        let subscription = self.events().subscribe();
        let observer_sub = if let Some(ro) = &read_only_ctx {
            if !ro.cacheable || is_no_cache(&ro.cfg.cache_policy) {
                None
            } else {
                Some(self.events().subscribe())
            }
        } else {
            None
        };

        let output = async { self.lookup_invocation_result(&idempotency_key).await }
            .instrument(span!(
                Level::INFO,
                "lookup_invocation_result",
                agent_id = %self.owned_agent_id.agent_id,
                idempotency_key = %idempotency_key,
            ))
            .await;
        let (result, enqueue_epoch) = match output {
            LookupResult::Complete(output) => (ResultOrSubscription::Finished(output), None),
            LookupResult::Interrupted => {
                return Err(InterruptKind::Interrupt(Timestamp::now_utc()).into());
            }
            LookupResult::Pending => (ResultOrSubscription::Pending(subscription), None),
            LookupResult::New => {
                if let AgentInvocation::AgentMethod {
                    scope_card: Some(scope_card),
                    ..
                } = &invocation
                {
                    crate::services::card::validate_scope_card(
                        self.card_service().as_ref(),
                        scope_card,
                    )
                    .await?;
                }
                // For ReadOnly the helper returns the epoch captured under the
                // enqueue lock; using any other epoch could store stale data.
                let captured = self
                    .enqueue_worker_invocation_with_effect(invocation, effect)
                    .await?
                    .0;
                (ResultOrSubscription::Pending(subscription), captured)
            }
        };

        // Only populate the cache when this call owns the enqueue (only then
        // do we have a valid epoch).
        if let Some(ro) = read_only_ctx
            && let (Some(mut obs_sub), Some(epoch)) = (observer_sub, enqueue_epoch)
        {
            // Do not capture `Arc<Self>` - a never-completing invocation would
            // otherwise pin the worker.
            let cache = self.read_only_cache.clone();
            // The observer task does a final epoch recheck before insert
            // (`populate_read_only_cache`), so it needs the live atomic.
            let read_only_cache_epoch = self.read_only_cache_epoch.clone();
            let agent_id = self.owned_agent_id.agent_id.clone();
            let idem = idempotency_key.clone();
            tokio::spawn(async move {
                let wait_result = obs_sub
                    .wait_for(|event| match event {
                        Event::InvocationCompleted {
                            agent_id: ev_agent,
                            idempotency_key,
                            result,
                        } if *ev_agent == agent_id && *idempotency_key == idem => {
                            Some(result.clone())
                        }
                        _ => None,
                    })
                    .await;
                if let Ok(result) = wait_result
                    && let Ok(output) = *result
                    && matches!(output.result, AgentInvocationResult::AgentMethod { .. })
                {
                    populate_read_only_cache(&cache, &read_only_cache_epoch, &ro, epoch, output)
                        .await;
                }
            });
        }

        Ok(result)
    }
    /// Admits a native external-tool invocation for this exact owner.
    ///
    /// The caller is trusted to have authenticated the supplied principal and context at its
    /// ingress boundary. This method preserves ordinary invocation queue, idempotency, scope-card,
    /// result, and cache-invalidation semantics; it does not establish a separate permission
    /// boundary.
    pub async fn invoke_external_tool(
        self: Arc<Self>,
        expected_fingerprint: AgentFingerprint,
        idempotency_key: IdempotencyKey,
        tool_name: ToolName,
        command_path: Vec<String>,
        input: TypedSchemaValue,
        invocation_context: InvocationContextStack,
        principal: Principal,
        scope_card: Option<golem_common::model::card::ScopeCard>,
    ) -> Result<AgentInvocationOutput, WorkerExecutorError> {
        if self.initial_worker_metadata.fingerprint != expected_fingerprint {
            return Err(WorkerExecutorError::invalid_request(
                "external tool invocation owner fingerprint does not match",
            ));
        }

        // A retry attaches to the already accepted payload/result. In particular it must not
        // resolve the tool against a newer environment deployment.
        if self.lookup_invocation_result(&idempotency_key).await != LookupResult::New {
            return self.await_enqueued_invocation(idempotency_key).await;
        }

        let invocation = self
            .prepare_external_tool_invocation(
                idempotency_key,
                tool_name,
                command_path,
                input,
                false,
                false,
                false,
                None,
                invocation_context,
                principal,
                scope_card,
            )
            .await?;
        self.invoke_and_await(invocation).await
    }
    pub(crate) async fn prepare_external_tool_invocation(
        &self,
        idempotency_key: IdempotencyKey,
        tool_name: ToolName,
        command_path: Vec<String>,
        input: TypedSchemaValue,
        stdin: bool,
        stdout: bool,
        stderr: bool,
        expected_deployment_revision: Option<DeploymentRevision>,
        invocation_context: InvocationContextStack,
        principal: Principal,
        scope_card: Option<golem_common::model::card::ScopeCard>,
    ) -> Result<AgentInvocation, WorkerExecutorError> {
        if let Some(scope_card) = &scope_card {
            crate::services::card::validate_scope_card(self.card_service().as_ref(), scope_card)
                .await?;
        }
        let accepted_index = self
            .durable_stream_session_status(&idempotency_key)
            .await?
            .and_then(|status| status.initial_pending_invocation_oplog_index);
        let activation = if let Some(index) = accepted_index {
            let OplogEntry::PendingAgentInvocation {
                idempotency_key: accepted_key,
                payload,
                ..
            } = self.oplog.read(index).await?
            else {
                return Err(WorkerExecutorError::runtime(
                    "native session does not reference a pending invocation",
                ));
            };
            if accepted_key != idempotency_key {
                return Err(WorkerExecutorError::runtime(
                    "native session references a different invocation key",
                ));
            }
            let payload: AgentInvocationPayload = self
                .oplog
                .download_payload(payload)
                .await
                .map_err(WorkerExecutorError::runtime)?;
            let AgentInvocationPayload::ExternalTool { activation, .. } = payload else {
                return Err(WorkerExecutorError::invalid_request(
                    "invocation key is already bound to an agent method",
                ));
            };
            activation
        } else {
            // The resident component snapshot starts at the CREATE revision and is only refreshed by
            // instance startup. Admission can happen while the owner is cold, so resolve against the
            // revision folded from the authoritative oplog status instead.
            let component_revision = self.state_actor.status().await.component_revision;
            let component = self
                .component_service()
                .get_metadata(self.owned_agent_id.component_id(), Some(component_revision))
                .await?;
            let owner = match &self.owner_context {
                ResolvedOwnerContext::Agent(agent) => ToolBindingOwner::AgentType {
                    agent_type_name: agent.agent_type.clone(),
                },
                ResolvedOwnerContext::ComponentWorker | ResolvedOwnerContext::ComponentBaseline => {
                    ToolBindingOwner::ComponentBaseline {
                        component_id: component.id,
                    }
                }
            };
            match self
                .environment_state_service()
                .get_tool_activation(
                    self.owned_agent_id.environment_id,
                    component.id,
                    component_revision,
                    &owner,
                    &tool_name,
                )
                .await
                .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?
            {
                crate::services::environment_state::ToolActivationOutcome::Ready(activation) => {
                    activation
                }
                crate::services::environment_state::ToolActivationOutcome::NotBound => {
                    return Err(WorkerExecutorError::permission_denied(format!(
                        "tool '{tool_name}' is not bound to owner '{owner:?}'"
                    )));
                }
                crate::services::environment_state::ToolActivationOutcome::NotRegistered => {
                    return Err(WorkerExecutorError::invalid_request(format!(
                        "tool '{tool_name}' is not registered"
                    )));
                }
            }
        };
        require_expected_tool_deployment_revision(
            expected_deployment_revision,
            activation.registered_tool.deployment_revision,
        )?;
        Ok(AgentInvocation::ExternalTool {
            idempotency_key,
            tool_name,
            command_path,
            input: Box::new(input),
            stdin,
            stdout,
            stderr,
            activation,
            invocation_context,
            principal,
            scope_card,
        })
    }
    /// Invokes the worker and awaits for a result.
    ///
    /// For cacheable read-only `AgentMethod` invocations, concurrent Await
    /// misses for the same `ReadOnlyCacheKey` are *coalesced* via
    /// [`golem_common::cache::Cache::get_or_insert_simple`] — only the first
    /// caller runs the underlying invocation and populates the cache; later
    /// concurrent callers receive the same result without re-enqueueing.
    ///
    /// Coalescing is intentionally scoped to the Await path. Fire-and-forget
    /// (`invoke`) callers must return immediately, so they do not block on
    /// pending entries and continue to use the detached observer to populate
    /// the cache. The unified key shape means an Await coalesce and a
    /// fire-and-forget observer can race; both produce the same
    /// [`ReadOnlyCacheEntry`] from the same output, so the race is benign.
    pub async fn invoke_and_await(
        self: Arc<Self>,
        invocation: AgentInvocation,
    ) -> Result<AgentInvocationOutput, WorkerExecutorError> {
        self.invoke_and_await_with_notifier(invocation, None).await
    }
    pub(crate) async fn invoke_and_await_with_notifier(
        self: Arc<Self>,
        invocation: AgentInvocation,
        mut admission: Option<tokio::sync::oneshot::Sender<read_only_cache::AdmissionSignal>>,
    ) -> Result<AgentInvocationOutput, WorkerExecutorError> {
        let idempotency_key = Self::require_idempotency_key(&invocation)?;

        // Fast path: read-only Await coalescing.
        //
        // Coalescing is only safe for genuinely new invocations.
        // Idempotency replay (`lookup_invocation_result` returns `Complete`)
        // must return the result that was recorded under whatever epoch the
        // original invocation ran in — so coalescing the call (and caching
        // its result under the current epoch's `ReadOnlyCacheKey`) would
        // poison the cache. `Pending` means another caller is responsible
        // for completing the invocation, so we just await the existing
        // result instead of enqueueing a duplicate or caching here.
        //
        // For non-`New` results we MUST NOT fall through to
        // `Worker::invoke`: that path checks the read-only cache HIT before
        // looking up the idempotency key, which would let a warm
        // current-epoch entry shadow the recorded idempotency result.
        // Instead we handle non-`New` results inline below.
        let lookup_for_coalesce = if let Some(ro) = self.read_only_context_for(&invocation)
            && ro.cacheable
            && !is_no_cache(&ro.cfg.cache_policy)
        {
            Some((
                ro,
                async { self.lookup_invocation_result(&idempotency_key).await }
                    .instrument(span!(
                        Level::INFO,
                        "lookup_invocation_result",
                        agent_id = %self.owned_agent_id.agent_id,
                        idempotency_key = %idempotency_key,
                    ))
                    .await,
            ))
        } else {
            None
        };

        match lookup_for_coalesce {
            Some((_, LookupResult::Complete(Ok(output)))) => {
                if let Some(tx) = admission.take() {
                    let _ = tx.send(read_only_cache::AdmissionSignal::Immediate);
                }
                return Ok(output);
            }
            Some((_, LookupResult::Complete(Err(err)))) => {
                if let Some(tx) = admission.take() {
                    let _ = tx.send(read_only_cache::AdmissionSignal::Failure(err.clone()));
                }
                return Err(err);
            }
            Some((_, LookupResult::Interrupted)) => {
                return Err(InterruptKind::Interrupt(Timestamp::now_utc()).into());
            }
            Some((_, LookupResult::Pending)) => {
                // Another caller already enqueued this idempotency key. Wait
                // for its result without going through `Worker::invoke` (so a
                // current-epoch read-only cache HIT cannot shadow the
                // recorded idempotency result), and do not populate the
                // read-only cache here.
                let subscription = self.events().subscribe();
                Worker::start_if_needed(self.clone()).await?;
                if let Some(tx) = admission.take() {
                    let _ = tx.send(read_only_cache::AdmissionSignal::Queued);
                }
                let result = self
                    .wait_for_invocation_result(&idempotency_key, subscription)
                    .await;
                return match result {
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
                };
            }
            _ => {}
        }

        if let Some((ro, LookupResult::New)) = lookup_for_coalesce {
            // Use the same key shape as the `invoke` cache HIT path so a hit
            // there and a coalesced miss here see the same entry.
            let cur_epoch = self.read_only_cache_epoch.load(Ordering::SeqCst);
            let principal_ref = if ro.cfg.uses_principal {
                Some(&ro.principal)
            } else {
                None
            };
            let key = read_only_cache::build_read_only_cache_key(
                &ro.method_name,
                &ro.input,
                principal_ref,
                ro.component_revision,
                cur_epoch,
            );

            // Honor TTL up front: a stale entry must miss, not hit (mirrors
            // the `Worker::invoke` HIT path).
            if let Some(entry) = self.read_only_cache.try_get(&key).await {
                if !entry.is_expired(tokio::time::Instant::now()) {
                    // Apply the same `is_deleting` / `startup_failure` guard
                    // the HIT path in `invoke` applies, so we don't return a
                    // cached value for a worker that's about to disappear.
                    let instance_guard = self.lock_non_stopping_worker().await;
                    if instance_guard.ensure_not_deleting().is_err() {
                        return Err(WorkerExecutorError::invalid_request(
                            "Cannot enqueue invocation to a deleting worker",
                        ));
                    }
                    if let Some(err) = instance_guard.startup_failure() {
                        return Err(err.clone());
                    }
                    drop(instance_guard);
                    if let Some(tx) = admission.take() {
                        let _ = tx.send(read_only_cache::AdmissionSignal::Immediate);
                    }
                    return Ok(entry.output.clone());
                } else {
                    let me = entry.clone();
                    let _ = self
                        .read_only_cache
                        .remove_if_cached(&key, move |current| Arc::ptr_eq(current, &me))
                        .await;
                }
            }

            // Coalesce concurrent first-time misses for this key. Only the
            // first caller spawns the underlying invocation; subsequent
            // concurrent callers wait on the same pending entry inside
            // `get_or_insert_simple_spawned` and receive the same
            // `ReadOnlyCacheEntry`.
            //
            // The spawned closure runs `invoke_and_await_uncoalesced` to
            // bypass this coalescing path. Returning Err removes the pending
            // entry so a later caller retries (failures must not poison the
            // cache).
            //
            // The closure is spawned via `tokio::task::spawn` (see
            // [`Cache::get_or_insert_spawned`]) so that cancellation of any
            // single Await caller does NOT leave the pending entry stuck
            // forever — the spawned owner future survives caller drop and
            // resolves the entry one way or the other.
            let ro_for_closure = ro.clone();
            let worker = self.clone();
            let invocation_for_closure = invocation;
            let idem_for_closure = idempotency_key.clone();

            // The owner future is spawned and deliberately survives this caller, so
            // it cannot run inside the caller's span - that would hold the gRPC span
            // open for the whole invocation. It links back instead, so the coalesced
            // execution is still reachable from the request that started it.
            //
            // Captured before the wait span rather than inside it, unlike
            // `enqueue_worker_invocation_with_effect`: N callers coalesce onto one
            // execution, so linking to any one
            // caller's wait would pick an arbitrary winner. The request that owns the
            // execution is the honest originator.
            let origin = TraceOrigin::capture_current();
            let owner_agent_id = self.owned_agent_id.agent_id.clone();
            let owner_idempotency_key = idempotency_key.clone();

            let pending_or_final = async {
                self.read_only_cache
                    .get_or_insert_pending(
                        &key,
                        move || {
                            read_only_cache::PendingReadOnlyCacheEntry::Invocation(
                                read_only_cache::PendingReadOnlyInvocation::new(),
                            )
                        },
                        move |pending| {
                            let read_only_cache::PendingReadOnlyCacheEntry::Invocation(pending) = pending.clone() else {
                                unreachable!("coalesced invocation created an immediate pending entry")
                            };
                        let span = related_span!(
                            origin,
                            Level::INFO,
                            "read_only_invocation",
                            agent_id = %owner_agent_id,
                            idempotency_key = %owner_idempotency_key,
                        );
                            Box::pin(async move {
                            let output = Worker::invoke_and_await_uncoalesced_admitted(
                                worker,
                                invocation_for_closure,
                                idem_for_closure,
                                Some(pending.clone()),
                            )
                            .await.inspect_err(|error| {
                                pending.publish(read_only_cache::AdmissionSignal::Failure(error.clone()));
                            })?;
                            if !matches!(output.result, AgentInvocationResult::AgentMethod { .. }) {
                                // Defensive: only `AgentMethod` outputs are cacheable.
                                return Err(WorkerExecutorError::unknown(
                                    "read-only invocation produced a non-AgentMethod result",
                                ));
                            }
                            Ok(build_read_only_cache_entry(&ro_for_closure, output))
                            }.instrument(span))
                        },
                    )
                    .await
            }
            // The caller's own share: however long it waits for the coalesced
            // execution, whether or not it is the one that owns it.
            .instrument(span!(
                Level::INFO,
                "wait_for_read_only_invocation",
                agent_id = %self.owned_agent_id.agent_id,
                idempotency_key = %idempotency_key,
            ))
            .await;
            let entry_result = match pending_or_final {
                Ok(entry) => match entry {
                    PendingOrFinal::Final(entry) => {
                        if let Some(tx) = admission.take() {
                            let _ = tx.send(read_only_cache::AdmissionSignal::Immediate);
                        }
                        Ok(entry)
                    }
                    PendingOrFinal::Pending(mut pending) => {
                        let pending_invocation = match &pending.value {
                            read_only_cache::PendingReadOnlyCacheEntry::Immediate => None,
                            read_only_cache::PendingReadOnlyCacheEntry::Invocation(value) => {
                                Some(value.clone())
                            }
                        };
                        let signal = match &pending_invocation {
                            Some(value) => value.admission().await,
                            None => read_only_cache::AdmissionSignal::Immediate,
                        };
                        if let Some(tx) = admission.take() {
                            let _ = tx.send(signal.clone());
                        }
                        if let read_only_cache::AdmissionSignal::Failure(error) = signal {
                            return Err(error);
                        }

                        pending
                            .completion
                            .wait_for(|value| value.is_some())
                            .await
                            .map_err(|_| {
                                WorkerExecutorError::runtime("read-only owner disappeared")
                            })?
                            .clone()
                            .expect("completion watch was observed as Some")
                    }
                },
                Err(error) => Err(error),
            };

            // Stale-populate guard: if the epoch bumped while the owner ran,
            // the entry we just inserted is keyed on the old epoch and is
            // already unreachable for any future lookup. We could leave it
            // for the LRU; explicitly removing it keeps the cache tidy.
            if self.read_only_cache_epoch.load(Ordering::SeqCst) != cur_epoch
                && let Ok(entry) = &entry_result
            {
                let me = entry.clone();
                let _ = self
                    .read_only_cache
                    .remove_if_cached(&key, move |current| Arc::ptr_eq(current, &me))
                    .await;
            }

            return entry_result.map(|entry| entry.output.clone());
        }

        // Non-cacheable path: `NoCache` read-only methods and all
        // non-read-only invocations skip coalescing entirely.
        let pending = admission
            .as_ref()
            .map(|_| read_only_cache::PendingReadOnlyInvocation::new());
        let admission_wait = pending.clone();
        let result = self
            .invoke_owned_prefix(invocation, pending)
            .await
            .map_err(|error| WorkerExecutorError::runtime(error.to_string()))??;
        if let Some(tx) = admission.take() {
            let signal = admission_wait
                .expect("admission pending exists when notifier exists")
                .admission()
                .await;
            let _ = tx.send(signal);
        }
        self.await_invocation_result(idempotency_key, result).await
    }
    pub(crate) async fn invoke_and_start(
        self: Arc<Self>,
        invocation: AgentInvocation,
    ) -> Result<ResultOrSubscription, WorkerExecutorError> {
        self.invoke_owned_prefix(invocation, None)
            .await
            .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?
    }
    pub(in crate::worker) fn invoke_owned_prefix(
        self: &Arc<Self>,
        invocation: AgentInvocation,
        admission: Option<read_only_cache::PendingReadOnlyInvocation>,
    ) -> tasks::FinishOnDrop<'_, Result<ResultOrSubscription, WorkerExecutorError>> {
        let worker = self.clone();
        self.tasks.finish_on_drop(tokio::spawn(async move {
            let result = match worker.clone().invoke(invocation).await {
                Ok(result) => result,
                Err(error) => {
                    if let Some(admission) = &admission {
                        admission.publish(read_only_cache::AdmissionSignal::Failure(error.clone()));
                    }
                    return Err(error);
                }
            };
            if matches!(result, ResultOrSubscription::Pending(_)) {
                if let Err(error) = Worker::start_if_needed(worker).await {
                    if let Some(admission) = &admission {
                        admission.publish(read_only_cache::AdmissionSignal::Failure(error.clone()));
                    }
                    return Err(error);
                }
                if let Some(admission) = &admission {
                    admission.publish(read_only_cache::AdmissionSignal::Queued);
                }
            } else if let Some(admission) = &admission {
                let signal = match &result {
                    ResultOrSubscription::Finished(Err(error)) => {
                        read_only_cache::AdmissionSignal::Failure(error.clone())
                    }
                    _ => read_only_cache::AdmissionSignal::Immediate,
                };
                admission.publish(signal);
            }
            Ok(result)
        }))
    }
    /// Underlying `invoke_and_await` implementation without read-only
    /// coalescing. Used directly for non-cacheable invocations and as the
    /// per-key owner future inside the coalesced path above.
    pub(in crate::worker) async fn invoke_and_await_uncoalesced_admitted(
        self: Arc<Self>,
        invocation: AgentInvocation,
        idempotency_key: IdempotencyKey,
        admission: Option<read_only_cache::PendingReadOnlyInvocation>,
    ) -> Result<AgentInvocationOutput, WorkerExecutorError> {
        let result = self
            .invoke_owned_prefix(invocation, admission)
            .await
            .map_err(|error| WorkerExecutorError::runtime(error.to_string()))??;
        self.await_invocation_result(idempotency_key, result).await
    }
    pub(crate) async fn await_enqueued_invocation(
        self: Arc<Self>,
        idempotency_key: IdempotencyKey,
    ) -> Result<AgentInvocationOutput, WorkerExecutorError> {
        let subscription = self.events().subscribe();
        let result = match self.lookup_invocation_result(&idempotency_key).await {
            LookupResult::Complete(result) => ResultOrSubscription::Finished(result),
            LookupResult::Pending => ResultOrSubscription::Pending(subscription),
            LookupResult::Interrupted => {
                return Err(InterruptKind::Interrupt(Timestamp::now_utc()).into());
            }
            LookupResult::New => {
                return Err(WorkerExecutorError::runtime(
                    "durable streaming acceptance did not leave its invocation pending",
                ));
            }
        };
        self.await_invocation_result(idempotency_key, result).await
    }
    pub(in crate::worker) fn require_idempotency_key(
        invocation: &AgentInvocation,
    ) -> Result<IdempotencyKey, WorkerExecutorError> {
        invocation.idempotency_key().cloned().ok_or_else(|| {
            WorkerExecutorError::invalid_request("Invocation has no idempotency key")
        })
    }
    pub(in crate::worker) fn ensure_component_agent_ingress(
        &self,
        operation: &str,
    ) -> Result<(), WorkerExecutorError> {
        if self.initial_worker_metadata.owner_kind != OwnerKind::ComponentAgent {
            return Err(WorkerExecutorError::invalid_request(format!(
                "{operation} is not supported for an external tool owner"
            )));
        }
        Ok(())
    }
    pub async fn pending_invocations(&self) -> Vec<PendingInvocationRef> {
        self.state_actor.status().await.pending_invocations.clone()
    }
    /// Reserved constructor key, also used to recognize initialization before inspecting files.
    pub(in crate::worker) fn initialization_idempotency_key(&self) -> IdempotencyKey {
        IdempotencyKey::new(format!("init-{}", self.agent_id()))
    }
    /// Reads the `PendingAgentInvocation` oplog entry referenced by `pending` and reconstructs the
    /// full invocation, downloading its payload from external storage if needed. The status record
    /// only keeps a lightweight reference, so callers that need to execute the invocation hydrate
    /// it on demand.
    ///
    /// A storage that did not answer gives `RecoveryRequired`. An entry or payload that does not
    /// decode, or another entry at the referenced index, gives `OplogFailed` for this agent.
    pub(in crate::worker) async fn hydrate_pending_invocation(
        &self,
        pending: &PendingInvocationRef,
    ) -> Result<HydratedInvocation, WorkerExecutorError> {
        let entry = self.oplog.read(pending.oplog_index).await?;
        match entry {
            OplogEntry::PendingAgentInvocation {
                idempotency_key,
                payload,
                trace_id,
                trace_states,
                invocation_context,
                ..
            } => {
                let agent_payload = self
                    .oplog
                    .download_payload_classified(payload.clone())
                    .await
                    .map_err(|error| match error {
                        OplogPayloadDownloadError::Backend(error) => {
                            WorkerExecutorError::recovery_required(format!(
                                "failed to download the pending agent invocation payload at oplog index {}: {error:#}",
                                pending.oplog_index
                            ))
                        }
                        OplogPayloadDownloadError::Corrupt(error) => {
                            WorkerExecutorError::oplog_failed(
                                self.agent_id(),
                                format!(
                                    "corrupt pending agent invocation payload at oplog index {}: {error:#}",
                                    pending.oplog_index
                                ),
                            )
                        }
                    })?;
                let invocation_context = InvocationContextStack::from_oplog_data(
                    trace_id,
                    trace_states,
                    invocation_context,
                );
                let invocation =
                    AgentInvocation::from_parts(idempotency_key, agent_payload, invocation_context);
                Ok(HydratedInvocation {
                    invocation,
                    payload,
                })
            }
            other => Err(WorkerExecutorError::oplog_failed(
                self.agent_id(),
                format!(
                    "expected a PendingAgentInvocation oplog entry at index {}, but found {other:?}",
                    pending.oplog_index
                ),
            )),
        }
    }
    /// Enqueue invocation of an exported function. Uses
    /// `UnknownAssumeMutating` as a safe default for callers without
    /// classification; the epoch is no longer bumped at enqueue time.
    pub(in crate::worker) async fn enqueue_worker_invocation(
        &self,
        invocation: AgentInvocation,
    ) -> Result<OplogIndex, WorkerExecutorError> {
        self.enqueue_worker_invocation_with_effect(
            invocation,
            read_only_cache::InvocationEffect::UnknownAssumeMutating,
        )
        .await
        .map(|(_, oplog_index)| {
            oplog_index.expect("keyless manual update admission always appends an oplog entry")
        })
    }
    pub(in crate::worker) async fn pending_invocation_entry(
        &self,
        oplog: &Arc<dyn Oplog>,
        invocation: AgentInvocation,
    ) -> Result<(Option<IdempotencyKey>, OplogEntry), WorkerExecutorError> {
        let (semantic_key, idempotency_key, invocation_payload, invocation_context) =
            into_pending_invocation_parts(invocation);
        let invocation_context = invocation_context
            .limit_depth(self.deps.config().limits.max_invocation_context_stack_depth);
        let payload = match invocation_payload {
            // A manual update invocation stays in the oplog whatever the payload limit, so every
            // reader of the oplog tells it from other invocations without a download.
            payload @ AgentInvocationPayload::ManualUpdate { .. } => {
                OplogPayload::SerializedInline {
                    bytes: golem_common::serialization::serialize(&payload).map_err(|e| {
                        WorkerExecutorError::invalid_request(format!(
                            "Failed to serialize invocation payload: {e}"
                        ))
                    })?,
                    cached: Some(Arc::new(payload)),
                }
            }
            payload => oplog.upload_payload_owned(payload).await.map_err(|e| {
                WorkerExecutorError::invalid_request(format!(
                    "Failed to upload invocation payload: {e}"
                ))
            })?,
        };
        let invocation_context_spans = invocation_context.to_oplog_data();
        Ok((
            semantic_key,
            OplogEntry::pending_agent_invocation(
                idempotency_key,
                payload,
                invocation_context.trace_id,
                invocation_context.trace_states,
                invocation_context_spans,
            ),
        ))
    }
    /// Enqueue invocation, classified by the caller. Passing `ReadOnly` for a
    /// mutating method would skip cache invalidation and produce stale reads.
    ///
    /// For `ReadOnly`, returns the epoch captured before admission. Populating
    /// the cache later must use this captured epoch, not the current one, to
    /// avoid storing a stale result under a post-mutation epoch.
    pub(crate) async fn enqueue_worker_invocation_with_effect(
        &self,
        invocation: AgentInvocation,
        read_only_cache_effect: read_only_cache::InvocationEffect,
    ) -> Result<(Option<u64>, Option<OplogIndex>), WorkerExecutorError> {
        // Carried on the span so the two sides of the hand-off share a searchable
        // key: the execution runs in its own trace, linked rather than nested, so
        // `idempotency_key` is what joins them. Left unset rather than empty when
        // there is none, so a search for one key cannot collide with every
        // keyless enqueue.
        let span = span!(
            Level::INFO,
            "enqueue_invocation",
            agent_id = %self.owned_agent_id.agent_id,
            idempotency_key = tracing::field::Empty,
            // Pairs with the `consumer` span the invocation loop creates when it
            // picks the work up.
            otel.kind = "producer"
        );
        if let Some(idempotency_key) = invocation.idempotency_key() {
            span.record("idempotency_key", tracing::field::display(idempotency_key));
        }

        async {
            self.accept_ephemeral_invocation(&invocation)?;
            let instance_guard = self.lock_non_stopping_worker().await;

            if instance_guard.ensure_not_deleting().is_err() {
                return Err(WorkerExecutorError::invalid_request(
                    "Cannot enqueue invocation to a deleting worker",
                ));
            };
            if self.owner_retirement_requested.is_cancelled() {
                return Err(self.retirement_error());
            }

            if let Some(err) = instance_guard.startup_failure() {
                if self.agent_mode() == AgentMode::Ephemeral {
                    crate::metrics::ephemeral::record_inactive_invocation_failure();
                }
                return Err(err.clone());
            }

            if let Some(idempotency_key) = invocation.idempotency_key()
                && self.lookup_invocation_result(idempotency_key).await != LookupResult::New
            {
                return Ok((None, None));
            }

            let (semantic_idempotency_key, entry) = self
                .pending_invocation_entry(&self.oplog, invocation)
                .await?;

            // Snapshot the epoch for a later read-only cache fill. Keyed admission releases and
            // reacquires the worker lifecycle lock below; that staleness is safe because
            // `populate_read_only_cache` rechecks the epoch before publishing the result. Mutating
            // invocations no longer bump here — the bump happens on *successful completion* in
            // `DurableWorkerCtx::on_agent_invocation_success`, so a cached
            // read-only result stays serviceable while the mutation is queued
            // or running.
            let read_only_epoch_snapshot = match read_only_cache_effect {
                read_only_cache::InvocationEffect::ReadOnly => {
                    Some(self.read_only_cache_epoch.load(Ordering::SeqCst))
                }
                read_only_cache::InvocationEffect::Mutating
                | read_only_cache::InvocationEffect::UnknownAssumeMutating => None,
            };

            let mut caller_instance_guard = Some(instance_guard);
            let admission_index = if let Some(idempotency_key) = semantic_idempotency_key.as_ref() {
                drop(caller_instance_guard.take());
                loop {
                    let status = self.state_actor.status().await;
                    if self.lookup_invocation_result(idempotency_key).await != LookupResult::New {
                        return Ok((None, None));
                    }
                    let current = self.last_known_status.load();
                    if current.invocation_results.change_generation()
                        != status.invocation_results.change_generation()
                        || current.invocation_results.revert_generation()
                            != status.invocation_results.revert_generation()
                    {
                        continue;
                    }
                    drop(current);
                    let instance_guard = self.lock_non_stopping_worker_owned().await;
                    if instance_guard.ensure_not_deleting().is_err() {
                        return Err(WorkerExecutorError::invalid_request(
                            "Cannot enqueue invocation to a deleting worker",
                        ));
                    }
                    if self.owner_retirement_requested.is_cancelled() {
                        return Err(self.retirement_error());
                    }
                    if !self
                        .state_actor
                        .append_invocation_if_version(
                            entry.clone(),
                            idempotency_key.clone(),
                            status.invocation_results.change_generation(),
                            status.invocation_results.revert_generation(),
                            instance_guard,
                        )
                        .await?
                    {
                        continue;
                    }
                    break;
                }
                None
            } else {
                Some(
                    self.add_and_commit_oplog_internal(
                        caller_instance_guard.as_ref().unwrap(),
                        entry,
                        None,
                    )
                    .await?,
                )
            };

            if let Some(idempotency_key) = semantic_idempotency_key {
                // Captured here, inside the producer span, because a consumer links
                // back to the *creation context* of the work rather than to wherever
                // the caller happened to call from.
                // Captured before taking the lock: `capture_current` reaches into the
                // tracing registry, which has no business happening inside this map's
                // critical section. Same-worker enqueues are already serialized by the
                // instance guard held above, so this keeps the critical section
                // minimal rather than fixing measurable contention.
                let origin = TraceOrigin::capture_current();
                self.external_invocation_origins
                    .write()
                    .await
                    .insert(idempotency_key, origin);
            }

            if let Some(instance_guard) = caller_instance_guard.as_ref()
                && let WorkerInstance::Running(running) = &**instance_guard
            {
                running.sender.send(WorkerCommand::WorkAvailable).unwrap();
            };

            drop(caller_instance_guard);

            Ok((read_only_epoch_snapshot, admission_index))
        }
        .instrument(span)
        .await
    }
    pub(in crate::worker) fn accept_ephemeral_invocation(
        &self,
        invocation: &AgentInvocation,
    ) -> Result<(), WorkerExecutorError> {
        let idempotency_key = match invocation {
            AgentInvocation::AgentMethod {
                idempotency_key, ..
            }
            | AgentInvocation::ExternalTool {
                idempotency_key, ..
            } => idempotency_key,
            _ => return Ok(()),
        };
        if self.agent_mode() != AgentMode::Ephemeral {
            return Ok(());
        }

        let mut state = self.ephemeral_invocation.lock().unwrap();
        let result = state.accept(idempotency_key);
        if result.is_err() {
            crate::metrics::ephemeral::record_inactive_invocation_failure();
        }
        result
    }
    pub async fn get_file_system_node(
        self: &Arc<Self>,
        path: CanonicalFilePath,
    ) -> Result<GetFileSystemNodeResult, WorkerExecutorError> {
        let (sender, receiver) = oneshot::channel();
        self.enqueue_filesystem_request(QueuedWorkerInvocation::GetFileSystemNode { path, sender })
            .await?;
        receiver
            .await
            .map_err(|_| WorkerExecutorError::runtime("Filesystem inspection stopped"))?
    }
    pub(in crate::worker) async fn enqueue_filesystem_request(
        self: &Arc<Self>,
        invocation: QueuedWorkerInvocation,
    ) -> Result<(), WorkerExecutorError> {
        let instance_guard = self.lock_non_stopping_worker().await;

        if instance_guard.ensure_not_deleting().is_err() {
            return Err(WorkerExecutorError::invalid_request(
                "Cannot access filesystem of a deleting worker",
            ));
        };
        if self.owner_retirement_requested.is_cancelled() {
            return Err(self.retirement_error());
        }

        if let Some(err) = instance_guard.startup_failure() {
            return Err(err.clone());
        }

        let status = self.state_actor.try_status().await?;
        Self::ensure_not_failed(&self.deps, &self.owned_agent_id, self.agent_mode(), &status)
            .await?;

        let mut queue = self.queue.write().await;
        queue.retain(|invocation| !invocation.is_abandoned());
        queue.push_back(invocation);
        drop(queue);

        if let WorkerInstance::Running(running) = &*instance_guard {
            running.sender.send(WorkerCommand::WorkAvailable).unwrap();
        };

        let needs_start = matches!(*instance_guard, WorkerInstance::Unloaded { .. });
        drop(instance_guard);
        if needs_start {
            Self::start_if_needed(self.clone()).await?;
        }
        Ok(())
    }
    pub async fn get_wallet_cards(&self) -> Result<Vec<StoredCard>, WorkerExecutorError> {
        let instance_guard = self.lock_non_stopping_worker().await;

        if instance_guard.ensure_not_deleting().is_err() {
            return Err(WorkerExecutorError::invalid_request(
                "Cannot access wallet of a deleting worker",
            ));
        };

        if let Some(err) = instance_guard.startup_failure() {
            return Err(err.clone());
        }

        let (sender, receiver) = oneshot::channel();

        let mut queue = self.queue.write().await;
        queue.retain(|invocation| !invocation.is_abandoned());
        queue.push_back(QueuedWorkerInvocation::GetWalletCards { sender });
        drop(queue);

        if let WorkerInstance::Running(running) = &*instance_guard {
            running.sender.send(WorkerCommand::WorkAvailable).unwrap();
        };

        drop(instance_guard);

        let mut wallet = receiver.await.unwrap()?;
        let revoked_cards = self
            .get_last_known_status()
            .await
            .pending_card_events
            .iter()
            .filter_map(|pending_event| match &pending_event.event {
                QueuedCardEvent::Revoke(event) => Some(event.card_id),
                QueuedCardEvent::Install(_)
                | QueuedCardEvent::TransferStarted(_)
                | QueuedCardEvent::TransferReceived(_) => None,
            })
            .collect::<HashSet<_>>();
        wallet.retain(|card| !revoked_cards.contains(&card.card_id()));
        Ok(wallet)
    }
    pub async fn read_file(
        self: &Arc<Self>,
        path: CanonicalFilePath,
        selection: FileByteSelection,
    ) -> Result<FileReadResponse, FileReadError> {
        validate_file_read_path(path.as_abs_str())?;
        selection.validate()?;
        let (sender, receiver) = tokio::sync::oneshot::channel();
        self.enqueue_filesystem_request(QueuedWorkerInvocation::ReadFile {
            path,
            selection,
            sender,
        })
        .await
        .map_err(|_| FileReadError::Lifecycle)?;

        receiver.await.map_err(|_| FileReadError::Lifecycle)?
    }
    pub async fn await_ready_to_process_commands(&self) -> Result<(), WorkerExecutorError> {
        let instance_guard = self.lock_non_stopping_worker().await;

        if instance_guard.ensure_not_deleting().is_err() {
            return Err(WorkerExecutorError::invalid_request(
                "Cannot await readiness of a deleting worker",
            ));
        };

        if let Some(err) = instance_guard.startup_failure() {
            return Err(err.clone());
        }

        // An unloaded worker has no invocation loop that could drain a queued readiness marker,
        // and this method must not start one: the worker already reached a stopped state, which is
        // exactly the "not processing anything until the next explicit start" condition callers
        // wait for.
        if matches!(&*instance_guard, WorkerInstance::Unloaded { .. }) {
            return Ok(());
        }

        let (sender, receiver) = oneshot::channel();

        let mut queue = self.queue.write().await;
        queue.retain(|invocation| !invocation.is_abandoned());
        queue.push_back(QueuedWorkerInvocation::AwaitReadyToProcessCommands { sender });
        drop(queue);

        if let WorkerInstance::Running(running) = &*instance_guard {
            running.sender.send(WorkerCommand::WorkAvailable).unwrap();
        };

        drop(instance_guard);

        // The marker is resolved either by the running invocation loop once it becomes idle, or
        // by the stop transition when the worker stops without draining it (see
        // `resolve_pending_readiness_awaiters_on_stop`), so this wait cannot hang on a worker
        // that suspends mid-invocation.
        receiver.await.unwrap()
    }
}

pub(crate) fn require_expected_tool_deployment_revision(
    expected: Option<DeploymentRevision>,
    actual: DeploymentRevision,
) -> Result<(), WorkerExecutorError> {
    if expected.is_some_and(|expected| expected != actual) {
        Err(WorkerExecutorError::invalid_request(
            "external tool deployment revision does not match",
        ))
    } else {
        Ok(())
    }
}

pub(in crate::worker) fn into_pending_invocation_parts(
    invocation: AgentInvocation,
) -> (
    Option<IdempotencyKey>,
    IdempotencyKey,
    AgentInvocationPayload,
    InvocationContextStack,
) {
    let semantic_idempotency_key = invocation.idempotency_key().cloned();
    let (storage_idempotency_key, payload, invocation_context) = invocation.into_parts();
    (
        semantic_idempotency_key,
        storage_idempotency_key,
        payload,
        invocation_context,
    )
}

pub(in crate::worker) fn replace_invocation_input(
    mut invocation: AgentInvocation,
    replacement: TypedSchemaValue,
) -> AgentInvocation {
    match &mut invocation {
        AgentInvocation::AgentMethod { input, .. } => *input = replacement.into_parts().1,
        AgentInvocation::ExternalTool { input, .. } => {
            **input = replacement;
        }
        _ => {}
    }
    invocation
}

pub(in crate::worker) fn inactive_ephemeral_agent_error() -> WorkerExecutorError {
    WorkerExecutorError::invalid_request(INACTIVE_EPHEMERAL_AGENT_ERROR)
}

pub(in crate::worker) fn validate_resolved_invocation_identity(
    agent_mode: AgentMode,
    phantom_id: Option<Uuid>,
    idempotency_key: &IdempotencyKey,
    freshness_disposition: InvocationFreshnessDisposition,
) -> Result<(), WorkerExecutorError> {
    if agent_mode == AgentMode::Ephemeral && phantom_id.is_none() {
        return Err(WorkerExecutorError::invalid_request(
            "An ephemeral invocation requires a final phantom agent ID",
        ));
    }

    if freshness_disposition == InvocationFreshnessDisposition::MayExist {
        return Ok(());
    }

    if agent_mode != AgentMode::Ephemeral {
        return Err(WorkerExecutorError::invalid_request(
            "KnownFresh can only be used for an ephemeral agent invocation",
        ));
    }

    let expected_phantom_id = ephemeral_invocation_phantom_id(idempotency_key);
    if phantom_id != Some(expected_phantom_id) {
        return Err(WorkerExecutorError::invalid_request(
            "KnownFresh ephemeral agent id does not match the invocation idempotency key",
        ));
    }

    Ok(())
}

impl EphemeralInvocationState {
    pub(in crate::worker) fn accept(
        &mut self,
        idempotency_key: &IdempotencyKey,
    ) -> Result<(), WorkerExecutorError> {
        match self {
            Self::Available => {
                *self = Self::Accepted(Some(idempotency_key.clone()));
                Ok(())
            }
            Self::Accepted(Some(accepted_key)) if accepted_key == idempotency_key => Ok(()),
            Self::Accepted(_) => Err(inactive_ephemeral_agent_error()),
        }
    }
}
