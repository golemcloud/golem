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

pub(super) type MarkerReceipt = tokio::sync::oneshot::Receiver<Result<(), WorkerExecutorError>>;

#[derive(Debug, Clone, Copy)]
pub(super) enum CompletionMarkerKind {
    Delivered,
    Discarded,
}

pub(super) enum OrderedAppend {
    Receipt(crate::services::oplog::OplogAddReceipt),
    Task(tokio::task::JoinHandle<Result<(), WorkerExecutorError>>),
}

impl OrderedAppend {
    async fn wait(self) -> Result<(), WorkerExecutorError> {
        match self {
            Self::Receipt(receipt) => {
                receipt.await;
                Ok(())
            }
            Self::Task(task) => task.await.map_err(|err| {
                WorkerExecutorError::runtime(format!("ordered oplog append task failed: {err}"))
            })?,
        }
    }
}

/// Records successful-completion delivery markers in the exact order their guest boundaries occur.
/// Recording is initiated synchronously from Wasmtime's terminal observer and reserves the marker's
/// oplog position before returning; only waiting for the append is asynchronous.
#[derive(Clone, Debug)]
pub(in crate::durable_host) struct CompletionMarkerRecorder {
    oplog: Arc<dyn Oplog>,
    replay_state: ReplayState,
}

impl CompletionMarkerRecorder {
    pub(in crate::durable_host) fn new(oplog: Arc<dyn Oplog>, replay_state: ReplayState) -> Self {
        Self {
            oplog,
            replay_state,
        }
    }

    pub(super) fn record(
        &self,
        start_idx: OplogIndex,
        kind: CompletionMarkerKind,
        pending_append: Option<OrderedAppend>,
    ) -> MarkerReceipt {
        let entry = match kind {
            CompletionMarkerKind::Delivered => OplogEntry::CompletionDelivered {
                timestamp: Timestamp::now_utc(),
                start_index: start_idx,
            },
            CompletionMarkerKind::Discarded => OplogEntry::CompletionDiscarded {
                timestamp: Timestamp::now_utc(),
                start_index: start_idx,
            },
        };

        // This call reserves the marker in the same oplog actor queue as every subsequent durable
        // operation before Wasmtime can enter the guest callback.
        let marker_append = self.oplog.enqueue_add(entry);
        let replay_state = self.replay_state.clone();
        let (done, receipt) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            if let Some(append) = pending_append {
                if let Err(error) = append.wait().await {
                    let _ = done.send(Err(error));
                    return;
                }
            }
            let marker_idx = marker_append.await;
            match kind {
                CompletionMarkerKind::Delivered => {
                    replay_state.record_delivered_completion(start_idx, marker_idx)
                }
                CompletionMarkerKind::Discarded => {
                    replay_state.record_discarded_completion(start_idx, marker_idx)
                }
            }
            let _ = done.send(Ok(()));
        });
        receipt
    }
}

pub(super) struct CompletionMarkerRecord {
    pub(super) start_idx: OplogIndex,
    pub(super) recorder: CompletionMarkerRecorder,
}

impl CompletionMarkerRecord {
    pub(super) fn record(
        self,
        kind: CompletionMarkerKind,
        pending_append: Option<OrderedAppend>,
    ) -> MarkerReceipt {
        self.recorder.record(self.start_idx, kind, pending_append)
    }
}

pub(super) async fn await_marker_receipt(
    receipt: &mut MarkerReceipt,
) -> Result<(), WorkerExecutorError> {
    receipt.await.map_err(|_| {
        WorkerExecutorError::runtime(
            "completion-marker recorder dropped a command without replying",
        )
    })?
}

fn receipt_for_pending_append(append: OrderedAppend) -> MarkerReceipt {
    let (done, receipt) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let _ = done.send(append.wait().await);
    });
    receipt
}

