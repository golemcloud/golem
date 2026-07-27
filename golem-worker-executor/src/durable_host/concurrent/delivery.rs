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

/// Everything needed to append a `CompletionDiscarded` marker from an owned task when an armed
/// [`AccessTerminalGuard`] is dropped: the guest tore the accessor completion future *after* the
/// successful `End` append was handed to its owned task, so the response was persisted but never
/// delivered. Armed only on the `End` path ([`CallHandle::persist_access_terminal`]) — never for
/// cancellations — and explicitly suppressed on internal-error returns the caller observes
/// ([`AccessTerminalGuard::suppress_discard_marker`]), so a marker is recorded exactly when the
/// completion was silently discarded by the guest.
pub(super) struct DiscardMarker {
    pub(super) start_idx: OplogIndex,
    pub(super) oplog: Arc<dyn Oplog>,
    pub(super) replay_state: ReplayState,
}

impl DiscardMarker {
    /// Appends the `CompletionDiscarded` marker entry and records it in the replay state. The
    /// terminal `End` append (and any ordered post-`End` append, e.g. a `FinishSpan`) must
    /// already be durable when this runs.
    async fn append(self) {
        let marker_idx = self
            .oplog
            .add(OplogEntry::CompletionDiscarded {
                timestamp: Timestamp::now_utc(),
                start_index: self.start_idx,
            })
            .await;
        self.replay_state
            .record_discarded_completion(self.start_idx, marker_idx);
    }

    /// Spawns the owned marker-recording task, chained after the (possibly still pending) owned
    /// terminal `End` append. Returns the chained handle, which replaces the terminal handle in
    /// the emitted [`DropEvent::CleanupAfterTerminal`], so every existing drain that joins the
    /// terminal also awaits the marker append — invocation completion cannot overtake it. The
    /// task is detached (`tokio::spawn`): even if the drain future itself is torn and the event
    /// is lost, the marker append still runs to completion.
    pub(super) fn spawn_chained(
        self,
        terminal: Option<tokio::task::JoinHandle<Result<(), WorkerExecutorError>>>,
    ) -> tokio::task::JoinHandle<Result<(), WorkerExecutorError>> {
        tokio::spawn(async move {
            if let Some(handle) = terminal {
                handle.await.map_err(|err| {
                    WorkerExecutorError::runtime(format!(
                        "durable call terminal recorder task failed: {err}"
                    ))
                })??;
            }
            self.append().await;
            Ok(())
        })
    }
}

/// A deferred guest-delivery token returned by [`CallHandle::complete_access_deferred`] /
/// [`CallHandle::replay_access_deferred`] for call sites whose result crosses one more fallible
/// boundary *after* the durable terminal is recorded — a second-stage channel send to the guest
/// task, a span finish plus resource-state transition before the host method returns, or a wire
/// conversion. The plain `complete_access` boundary (the accessor terminal itself) is too early
/// for those sites: the guest can silently discard the persisted completion between the `End`
/// and the real delivery, which replay would otherwise deliver.
///
/// Live, the token stays armed after the `End` is persisted and the durable scope is closed:
/// - [`Self::delivered`] — the final guest-facing transfer succeeded; no marker.
/// - [`Self::suppress`] — a post-`End` error is observed by the caller (the worker traps); no
///   marker.
/// - [`Self::discarded`] — the caller detected a silent discard (e.g. the guest dropped the
///   receiving end of the delivery channel); appends exactly one `CompletionDiscarded` marker
///   inline and returns once it is durable.
/// - `Drop` while armed — the delivering future itself was torn; spawns exactly one owned marker
///   append (ordered after any pending [`Self::append_ordered`] entry) and hands its join plus
///   the in-flight [`LiveCallPermit`] to the drain queue via [`DropEvent::AwaitDiscardMarker`],
///   so invocation settlement cannot overtake the append.
///
/// On replay the token mirrors the recorded delivery status: [`Self::is_replay_discarded`]
/// reports whether the recorded run discarded the completion, in which case the caller must not
/// deliver — it performs its deterministic post-`End` continuation (span consumption, terminal
/// bookkeeping) and parks at the exact point where live delivery would have happened. All token
/// operations are no-ops on replay.
pub struct CompletionDelivery {
    pub(super) state: CompletionDeliveryState,
}

