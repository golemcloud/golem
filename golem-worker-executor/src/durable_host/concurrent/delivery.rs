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
            if let Some(append) = pending_append
                && let Err(error) = append.wait().await
            {
                let _ = done.send(Err(error));
                return;
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

/// A deferred guest-delivery token returned by [`DurableCallSession::complete_access_deferred`] /
/// [`DurableCallSession::replay_access_deferred`] for call sites whose result crosses one more fallible
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
    /// Live, but the call was not persisted: nothing to reconcile.
    Unarmed,
    /// Replay of a recorded terminal the guest observed (or must observe): see [`ReplayDelivery`]
    /// for the per-disposition gating.
    ReplayDelivered(ReplayDelivery),
    /// Replay of a recorded discarded completion: the caller must not deliver and parks at the
    /// delivery boundary.
    ReplayDiscarded,
    /// Consumed (`delivered`/`suppress`/`discarded`).
    Done,
}

/// How a replayed guest-observed terminal is gated before it may cross to the guest. Produced
/// from a [`ReplayDeliveryDisposition`] by [`CompletionDelivery::replay_delivered`].
pub(super) enum ReplayDelivery {
    /// A guest-observed terminal that never carries a marker: a cancelled call's recorded partial
    /// result, delivered at the guest-initiated (and therefore deterministic) cancellation point.
    /// Nothing to gate or reconcile.
    Immediate,
    /// A completed `End` with a recorded `CompletionDelivered` marker: delivery is gated on
    /// consuming that exact marker ([`CompletionDelivery::prepare_delivery`]).
    AtMarker {
        replay_state: ReplayState,
        start_index: OplogIndex,
        marker_index: OplogIndex,
    },
    /// The marker was consumed: the token owns the global cursor gate until the actual guest
    /// boundary acknowledges it.
    Armed(crate::durable_host::replay_state::ReplayDeliveryBarrier),
    /// A completed `End` with no delivery marker: the recorded run crashed after the `End` became
    /// durable but before the completion crossed to the guest. Delivery is withheld until the
    /// replay tail naturally exhausts ([`ReplayState::await_natural_tail_end`]) — no tail entry
    /// can depend on the never-happened delivery (a marker would precede any dependent entry) —
    /// and the token then converts to live-armed so the eventual real delivery or discard records
    /// its marker durably.
    AtReplayTail(Box<LiveDelivery>),
}

impl ReplayDelivery {
    fn fail(self, reason: impl Into<String>) -> WorkerExecutorError {
        let reason = reason.into();
        match self {
            Self::Immediate => {}
            Self::AtMarker {
                replay_state,
                start_index,
                marker_index,
            } => replay_state.fail_completion_delivery(start_index, marker_index, reason.clone()),
            Self::Armed(barrier) => barrier.fail(reason.clone()),
            // A still-tail-gated token settles silently: no marker exists and none may be
            // appended during replay, so the `End` simply stays markerless and the next recovery
            // tail-gates it again — exactly the recorded state.
            Self::AtReplayTail(_) => {}
        }
        WorkerExecutorError::runtime(format!(
            "replay could not reproduce a recorded successful completion delivery: {reason}"
        ))
    }
}

/// Pure classification of how a replayed guest-observed terminal must be gated, produced by
/// [`super::call::classify_replay_resolution`] and consumed by
/// [`CompletionDelivery::replay_delivered`]. The direct (non-accessor) replay path rejects
/// [`Self::AtMarker`] (markers are recorded only on accessor paths) and ignores the gating
/// otherwise: a direct call's guest task is synchronously blocked inside the host call, so its
/// delivery coincides with the host return and has no pre-delivery divergence window.
#[derive(Debug, Clone, Copy)]
pub(super) enum ReplayDeliveryDisposition {
    /// A cancelled call's recorded partial result: guest-initiated, never marker-bearing.
    Immediate,
    /// A completed `End` with its recorded `CompletionDelivered` marker.
    AtMarker(OplogIndex),
    /// A completed `End` without a delivery marker (the recorded run crashed after the `End`
    /// became durable but before the completion crossed to the guest).
    AtReplayTail,
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