pub(super) fn task_for_marker_receipt(
    mut receipt: MarkerReceipt,
) -> tokio::task::JoinHandle<Result<(), WorkerExecutorError>> {
    tokio::spawn(async move { await_marker_receipt(&mut receipt).await })
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
/// - [`Self::delivered`] — the final guest-facing transfer succeeded; records a
///   `CompletionDelivered` marker at that boundary.
/// - [`Self::suppress`] — a post-`End` error is observed by the caller (the worker traps); no
///   marker.
/// - [`Self::discarded`] — the caller detected a silent discard (e.g. the guest dropped the
///   receiving end of the delivery channel); appends exactly one `CompletionDiscarded` marker
///   inline and returns once it is durable.
/// - `Drop` while armed — the delivering future itself was torn; spawns exactly one owned marker
///   append (ordered after any pending [`Self::append_ordered`] entry) and hands its join plus
///   the in-flight [`LiveCallPermit`] to the drain queue via [`DropEvent::AwaitCompletionMarker`],
///   so invocation settlement cannot overtake the append.
///
/// On replay the token mirrors the recorded delivery status. A discarded completion parks at its
/// delivery boundary. A delivered completion first lets the host-side post-`End` continuation run,
/// then [`Self::prepare_delivery`] consumes its exact `CompletionDelivered` marker and holds the
/// global replay cursor until [`Self::delivered`] observes the actual guest-facing boundary.
pub struct CompletionDelivery {
    pub(super) state: CompletionDeliveryState,
}

pub(super) enum CompletionDeliveryState {
    /// Live, armed: the `End` is persisted and a torn/failed delivery must record a marker.
    Live(Box<LiveDelivery>),
    /// Live, but the call was not persisted (snapshotting): nothing to reconcile.
    Unarmed,
    /// Replay of a normally delivered completion, optionally gated by a marker (old oplogs have
    /// no marker and retain the legacy immediate behavior).
    ReplayDelivered(ReplayDelivery),
    /// Replay of a recorded discarded completion: the caller must not deliver and parks at the
    /// delivery boundary.
    ReplayDiscarded,
    /// Consumed (`delivered`/`suppress`/`discarded`).
    Done,
}

pub(super) enum ReplayDelivery {
    Legacy,
    Pending {
        replay_state: ReplayState,
        start_index: OplogIndex,
        marker_index: OplogIndex,
    },
    Armed(crate::durable_host::replay_state::ReplayDeliveryBarrier),
}

impl ReplayDelivery {
    fn fail(self, reason: impl Into<String>) -> WorkerExecutorError {
        let reason = reason.into();
        match self {
            Self::Legacy => {}
            Self::Pending {
                replay_state,
                start_index,
                marker_index,
            } => replay_state.fail_completion_delivery(start_index, marker_index, reason.clone()),
            Self::Armed(barrier) => barrier.fail(reason.clone()),
        }
        WorkerExecutorError::runtime(format!(
            "replay could not reproduce a recorded successful completion delivery: {reason}"
        ))
    }
}

pub(super) struct LiveDelivery {
    pub(super) marker: CompletionMarkerRecord,
    pub(super) trap_context: DurableCallTrapContext,
    /// Keeps the call counted as in flight (for positional-boundary and snapshot checks) until
    /// the token is consumed or its drain event is processed. Settlement itself waits for the
    /// marker because both invocation exit paths drain the drop-event queue — joining any
    /// [`DropEvent::AwaitCompletionMarker`] — before writing their final oplog state.
    pub(super) live_call_permit: Option<LiveCallPermit>,
    pub(super) cleanup_sink: Option<UnboundedSender<DropEvent>>,
    /// An oplog append (e.g. a durable `FinishSpan`) synchronously reserved before any marker,
    /// preserving the recorded `End → FinishSpan → CompletionDiscarded` order replay consumes
    /// positionally. See [`CompletionDelivery::append_ordered`].
    pub(super) pending_append: Option<OrderedAppend>,
}

impl CompletionDelivery {
    pub(super) fn unarmed() -> Self {
        Self {
            state: CompletionDeliveryState::Unarmed,
        }
    }

    pub(super) fn replay_delivered(
        replay_state: ReplayState,
        start_index: OplogIndex,
        delivery_marker: Option<OplogIndex>,
    ) -> Self {
        Self {
            state: CompletionDeliveryState::ReplayDelivered(match delivery_marker {
                Some(marker_index) => ReplayDelivery::Pending {
                    replay_state,
                    start_index,
                    marker_index,
                },
                None => ReplayDelivery::Legacy,
            }),
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

    /// Positions replay at this completion's recorded guest-delivery boundary. All deterministic
    /// host-side continuation after `End` must run before this call. For marker-bearing replay the
    /// returned token owns the global cursor gate until [`Self::delivered`] acknowledges the actual
    /// callback/channel handoff; live and legacy replay are no-ops.
    pub async fn prepare_delivery(&mut self) -> Result<(), WorkerExecutorError> {
        let pending = match &self.state {
            CompletionDeliveryState::ReplayDelivered(ReplayDelivery::Pending {
                replay_state,
                start_index,
                marker_index,
            }) => Some((replay_state.clone(), *start_index, *marker_index)),
            _ => None,
        };
        if let Some((replay_state, start_index, marker_index)) = pending {
            let barrier = replay_state
                .await_completion_delivery(start_index, marker_index)
                .await?;
            self.state = CompletionDeliveryState::ReplayDelivered(ReplayDelivery::Armed(barrier));
        }
        Ok(())
    }

    /// Hands an oplog entry append (e.g. a durable `FinishSpan`) to an owned task ordered
    /// *before* any later marker append by this token. Must be called with no `await` between
    /// the token's creation (or previous [`Self::wait_appends`]) and this call when the entry is
    /// mandatory — a tear cannot happen between synchronous statements, so the obligation is
    /// transferred atomically. No-op unless live and armed.
    pub fn append_ordered(&mut self, entry: OplogEntry) {
        if let CompletionDeliveryState::Live(live) = &mut self.state {
            live.pending_append = Some(OrderedAppend::Receipt(
                live.marker.recorder.oplog.enqueue_add(entry),
            ));
        }
    }

    /// Joins the pending ordered append(s). Cancellation-safe: a tear mid-join leaves the join
    /// handle owned by the token, so the torn-drop marker append still chains after it.
    pub async fn wait_appends(&mut self) -> Result<(), WorkerExecutorError> {
        if let CompletionDeliveryState::Live(live) = &mut self.state
            && let Some(append) = live.pending_append.take()
        {
            append.wait().await?;
        }
        Ok(())
    }

    /// The final guest-facing delivery succeeded. Synchronously queues a `CompletionDelivered`
    /// command so callback handoffs are serialized in their observed order, then hands its receipt
    /// and the in-flight permit to the drain queue. The next durable call and invocation settlement
    /// both wait for the marker to become durable.
    pub fn delivered(mut self) {
        match std::mem::replace(&mut self.state, CompletionDeliveryState::Done) {
            CompletionDeliveryState::Live(live) => {
                let receipt = live
                    .marker
                    .record(CompletionMarkerKind::Delivered, live.pending_append);
                Self::emit_await_event(
                    live.cleanup_sink,
                    receipt,
                    live.trap_context,
                    live.live_call_permit,
                );
            }
            CompletionDeliveryState::ReplayDelivered(ReplayDelivery::Armed(barrier)) => {
                barrier.acknowledge();
            }
            CompletionDeliveryState::ReplayDelivered(ReplayDelivery::Legacy) => {}
            CompletionDeliveryState::ReplayDelivered(pending @ ReplayDelivery::Pending { .. }) => {
                let _ = pending.fail("delivery occurred before its recorded marker was consumed");
            }
            CompletionDeliveryState::ReplayDiscarded
            | CompletionDeliveryState::Unarmed
            | CompletionDeliveryState::Done => {}
        }
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
    pub async fn deliver_at_accessor_terminal<T, D>(
        mut self,
        store: &Accessor<T, D>,
    ) -> Result<(), WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
    {
        self.prepare_delivery().await?;
        let replay_barrier = matches!(
            self.state,
            CompletionDeliveryState::ReplayDelivered(ReplayDelivery::Armed(_))
        );
        if !self.is_live_armed() && !replay_barrier {
            self.delivered();
            return Ok(());
        }
        let guard = AccessorDeliveryGuard {
            delivery: Some(self),
        };
        if let Err(error) = store
            .register_terminal_observer(Box::new(move |consumption| guard.consume(consumption)))
        {
            // No guest-visible host subtask to observe (the guard is dropped by the failed
            // registration, suppressing the token — no marker).
            if replay_barrier {
                return Err(WorkerExecutorError::runtime(format!(
                    "recorded completion delivery has no guest-visible host subtask during replay: {error}"
                )));
            } else {
                tracing::debug!(
                    "durable call completion has no guest-visible host subtask to observe: {error}"
                );
            }
        }
        Ok(())
    }

    fn settle(&mut self) {
        match std::mem::replace(&mut self.state, CompletionDeliveryState::Done) {
            CompletionDeliveryState::Live(live) => {
                if let Some(pending) = live.pending_append {
                    // The ordered append is still in flight: keep it settlement-accounted via
                    // the drain queue, without a marker.
                    Self::emit_await_event(
                        live.cleanup_sink,
                        receipt_for_pending_append(pending),
                        live.trap_context,
                        live.live_call_permit,
                    );
                }
            }
            CompletionDeliveryState::ReplayDelivered(replay) => {
                let _ = replay.fail("delivery was suppressed before reaching the guest");
            }
            CompletionDeliveryState::ReplayDiscarded
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
                    receipt: Some(marker.record(CompletionMarkerKind::Discarded, pending_append)),
                    trap_context,
                    live_call_permit,
                    cleanup_sink,
                };
                guard.wait().await
            }
            CompletionDeliveryState::ReplayDelivered(replay) => Err(replay.fail(
                "the replayed guest discarded a completion that was delivered when recorded",
            )),
            CompletionDeliveryState::ReplayDiscarded
            | CompletionDeliveryState::Unarmed
            | CompletionDeliveryState::Done => Ok(()),
        }
    }

    fn emit_await_event(
        sink: Option<UnboundedSender<DropEvent>>,
        receipt: MarkerReceipt,
        trap_context: DurableCallTrapContext,
        live_call_permit: Option<LiveCallPermit>,
    ) {
        if let Some(sink) = &sink {
            let _ = sink.send(DropEvent::AwaitCompletionMarker {
                receipt: Some(receipt),
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
        let recorder = CompletionMarkerRecorder::new(oplog, replay_state);
        Ok(Self {
            state: CompletionDeliveryState::Live(Box::new(LiveDelivery {
                marker: CompletionMarkerRecord {
                    start_idx,
                    recorder,
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

    pub(crate) fn test_replay_delivered_legacy() -> Self {
        Self {
            state: CompletionDeliveryState::ReplayDelivered(ReplayDelivery::Legacy),
        }
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
                // marker command is queued synchronously — marker recording must not depend on
                // the event surviving the drain.
                let receipt = live
                    .marker
                    .record(CompletionMarkerKind::Discarded, live.pending_append);
                Self::emit_await_event(
                    live.cleanup_sink,
                    receipt,
                    live.trap_context,
                    live.live_call_permit,
                );
            }
            CompletionDeliveryState::ReplayDelivered(replay) => {
                let _ = replay.fail(
                    "completion-delivery token was dropped before the recorded guest boundary",
                );
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
            // Queue the successful delivery marker immediately before Wasmtime enters the guest
            // callback (or after wait/poll/sync lowering handed the result to the guest).
            TerminalConsumption::Delivered => delivery.delivered(),
            // The guest consumed the pending terminal via `subtask.cancel` after the successful
            // lowering (`Discarded`), or cancelled the call after the `End` was persisted
            // (`Cancelled`): either way the guest never observes the persisted completion.
            // Dropping the armed token spawns the owned cancellation-safe marker append and
            // hands its join to the drain queue, so invocation settlement waits for it.
            TerminalConsumption::Discarded | TerminalConsumption::Cancelled => {
                if matches!(&delivery.state, CompletionDeliveryState::ReplayDelivered(_)) {
                    delivery.suppress();
                } else {
                    drop(delivery);
                }
            }
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

/// RAII guard for awaiting an owned `CompletionDiscarded` marker command inline
/// ([`CompletionDelivery::discarded`]). The command already lives in the recorder actor; the
/// guard makes the *wait* cancellation-safe: a tear mid-wait hands the receipt plus the in-flight
/// [`LiveCallPermit`] to the drain queue via [`DropEvent::AwaitCompletionMarker`], so invocation
/// settlement still waits for the marker append.
struct MarkerAwaitGuard {
    /// `Some` until the receipt completes (successfully or not); a `Drop` with the receipt still
    /// pending emits the drain event.
    receipt: Option<MarkerReceipt>,
    trap_context: DurableCallTrapContext,
    live_call_permit: Option<LiveCallPermit>,
    cleanup_sink: Option<UnboundedSender<DropEvent>>,
}

impl MarkerAwaitGuard {
    async fn wait(mut self) -> Result<(), WorkerExecutorError> {
        let result = await_marker_receipt(
            self.receipt
                .as_mut()
                .expect("MarkerAwaitGuard is always constructed with a marker receipt"),
        )
        .await;
        self.receipt = None;
        result
    }
}

impl Drop for MarkerAwaitGuard {
    fn drop(&mut self) {
        if let Some(receipt) = self.receipt.take() {
            CompletionDelivery::emit_await_event(
                self.cleanup_sink.take(),
                receipt,
                self.trap_context,
                self.live_call_permit.take(),
            );
        }
    }
}