pub(super) enum CompletionDeliveryState {
    /// Live, armed: the `End` is persisted and a torn/failed delivery must record a marker.
    Live(Box<LiveDelivery>),
    /// Live, but the call was not persisted (snapshotting): nothing to reconcile.
    Unarmed,
    /// Replay of a normally delivered completion.
    ReplayDelivered,
    /// Replay of a recorded discarded completion: the caller must not deliver and parks at the
    /// delivery boundary.
    ReplayDiscarded,
    /// Consumed (`delivered`/`suppress`/`discarded`).
    Done,
}

pub(super) struct LiveDelivery {
    pub(super) marker: DiscardMarker,
    pub(super) trap_context: DurableCallTrapContext,
    /// Keeps the call counted as in flight (for positional-boundary and snapshot checks) until
    /// the token is consumed or its drain event is processed. Settlement itself waits for the
    /// marker because both invocation exit paths drain the drop-event queue — joining any
    /// [`DropEvent::AwaitDiscardMarker`] — before writing their final oplog state.
    pub(super) live_call_permit: Option<LiveCallPermit>,
    pub(super) cleanup_sink: Option<UnboundedSender<DropEvent>>,
    /// An owned oplog append (e.g. a durable `FinishSpan`) that must land *before* any marker
    /// append, preserving the recorded `End → FinishSpan → CompletionDiscarded` order replay
    /// consumes positionally. See [`CompletionDelivery::append_ordered`].
    pub(super) pending_append: Option<tokio::task::JoinHandle<Result<(), WorkerExecutorError>>>,
}

impl CompletionDelivery {
    pub(super) fn unarmed() -> Self {
        Self {
            state: CompletionDeliveryState::Unarmed,
        }
    }

    pub(super) fn replay_delivered() -> Self {
        Self {
            state: CompletionDeliveryState::ReplayDelivered,
        }
    }

    pub(super) fn replay_discarded() -> Self {
        Self {
            state: CompletionDeliveryState::ReplayDiscarded,
        }
    }

    /// Whether the recorded run discarded this completion: the caller must not deliver the
    /// response to the guest and instead parks at the delivery boundary after finishing its
    /// deterministic post-`End` continuation.
    pub fn is_replay_discarded(&self) -> bool {
        matches!(self.state, CompletionDeliveryState::ReplayDiscarded)
    }

    /// Whether the token is live and armed (a torn delivery would record a marker). Callers use
    /// this to route ordered post-`End` appends through [`Self::append_ordered`] instead of a
    /// direct oplog append that would race the torn-drop marker.
    pub fn is_live_armed(&self) -> bool {
        matches!(self.state, CompletionDeliveryState::Live(_))
    }

    /// Hands an oplog entry append (e.g. a durable `FinishSpan`) to an owned task ordered
    /// *before* any later marker append by this token. Must be called with no `await` between
    /// the token's creation (or previous [`Self::wait_appends`]) and this call when the entry is
    /// mandatory — a tear cannot happen between synchronous statements, so the obligation is
    /// transferred atomically. No-op unless live and armed.
    pub fn append_ordered(&mut self, entry: OplogEntry) {
        if let CompletionDeliveryState::Live(live) = &mut self.state {
            let oplog = live.marker.oplog.clone();
            let previous = live.pending_append.take();
            live.pending_append = Some(tokio::spawn(async move {
                if let Some(handle) = previous {
                    handle.await.map_err(|err| {
                        WorkerExecutorError::runtime(format!(
                            "ordered post-End append task failed: {err}"
                        ))
                    })??;
                }
                oplog.add(entry).await;
                Ok(())
            }));
        }
    }

    /// Joins the pending ordered append(s). Cancellation-safe: a tear mid-join leaves the join
    /// handle owned by the token, so the torn-drop marker append still chains after it.
    pub async fn wait_appends(&mut self) -> Result<(), WorkerExecutorError> {
        if let CompletionDeliveryState::Live(live) = &mut self.state
            && let Some(handle) = &mut live.pending_append
        {
            let result = handle.await.map_err(|err| {
                WorkerExecutorError::runtime(format!("ordered post-End append task failed: {err}"))
            });
            live.pending_append = None;
            result??;
        }
        Ok(())
    }