    /// Builds the replay token for a guest-observed terminal according to its recorded
    /// [`ReplayDeliveryDisposition`]. `recorder`, `trap_context` and `cleanup_sink` are needed
    /// only for [`ReplayDeliveryDisposition::AtReplayTail`], whose token converts to live-armed
    /// once the replay tail exhausts (see [`Self::prepare_delivery`]); `live_call_permit` stays
    /// `None` — a marker lost after a suspension just tail-gates the `End` again.
    pub(super) fn replay_delivered(
        disposition: ReplayDeliveryDisposition,
        start_index: OplogIndex,
        recorder: CompletionMarkerRecorder,
        trap_context: DurableCallTrapContext,
        cleanup_sink: Option<UnboundedSender<DropEvent>>,
    ) -> Self {
        Self {
            state: CompletionDeliveryState::ReplayDelivered(match disposition {
                ReplayDeliveryDisposition::Immediate => ReplayDelivery::Immediate,
                ReplayDeliveryDisposition::AtMarker(marker_index) => ReplayDelivery::AtMarker {
                    replay_state: recorder.replay_state.clone(),
                    start_index,
                    marker_index,
                },
                ReplayDeliveryDisposition::AtReplayTail => {
                    ReplayDelivery::AtReplayTail(Box::new(LiveDelivery {
                        marker: CompletionMarkerRecord {
                            start_idx: start_index,
                            recorder,
                        },
                        trap_context,
                        live_call_permit: None,
                        cleanup_sink,
                        pending_append: None,
                    }))
                }
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

    /// Whether this replayed completion has a recorded delivery marker that must be reached
    /// before cancellation can settle the guest-facing transfer. Callers inspect this before
    /// [`Self::prepare_delivery`] replaces the marker position with an armed barrier.
    pub fn is_replay_at_marker(&self) -> bool {
        matches!(
            self.state,
            CompletionDeliveryState::ReplayDelivered(ReplayDelivery::AtMarker { .. })
        )
    }

    /// Whether the token is live and armed (a torn delivery would record a marker). Callers use
    /// this to route ordered post-`End` appends through [`Self::append_ordered`] instead of a
    /// direct oplog append that would race the torn-drop marker.
    pub fn is_live_armed(&self) -> bool {
        matches!(self.state, CompletionDeliveryState::Live(_))
    }

    /// Positions replay at this completion's recorded guest-delivery boundary. All deterministic
    /// host-side continuation after `End` must run before this call.
    ///
    /// For marker-bearing replay ([`ReplayDelivery::AtMarker`]) the token consumes its exact
    /// `CompletionDelivered` marker and owns the global cursor gate until [`Self::delivered`]
    /// acknowledges the actual callback/channel handoff.
    ///
    /// For markerless completed replay ([`ReplayDelivery::AtReplayTail`]) the delivery is
    /// withheld until the recorded tail naturally exhausts — nothing in the tail can depend on
    /// the never-happened delivery, while delivering earlier could make the replayed guest skip
    /// recorded entries — and the token then converts to live-armed, so the eventual real
    /// delivery or discard records its marker. Cancellation-safe: a tear mid-wait leaves the
    /// token tail-gated and settles it silently, keeping the `End` markerless for the next
    /// recovery.
    ///
    /// Live and immediate replay are no-ops.
    pub async fn prepare_delivery(&mut self) -> Result<(), WorkerExecutorError> {
        match &self.state {
            CompletionDeliveryState::ReplayDelivered(ReplayDelivery::AtMarker {
                replay_state,
                start_index,
                marker_index,
            }) => {
                let (replay_state, start_index, marker_index) =
                    (replay_state.clone(), *start_index, *marker_index);
                let barrier = replay_state
                    .await_completion_delivery(start_index, marker_index)
                    .await?;
                self.state =
                    CompletionDeliveryState::ReplayDelivered(ReplayDelivery::Armed(barrier));
            }
            CompletionDeliveryState::ReplayDelivered(ReplayDelivery::AtReplayTail(live)) => {
                let replay_state = live.marker.recorder.replay_state.clone();
                replay_state.await_natural_tail_end().await?;
                if let CompletionDeliveryState::ReplayDelivered(ReplayDelivery::AtReplayTail(
                    live,
                )) = std::mem::replace(&mut self.state, CompletionDeliveryState::Done)
                {
                    self.state = CompletionDeliveryState::Live(live);
                }
            }
            _ => {}
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
            CompletionDeliveryState::ReplayDelivered(ReplayDelivery::Immediate) => {}
            CompletionDeliveryState::ReplayDelivered(pending @ ReplayDelivery::AtMarker { .. }) => {
                let _ = pending.fail("delivery occurred before its recorded marker was consumed");
            }
            CompletionDeliveryState::ReplayDelivered(ReplayDelivery::AtReplayTail(live)) => {
                // A delivery boundary fired while the markerless completion was still
                // tail-gated: the call site never awaited `prepare_delivery`. Poison replay
                // loudly instead of silently re-opening the crash window this gate closes.
                live.marker.recorder.replay_state.fail_tail_delivery(
                    live.marker.start_idx,
                    "delivery occurred while the markerless completion was still tail-gated \
                     (prepare_delivery was never awaited)",
                );
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
    /// Starting a later durable call on the same host subtask while this observer is still armed
    /// is a forbidden host-function pattern (see
    /// [`DurableCallSession::supersede_prior_completion_delivery`], which hard-errors): host code must
    /// arm the observer only as the tail operation of its host function. That invariant is what
    /// lets a markerless `End` be tail-gated on replay — a completion consumed host-internally
    /// would legitimize durable tail entries that depend on an unmarked delivery.
    ///
    /// Non-live tokens (replay and unpersisted calls) settle immediately; if the
    /// accessor has no guest-visible host subtask (e.g. a spawned background task), the token
    /// settles without a marker, matching the pre-observer behavior of consuming it at the host
    /// return. A tail-gated markerless replay token checks for that subtask *before* gating:
    /// live never recorded a marker in such a context, and the same background task may have
    /// recorded later durable calls in the tail, so gating would stall their claims — it
    /// delivers immediately instead, mirroring live.
    pub async fn deliver_at_accessor_terminal<T, D>(
        mut self,
        store: &Accessor<T, D>,
    ) -> Result<(), WorkerExecutorError>
    where
        T: 'static,
        D: HasData + ?Sized,
    {
        if matches!(
            &self.state,
            CompletionDeliveryState::ReplayDelivered(ReplayDelivery::AtReplayTail(_))
        ) && !store.has_guest_visible_subtask()
        {
            self.state = CompletionDeliveryState::Done;
            return Ok(());
        }
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
        if let Err(error) =
            store.register_terminal_observer(move |consumption| guard.consume(consumption))
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
    /// [`DurableCallSession::complete_access_deferred`] arms one for a persisted live call whose `End`
    /// is already durable. `oplog` must already contain the call's `Start`/`End` entries (the
    /// token's replay state is built over its current contents).
    pub(crate) async fn test_live_armed(
        oplog: Arc<dyn Oplog>,
        start_idx: OplogIndex,
    ) -> Result<Self, WorkerExecutorError> {
        let replay_state = ReplayState::new_for_owner(
            golem_common::model::OwnedAgentId {
                environment_id: golem_common::model::environment::EnvironmentId::new(),
                agent_id: golem_common::model::AgentId {
                    component_id: golem_common::model::component::ComponentId::new(),
                    agent_id: "completion-delivery-test".to_string(),
                },
            },
            oplog.clone(),
            golem_common::model::regions::DeletedRegions::default(),
            None,
            crate::durable_host::tool::operation::OwnerToolOperations::new(),
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
    /// [`DurableCallSession::replay_access_deferred`] returns when the recorded run persisted the `End`
    /// but never delivered it.
    pub(crate) fn test_replay_discarded() -> Self {
        Self::replay_discarded()
    }

    /// A replay token for a guest-observed terminal that never carries a marker (a cancelled
    /// call's recorded partial result): delivered immediately, nothing to reconcile.
    pub(crate) fn test_replay_delivered_immediate() -> Self {
        Self {
            state: CompletionDeliveryState::ReplayDelivered(ReplayDelivery::Immediate),
        }
    }

    /// A tail-gated replay token for a markerless completed `End` (the recorded run crashed
    /// after the `End` became durable but before the completion crossed to the guest), exactly
    /// as [`DurableCallSession::replay_access_deferred`] builds one. `replay_state` must be the state
    /// replaying `oplog`, with the call's `Start` already claimed and resolved — as the real
    /// replay path guarantees before it constructs the token.
    pub(crate) fn test_replay_at_tail(
        oplog: Arc<dyn Oplog>,
        replay_state: ReplayState,
        start_idx: OplogIndex,
        cleanup_sink: Option<UnboundedSender<DropEvent>>,
    ) -> Self {
        let recorder = CompletionMarkerRecorder::new(oplog, replay_state);
        Self::replay_delivered(
            ReplayDeliveryDisposition::AtReplayTail,
            start_idx,
            recorder,
            DurableCallTrapContext {
                retry_from: start_idx,
                in_atomic_region: false,
            },
            cleanup_sink,
        )
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
            // The guest consumed the pending terminal without observing the persisted
            // completion (`subtask.cancel` on a returned terminal, or a cancellation).
            // Dropping the armed token spawns the owned cancellation-safe marker append and
            // hands its join to the drain queue, so invocation settlement waits for it.
            TerminalConsumption::NotDelivered => {
                if matches!(&delivery.state, CompletionDeliveryState::ReplayDelivered(_)) {
                    delivery.suppress();
                } else {
                    drop(delivery);
                }
            }
            // Observer replacement on an armed subtask is a forbidden pattern
            // (`supersede_prior_completion_delivery` hard-errors before a newer observer can be
            // registered), so this firing at all is an invariant breach. Log loudly and settle
            // without a marker: a marker-bearing replay token poisons replay via `suppress`,
            // while a live token stays markerless so recovery tail-gates the `End`.
            TerminalConsumption::Superseded => {
                tracing::error!(
                    "completion-delivery observer was superseded by a newer observer on the same \
                     host subtask; deliver_at_accessor_terminal must be the tail operation of its \
                     host function"
                );
                delivery.suppress();
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
