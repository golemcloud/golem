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

impl P3HttpSendSpan {
    pub(crate) fn claim_closure(&self) -> bool {
        !self.closure_claimed.swap(true, Ordering::AcqRel)
    }
}

/// Deterministically derives the span id of a p3 send's
/// `outgoing-http-request` span from the send's own host-call `Start` index,
/// which is stable across live execution and replay. Uses UUIDv5 (like
/// `IdempotencyKey::derived`) folded to 64 bits, so re-derived trace-context
/// headers are stable across executor versions too.
pub(in crate::durable_host::p3::http) fn derive_p3_send_span_id(
    owned_agent_id: &OwnedAgentId,
    start_index: OplogIndex,
) -> SpanId {
    let name = format!("{owned_agent_id}/outgoing-http-request/{start_index}");
    let (hi, lo) = Uuid::new_v5(&Uuid::NAMESPACE_OID, name.as_bytes()).as_u64_pair();
    let n = NonZeroU64::new(hi ^ lo).unwrap_or(NonZeroU64::new(u64::MAX).unwrap());
    SpanId(n)
}

pub(super) fn install_p3_send_span<Ctx: WorkerCtx>(
    ctx: &mut crate::durable_host::DurableWorkerCtx<Ctx>,
    started: &SpanStarted,
) -> Result<Arc<InvocationContextSpan>, WorkerExecutorError> {
    let parent = started
        .parent_span_id
        .as_ref()
        .map(|id| ctx.state.invocation_context.get(id))
        .transpose()
        .map_err(WorkerExecutorError::runtime)?;
    let mut builder = InvocationContextSpan::local()
        .with_span_id(started.span_id.clone())
        .with_start(started.started_at)
        .with_attributes(started.attributes.0.clone());
    if let Some(parent) = parent {
        builder = builder.with_parent(parent);
    }
    let span = builder.build();
    ctx.state.invocation_context.add_span_with_origin(
        span.clone(),
        started.trace_id.clone(),
        started.trace_states.clone(),
    );
    Ok(span)
}

pub(in crate::durable_host::p3::http) fn p3_send_span_finished(
    span: &P3HttpSendSpan,
    outcome: golem_common::model::oplog::SpanOutcome,
) -> golem_common::model::oplog::SpanFinished {
    golem_common::model::oplog::SpanFinished {
        span_id: span.span_id.clone(),
        finished_at: golem_common::model::Timestamp::now_utc(),
        outcome,
    }
}

impl std::fmt::Debug for P3SendSpanLifecycle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("P3SendSpanLifecycle")
            .field("closure_claimed", &self.closure_claimed)
            .finish_non_exhaustive()
    }
}

impl P3SendSpanLifecycle {
    pub(super) fn new(recorder: crate::durable_host::concurrent::HttpSpanCleanupRecorder) -> Self {
        Self {
            state: Mutex::new(P3SendSpanLifecycleState {
                recorder,
                started: None,
                abandonment: None,
                cleanup_submitted: false,
                replay_tx: None,
            }),
            closure_claimed: Arc::new(AtomicBool::new(false)),
        }
    }

    fn set_replay_sender(
        &self,
        replay_tx: oneshot::Sender<Option<crate::durable_host::concurrent::HttpSpanCleanupReplay>>,
    ) {
        self.state.lock().unwrap().replay_tx = Some(replay_tx);
    }

    pub(super) fn started_span(&self) -> Option<SpanStarted> {
        self.state.lock().unwrap().started.clone()
    }

    pub(super) fn snapshot_started(
        &self,
        index: OplogIndex,
        owner: Option<OplogIndex>,
        started: SpanStarted,
    ) {
        let mut state = self.state.lock().unwrap();
        state.recorder = state.recorder.clone().for_start(index, owner, true);
        state.started = Some(started);
    }

    pub(super) fn abandon(&self) {
        let mut state = self.state.lock().expect("p3 send span lifecycle poisoned");
        state
            .abandonment
            .get_or_insert_with(golem_common::model::Timestamp::now_utc);
        self.submit_cleanup(&mut state);
    }

    fn submit_cleanup(&self, state: &mut P3SendSpanLifecycleState) {
        if state.cleanup_submitted {
            return;
        }
        let (Some(started), Some(finished_at)) = (&state.started, state.abandonment) else {
            return;
        };
        if self.closure_claimed.swap(true, Ordering::AcqRel) {
            return;
        }
        state.cleanup_submitted = true;
        let replay = state.recorder.clone().record(SpanFinished {
            span_id: started.span_id.clone(),
            finished_at,
            outcome: golem_common::model::oplog::SpanOutcome::Abandoned,
        });
        if let Some(tx) = state.replay_tx.take() {
            let _ = tx.send(replay);
        }
    }
}