    /// The final guest-facing delivery succeeded: no marker. Consumes the token; any pending
    /// ordered append keeps running as an owned task and its join (plus the in-flight permit) is
    /// handed to the drain queue so settlement still waits for it.
    pub fn delivered(mut self) {
        self.settle();
    }

    /// A post-`End` error is returned to (observed by) the caller — the worker traps — so the
    /// completion was not *silently* discarded: no marker.
    pub fn suppress(mut self) {
        self.settle();
    }

    /// Consumes the token by arming Wasmtime's terminal-consumption observer for the host
    /// subtask `store` belongs to: the *actual* guest-delivery boundary of a direct accessor
    /// host call. The host method returning its result is not that boundary — Wasmtime still
    /// lowers the result and queues the subtask's `Returned` event afterwards, and the guest can
    /// consume that event via `subtask.cancel` (or abandon it through a post-`End` cancellation)
    /// without ever observing the response.
    ///
    /// The observer maps Wasmtime's verdict onto the token:
    /// - `Delivered` (the guest received the successful terminal) → [`Self::delivered`].
    /// - `Discarded` / `Cancelled` (the guest consumed or abandoned the completion without
    ///   observing the result after the `End` was persisted) → the armed token is dropped, which
    ///   spawns the owned cancellation-safe `CompletionDiscarded` marker append and hands its
    ///   join to the drain queue, so invocation settlement waits for it.
    /// - Dropped without being invoked (a trap, a lowering failure, or store teardown — all of
    ///   which abandon the whole execution rather than silently discarding this one completion;
    ///   replay re-executes the guest to the same point and redelivers) → [`Self::suppress`].
    ///
    /// Registering a newer observer for the same host subtask (a later durable call in the same
    /// host function) supersedes this one, suppressing its token: once a later durable event is
    /// recorded, replay re-executes the host code past this `End` deterministically and
    /// re-consumes the response internally, so no marker is needed.
    ///
    /// Non-live tokens (replay, unpersisted snapshotting calls) settle immediately; if the
    /// accessor has no guest-visible host subtask (e.g. a spawned background task), the token
    /// settles without a marker, matching the pre-observer behavior of consuming it at the host
    /// return.
    pub fn deliver_at_accessor_terminal<T, D>(self, store: &Accessor<T, D>)
    where
        T: 'static,
        D: HasData + ?Sized,
    {
        if !self.is_live_armed() {
            self.delivered();
            return;
        }
        let guard = AccessorDeliveryGuard {
            delivery: Some(self),
        };
        if let Err(error) = store
            .register_terminal_observer(Box::new(move |consumption| guard.consume(consumption)))
        {
            // No guest-visible host subtask to observe (the guard is dropped by the failed
            // registration, suppressing the token — no marker).
            tracing::debug!(
                "durable call completion has no guest-visible host subtask to observe: {error}"
            );
        }
    }

    fn settle(&mut self) {
        match std::mem::replace(&mut self.state, CompletionDeliveryState::Done) {
            CompletionDeliveryState::Live(live) => {
                if let Some(pending) = live.pending_append {
                    // The ordered append is still in flight: keep it settlement-accounted via
                    // the drain queue, without a marker.
                    Self::emit_await_event(
                        live.cleanup_sink,
                        pending,
                        live.trap_context,
                        live.live_call_permit,
                    );
                }
            }
            CompletionDeliveryState::ReplayDelivered
            | CompletionDeliveryState::ReplayDiscarded
            | CompletionDeliveryState::Unarmed
            | CompletionDeliveryState::Done => {}
        }
    }

    /// The caller detected a silent discard of the persisted completion (e.g. the guest dropped
    /// the receiving end of the delivery channel): appends exactly one `CompletionDiscarded`
    /// marker — ordered after any pending [`Self::append_ordered`] entry — and returns once it
    /// is durable. Cancellation-safe: marker persistence moves to an owned task *before* the
    /// first await, and a tear mid-wait hands the join plus the in-flight permit to the drain
    /// queue exactly like a torn armed drop, so the marker still lands and settlement still
    /// waits for it. No-op on replay.
    pub async fn discarded(mut self) -> Result<(), WorkerExecutorError> {
        match std::mem::replace(&mut self.state, CompletionDeliveryState::Done) {
            CompletionDeliveryState::Live(live) => {
                let LiveDelivery {
                    marker,
                    trap_context,
                    live_call_permit,
                    cleanup_sink,
                    pending_append,
                } = *live;
                let guard = MarkerAwaitGuard {
                    join: Some(marker.spawn_chained(pending_append)),
                    trap_context,
                    live_call_permit,
                    cleanup_sink,
                };
                guard.wait().await
            }
            CompletionDeliveryState::ReplayDelivered => {
                // Live delivered this completion (no marker on record), but replay could not: a
                // nondeterministic guest tore the receiving end at a different point. Nothing
                // durable to reconcile.
                tracing::warn!(
                    "replayed durable call completion could not be delivered although the recorded run delivered it"
                );
                Ok(())
            }
            CompletionDeliveryState::ReplayDiscarded
            | CompletionDeliveryState::Unarmed
            | CompletionDeliveryState::Done => Ok(()),
        }
    }

    fn emit_await_event(
        sink: Option<UnboundedSender<DropEvent>>,
        terminal: tokio::task::JoinHandle<Result<(), WorkerExecutorError>>,
        trap_context: DurableCallTrapContext,
        live_call_permit: Option<LiveCallPermit>,
    ) {
        if let Some(sink) = &sink {
            let _ = sink.send(DropEvent::AwaitDiscardMarker {
                terminal: Some(terminal),
                trap_context,
                live_call_permit,
            });
        }
    }
}

/// Test-only [`CompletionDelivery`] factories for delivery-boundary unit tests outside this
/// module (e.g. the consume-body chunk transfer helper). They build real tokens — the live one
/// appends a real `CompletionDiscarded` marker to the given oplog — without exposing the token
/// internals.
#[cfg(test)]
impl CompletionDelivery {
    /// A live-armed token over `oplog` whose torn/failed delivery appends a
    /// `CompletionDiscarded` marker for `start_idx`, exactly as
    /// [`CallHandle::complete_access_deferred`] arms one for a persisted live call whose `End`
    /// is already durable. `oplog` must already contain the call's `Start`/`End` entries (the
    /// token's replay state is built over its current contents).
    pub(crate) async fn test_live_armed(
        oplog: Arc<dyn Oplog>,
        start_idx: OplogIndex,
    ) -> Result<Self, WorkerExecutorError> {
        let replay_state = ReplayState::new(
            golem_common::model::OwnedAgentId {
                environment_id: golem_common::model::environment::EnvironmentId::new(),
                agent_id: golem_common::model::AgentId {
                    component_id: golem_common::model::component::ComponentId::new(),
                    agent_id: "completion-delivery-test".to_string(),
                },
            },
            oplog.clone(),
            golem_common::model::regions::DeletedRegions::default(),
        )
        .await?;
        Ok(Self {
            state: CompletionDeliveryState::Live(Box::new(LiveDelivery {
                marker: DiscardMarker {
                    start_idx,
                    oplog,
                    replay_state,
                },
                trap_context: DurableCallTrapContext {
                    retry_from: start_idx,
                    in_atomic_region: false,
                },
                live_call_permit: None,
                cleanup_sink: None,
                pending_append: None,
            })),
        })
    }

    /// A replay token for a recorded discarded completion, as
    /// [`CallHandle::replay_access_deferred`] returns when the recorded run persisted the `End`
    /// but never delivered it.
    pub(crate) fn test_replay_discarded() -> Self {
        Self::replay_discarded()
    }
}