impl CallSpanObserver for P3SendSpanLifecycle {
    fn started(self: Arc<Self>, index: OplogIndex, entry: &OplogEntry, replay: bool) {
        let OplogEntry::Start {
            observational_owner,
            span_started: Some(started),
            ..
        } = entry
        else {
            return;
        };
        let mut state = self.state.lock().expect("p3 send span lifecycle poisoned");
        state.recorder = state
            .recorder
            .clone()
            .for_start(index, *observational_owner, !replay);
        state.started = Some((**started).clone());
        self.submit_cleanup(&mut state);
    }

    fn closed(&self, _finished: &SpanFinished) {
        self.closure_claimed.store(true, Ordering::Release);
    }
}

impl Drop for SendTerminalState {
    fn drop(&mut self) {
        if let Some(delivery) = self.delivery.take() {
            delivery.suppress();
        }
    }
}

impl SendTerminalObserver {
    pub(super) fn register<Ctx: WorkerCtx, U: Send + 'static>(
        store: &Accessor<U, DurableP3<Ctx>>,
        lifecycle: Arc<P3SendSpanLifecycle>,
    ) -> Result<Self, WorkerExecutorError> {
        let state = Arc::new(Mutex::new(SendTerminalState {
            consumption: None,
            delivery: None,
        }));
        if !store.has_guest_visible_subtask() {
            return Ok(Self {
                state,
                registered: false,
            });
        }
        let cleared = store
            .clear_terminal_observer()
            .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
        if cleared {
            return Err(WorkerExecutorError::runtime(
                "a completion-delivery observer of an earlier durable call was still armed when p3 HTTP send started on the same host subtask",
            ));
        }
        let observer_state = state.clone();
        let (replay_tx, replay_rx) = oneshot::channel();
        lifecycle.set_replay_sender(replay_tx);
        store.spawn(SpawnHttpCleanupReplay::<Ctx> {
            replay_rx,
            _phantom: PhantomData,
        });
        store
            .register_terminal_observer(move |consumption| {
                if consumption == TerminalConsumption::NotDelivered {
                    lifecycle.abandon();
                }
                let delivery = {
                    let mut state = observer_state.lock().expect("send terminal lock poisoned");
                    if let Some(delivery) = state.delivery.take() {
                        Some(delivery)
                    } else {
                        state.consumption = Some(consumption);
                        None
                    }
                };
                if let Some(delivery) = delivery {
                    settle_send_terminal(consumption, delivery);
                }
            })
            .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
        Ok(Self {
            state,
            registered: true,
        })
    }

    pub(super) async fn attach<Ctx: WorkerCtx, U: Send + 'static>(
        self,
        store: &Accessor<U, DurableP3<Ctx>>,
        mut delivery: crate::durable_host::concurrent::CompletionDelivery,
    ) -> Result<(), WorkerExecutorError> {
        if !self.registered {
            return delivery.deliver_at_accessor_terminal(store).await;
        }
        delivery.prepare_delivery(None).await?;
        let consumption = {
            let mut state = self.state.lock().expect("send terminal lock poisoned");
            if state.consumption.is_none() {
                state.delivery = Some(delivery);
                return Ok(());
            }
            state.consumption.take().unwrap()
        };
        settle_send_terminal(consumption, delivery);
        Ok(())
    }
}

pub(super) fn settle_send_terminal(
    consumption: TerminalConsumption,
    delivery: crate::durable_host::concurrent::CompletionDelivery,
) {
    match consumption {
        TerminalConsumption::Delivered => delivery.delivered(),
        TerminalConsumption::NotDelivered => {
            drop(delivery);
        }
        TerminalConsumption::Superseded => delivery.suppress(),
    }
}

impl<Ctx: WorkerCtx, U: Send + 'static> AccessorTask<U, DurableP3<Ctx>>
    for SpawnHttpCleanupReplay<Ctx>
{
    async fn run(self, accessor: &Accessor<U, DurableP3<Ctx>>) -> wasmtime::Result<()> {
        if let Ok(Some(replay)) = self.replay_rx.await {
            accessor.spawn(replay);
        }
        Ok(())
    }
}

/// Finishes the send's derived `outgoing-http-request` span in memory.
pub(in crate::durable_host::p3::http) async fn finish_p3_send_span<Ctx: WorkerCtx, U: 'static>(
    store: &Accessor<U, DurableP3<Ctx>>,
    span: &P3HttpSendSpan,
) -> Result<(), WorkerExecutorError> {
    store.with(|mut access| {
        let ctx = durable_worker_ctx::<Ctx, U>(access.data_mut());
        finish_span_in_memory(ctx, &span.span_id)
    })
}

/// Finishes the send span synchronously at a live deferred-completion site.
pub(in crate::durable_host::p3::http) fn finish_p3_send_span_in_memory<
    Ctx: WorkerCtx,
    U: 'static,
>(
    store: &Accessor<U, DurableP3<Ctx>>,
    span: &P3HttpSendSpan,
) -> Result<(), WorkerExecutorError> {
    store.with(|mut access| {
        let ctx = durable_worker_ctx::<Ctx, U>(access.data_mut());
        finish_span_in_memory(ctx, &span.span_id)
    })
}