impl Drop for CompletionDelivery {
    fn drop(&mut self) {
        match std::mem::replace(&mut self.state, CompletionDeliveryState::Done) {
            CompletionDeliveryState::Live(live) => {
                // The delivering future was torn while the token was still armed: the guest
                // silently discarded a persisted successful completion. Chain the owned marker
                // append after any pending ordered append (preserving the recorded
                // `End → FinishSpan → CompletionDiscarded` order) and hand the join plus the
                // in-flight permit to the drain queue so invocation settlement waits for it. The
                // task is spawned unconditionally — marker recording must not depend on the
                // event surviving the drain.
                let terminal = live.marker.spawn_chained(live.pending_append);
                Self::emit_await_event(
                    live.cleanup_sink,
                    terminal,
                    live.trap_context,
                    live.live_call_permit,
                );
            }
            CompletionDeliveryState::ReplayDelivered => {
                // Anomalous: replay delivered a completion the recorded run delivered too, but
                // the site dropped the token without consuming it (or the future was torn at a
                // point live was not). Nothing durable to reconcile.
                tracing::warn!("replay completion-delivery token dropped without being consumed");
            }
            CompletionDeliveryState::ReplayDiscarded
            | CompletionDeliveryState::Unarmed
            | CompletionDeliveryState::Done => {}
        }
    }
}

/// Adapts a live armed [`CompletionDelivery`] token to a Wasmtime terminal observer (see
/// [`CompletionDelivery::deliver_at_accessor_terminal`]). The observer runs inside Wasmtime's
/// event loop and must not access the store: every branch below only consumes the token, which
/// touches Golem-owned channels and owned tasks.
struct AccessorDeliveryGuard {
    delivery: Option<CompletionDelivery>,
}

impl AccessorDeliveryGuard {
    fn consume(mut self, consumption: TerminalConsumption) {
        let delivery = self
            .delivery
            .take()
            .expect("terminal observers are invoked at most once");
        match consumption {
            // The guest received the successful terminal: no marker.
            TerminalConsumption::Delivered => delivery.delivered(),
            // The guest consumed the pending terminal via `subtask.cancel` after the successful
            // lowering (`Discarded`), or cancelled the call after the `End` was persisted
            // (`Cancelled`): either way the guest never observes the persisted completion.
            // Dropping the armed token spawns the owned cancellation-safe marker append and
            // hands its join to the drain queue, so invocation settlement waits for it.
            TerminalConsumption::Discarded | TerminalConsumption::Cancelled => drop(delivery),
        }
    }
}

impl Drop for AccessorDeliveryGuard {
    fn drop(&mut self) {
        // Dropped without being invoked: the terminal was never consumed by the guest — a trap,
        // a lowering failure, or store teardown (or a later durable call in the same host
        // function superseding this observer). None of these silently discard the completion,
        // so no marker.
        if let Some(delivery) = self.delivery.take() {
            delivery.suppress();
        }
    }
}

/// RAII guard for awaiting an owned `CompletionDiscarded` marker task inline
/// ([`CompletionDelivery::discarded`]). Marker persistence already lives in the owned task; the
/// guard makes the *wait* cancellation-safe: a tear mid-wait hands the join plus the in-flight
/// [`LiveCallPermit`] to the drain queue via [`DropEvent::AwaitDiscardMarker`], so invocation
/// settlement still waits for the marker append.
struct MarkerAwaitGuard {
    /// `Some` until the join completes (successfully or not); a `Drop` with the join still
    /// pending emits the drain event.
    join: Option<tokio::task::JoinHandle<Result<(), WorkerExecutorError>>>,
    trap_context: DurableCallTrapContext,
    live_call_permit: Option<LiveCallPermit>,
    cleanup_sink: Option<UnboundedSender<DropEvent>>,
}

impl MarkerAwaitGuard {
    async fn wait(mut self) -> Result<(), WorkerExecutorError> {
        let joined = self
            .join
            .as_mut()
            .expect("MarkerAwaitGuard is always constructed with a join handle")
            .await;
        self.join = None;
        joined.map_err(|err| {
            WorkerExecutorError::runtime(format!("durable call discard-marker task failed: {err}"))
        })?
    }
}

impl Drop for MarkerAwaitGuard {
    fn drop(&mut self) {
        if let Some(join) = self.join.take() {
            CompletionDelivery::emit_await_event(
                self.cleanup_sink.take(),
                join,
                self.trap_context,
                self.live_call_permit.take(),
            );
        }
    }
}
