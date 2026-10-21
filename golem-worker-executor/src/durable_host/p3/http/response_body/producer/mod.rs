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

/// A demand from the body stream producer to the durable [`HttpConsumeBodyTask`].
pub(in crate::durable_host::p3::http) enum HttpBodyDemand {
    /// Read and durably persist the next body chunk, replying on the channel.
    Read {
        reply: oneshot::Sender<HttpBodyChunkReply>,
        cancel: oneshot::Receiver<()>,
        cancel_ack: oneshot::Sender<()>,
        guest: bool,
    },
    /// The guest dropped/cancelled the stream with no read in flight. Finalize
    /// the durable consume-body parent without reading more upstream bytes, then
    /// acknowledge so the guest invocation cannot return with an incomplete
    /// parent scope in the oplog.
    Cancel(oneshot::Sender<()>),
}

/// The task's reply to a single producer demand.
pub(in crate::durable_host::p3::http) enum HttpBodyChunkReply {
    /// One non-empty body frame, already persisted to the oplog as a `Data`
    /// child chunk before being handed back for delivery to the guest.
    Data {
        bytes: Bytes,
        delivery: HttpBodyChunkDelivery,
    },
    /// The body stream reached its terminal (clean EOF, trailers, or a body
    /// error); there are no more bytes to deliver.
    End { delivery: HttpBodyChunkDelivery },
    /// The guest cancelled this pending body read before upstream bytes arrived.
    Cancelled { delivery: HttpBodyChunkDelivery },
    /// A durable failure occurred while persisting/replaying the body; the guest
    /// stream traps with this message, tagged with the failing call scope's trap
    /// context so post-trap retry grouping stays owned by that call.
    Failed {
        message: String,
        trap_context: DurableCallTrapContext,
    },
}

/// Settles a persisted frame when its first bytes (or EOF) reach the reader. Guest reads settle
/// through Wasmtime's consumption observer, not when the producer queues a buffer. Host reads
/// and explicit cancellation acknowledgements retain the producer handoff boundary.
pub(in crate::durable_host::p3::http) struct HttpBodyChunkDelivery {
    delivery: Option<CompletionDelivery>,
    observed: Option<oneshot::Sender<()>>,
}

impl HttpBodyChunkDelivery {
    pub(super) fn new(delivery: CompletionDelivery) -> (Self, oneshot::Receiver<()>) {
        let (observed, observation) = oneshot::channel();
        (
            Self {
                delivery: Some(delivery),
                observed: Some(observed),
            },
            observation,
        )
    }

    fn observed(mut self) {
        self.delivery
            .take()
            .expect("body chunk delivery is settled exactly once")
            .delivered();
        let _ = self
            .observed
            .take()
            .expect("body chunk observation is sent exactly once")
            .send(());
    }

    fn observe_read<D>(
        self,
        store: &mut StoreContextMut<'_, D>,
        destination: &mut Destination<'_, u8, Bytes>,
    ) -> wasmtime::Result<()> {
        if destination.remaining(&mut *store).is_some() {
            destination.register_read_observer(&mut *store, move |consumption| {
                match consumption {
                    TerminalConsumption::Delivered => self.observed(),
                    TerminalConsumption::NotDelivered => drop(self),
                    TerminalConsumption::Superseded => {
                        // Dropping the token poisons replay rather than releasing a recorded
                        // guest boundary that was not reproduced.
                        tracing::error!("HTTP body read observer was superseded");
                        drop(self);
                    }
                }
            })
        } else {
            self.observed();
            Ok(())
        }
    }
}

/// Resolution delivered to the guest-facing trailers future once the body closes
/// (or the durable task fails before recording the terminal).
pub(in crate::durable_host::p3::http) enum HttpTrailersResolution {
    /// The body terminal: clean trailers (or a body `ErrorCode`).
    Outcome(HttpTrailersOutcome),
    /// A durability failure: the trailers future traps with this message, tagged
    /// with the failing call scope's trap context.
    Trap {
        message: String,
        trap_context: DurableCallTrapContext,
    },
}

enum HttpTrailersDeliveryState {
    Awaiting,
    Armed(CompletionDelivery),
    Observed(Option<TerminalConsumption>),
    Done,
}

pub(super) struct HttpTrailersDelivery {
    state: Mutex<HttpTrailersDeliveryState>,
}

impl HttpTrailersDelivery {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(HttpTrailersDeliveryState::Awaiting),
        })
    }

    pub(super) fn arm(&self, delivery: CompletionDelivery) {
        let observed = {
            let mut state = self.state.lock().expect("trailers delivery lock poisoned");
            match &*state {
                HttpTrailersDeliveryState::Awaiting => {
                    *state = HttpTrailersDeliveryState::Armed(delivery);
                    return;
                }
                HttpTrailersDeliveryState::Observed(observed) => {
                    let observed = *observed;
                    *state = HttpTrailersDeliveryState::Done;
                    observed
                }
                HttpTrailersDeliveryState::Armed(_) | HttpTrailersDeliveryState::Done => {
                    unreachable!("trailers delivery is armed exactly once")
                }
            }
        };
        Self::settle(delivery, observed);
    }

    pub(super) fn observe(&self, consumption: TerminalConsumption) {
        let delivery = {
            let mut state = self.state.lock().expect("trailers delivery lock poisoned");
            match std::mem::replace(&mut *state, HttpTrailersDeliveryState::Done) {
                HttpTrailersDeliveryState::Awaiting => {
                    *state = HttpTrailersDeliveryState::Observed(Some(consumption));
                    return;
                }
                HttpTrailersDeliveryState::Armed(delivery) => delivery,
                HttpTrailersDeliveryState::Observed(_) | HttpTrailersDeliveryState::Done => {
                    unreachable!("trailers terminal observer is invoked at most once")
                }
            }
        };
        Self::settle(delivery, Some(consumption));
    }

    pub(super) fn abandon(&self) {
        let delivery = {
            let mut state = self.state.lock().expect("trailers delivery lock poisoned");
            match std::mem::replace(&mut *state, HttpTrailersDeliveryState::Done) {
                HttpTrailersDeliveryState::Awaiting => {
                    *state = HttpTrailersDeliveryState::Observed(None);
                    return;
                }
                HttpTrailersDeliveryState::Armed(delivery) => delivery,
                HttpTrailersDeliveryState::Observed(_) | HttpTrailersDeliveryState::Done => return,
            }
        };
        delivery.suppress();
    }

    fn settle(delivery: CompletionDelivery, consumption: Option<TerminalConsumption>) {
        match consumption {
            Some(TerminalConsumption::Delivered) => delivery.delivered(),
            Some(TerminalConsumption::NotDelivered) => drop(delivery),
            Some(TerminalConsumption::Superseded) => {
                tracing::error!("consume-body trailers terminal observer was superseded");
                delivery.suppress();
            }
            None => delivery.suppress(),
        }
    }
}

pub(super) struct HttpTrailersDeliveryGuard {
    delivery: Arc<HttpTrailersDelivery>,
    observed: bool,
}

impl HttpTrailersDeliveryGuard {
    pub(super) fn new(delivery: Arc<HttpTrailersDelivery>) -> Self {
        Self {
            delivery,
            observed: false,
        }
    }

    pub(super) fn observe(mut self, consumption: TerminalConsumption) {
        self.observed = true;
        self.delivery.observe(consumption);
    }
}

impl Drop for HttpTrailersDeliveryGuard {
    fn drop(&mut self) {
        if !self.observed {
            self.delivery.abandon();
        }
    }
}

/// Body stream returned to the guest from `consume-body`.
///
/// `consume-body` is a *synchronous* host function but durable persistence is
/// async, so the producer never touches the oplog (or the upstream body)
/// itself. Instead it bridges to the spawned [`HttpConsumeBodyTask`] with a
/// demand/reply protocol: when the guest needs more bytes the producer sends a
/// demand and parks; the task reads (live) or replays (on replay) exactly one
/// body frame, persists/claims it as a child durable call, and replies with the
/// bytes. The producer retains the frame's suffix across read-sized buffers, so the read observer
/// covers only the bytes accepted by that read. No further durable demand is made until the suffix
/// is drained. A fresh demand always parks; an already recorded suffix is available inline, both
/// live and on replay.
pub(in crate::durable_host::p3::http) struct DurableHttpBodyProducer {
    demand_tx: mpsc::Sender<HttpBodyDemand>,
    demand_signal: Arc<HttpBodyDemandSignal>,
    hook: Option<Arc<dyn P3HttpBodyProducerHook>>,
    pending: Option<PendingHttpBodyRead>,
    pending_cancel: Option<oneshot::Receiver<()>>,
    buffered: Bytes,
    finished: bool,
}

pub(in crate::durable_host::p3::http) struct PendingHttpBodyRead {
    reply: oneshot::Receiver<HttpBodyChunkReply>,
    ready_reply: Option<HttpBodyChunkReply>,
    cancel: Option<oneshot::Sender<()>>,
    cancel_ack: Option<oneshot::Receiver<()>>,
    cancelling: bool,
}

impl DurableHttpBodyProducer {
    pub(super) fn new(
        demand_tx: mpsc::Sender<HttpBodyDemand>,
        demand_signal: Arc<HttpBodyDemandSignal>,
        hook: Option<Arc<dyn P3HttpBodyProducerHook>>,
    ) -> Self {
        Self {
            demand_tx,
            demand_signal,
            hook,
            pending: None,
            pending_cancel: None,
            buffered: Bytes::new(),
            finished: false,
        }
    }
}

impl<D> StreamProducer<D> for DurableHttpBodyProducer {
    type Item = u8;
    type Buffer = Bytes;

    fn poll_produce<'a>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: StoreContextMut<'a, D>,
        mut dst: Destination<'a, Self::Item, Self::Buffer>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        loop {
            if self.finished {
                return Poll::Ready(Ok(StreamResult::Dropped));
            }

            if !self.buffered.is_empty() {
                let capacity = dst.remaining(&mut store).unwrap_or(self.buffered.len());
                let count = capacity.min(self.buffered.len());
                dst.set_buffer(self.buffered.split_to(count));
                return Poll::Ready(Ok(StreamResult::Completed));
            }

            if let Some(rx) = self.pending_cancel.as_mut() {
                match Pin::new(rx).poll(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(_) => {
                        self.pending_cancel = None;
                        self.finished = true;
                        return Poll::Ready(Ok(StreamResult::Cancelled));
                    }
                }
            }

            let hook = self.hook.clone();
            if let Some(pending) = self.pending.as_mut() {
                if finish && !pending.cancelling {
                    if let Some(cancel) = pending.cancel.take() {
                        let _ = cancel.send(());
                    }
                    pending.cancelling = true;
                }
                let reply = match pending.ready_reply.take() {
                    Some(reply) => Poll::Ready(Ok(reply)),
                    None => Pin::new(&mut pending.reply).poll(cx),
                };
                let reply = match reply {
                    Poll::Ready(Ok(reply))
                        if !finish
                            && hook
                                .as_ref()
                                .is_some_and(|hook| hook.should_defer_ready_reply()) =>
                    {
                        pending.ready_reply = Some(reply);
                        hook.expect("the checked hook must exist")
                            .ready_reply_deferred();
                        return Poll::Pending;
                    }
                    reply => reply,
                };
                match reply {
                    Poll::Pending => {
                        // A reply that is already queued wins over cancellation: it may contain
                        // bytes that were copied into guest memory before cancel-read completed
                        // live. Only settle cancellation when no reply is ready.
                        if pending.cancelling
                            && let Some(cancel_ack) = pending.cancel_ack.as_mut()
                        {
                            match Pin::new(cancel_ack).poll(cx) {
                                Poll::Ready(_) => {
                                    self.pending = None;
                                    self.finished = true;
                                    return Poll::Ready(Ok(StreamResult::Cancelled));
                                }
                                Poll::Pending => {}
                            }
                        }
                        // A demand is in flight. If `finish` was set above, the
                        // durable task has also been signalled to stop the
                        // upstream read and record a terminal, so this pending
                        // wait is bounded by durable finalization rather than by
                        // the remote peer producing more body bytes.
                        return Poll::Pending;
                    }
                    Poll::Ready(Ok(HttpBodyChunkReply::Data {
                        mut bytes,
                        delivery,
                    })) => {
                        self.pending = None;
                        if bytes.is_empty() {
                            continue;
                        }
                        let capacity = dst.remaining(&mut store).unwrap_or(bytes.len());
                        let count = capacity.min(bytes.len());
                        delivery.observe_read(&mut store, &mut dst)?;
                        dst.set_buffer(bytes.split_to(count));
                        self.buffered = bytes;
                        return Poll::Ready(Ok(StreamResult::Completed));
                    }
                    Poll::Ready(Ok(HttpBodyChunkReply::End { delivery })) => {
                        let cancelling = pending.cancelling;
                        let cancel_ack = pending.cancel_ack.take();
                        self.pending = None;
                        if cancelling {
                            // A cancellation acknowledgement is not a successful EOF read. It
                            // must release the task before waiting for parent finalization.
                            delivery.observed();
                            if let Some(cancel_ack) = cancel_ack {
                                self.pending_cancel = Some(cancel_ack);
                                continue;
                            }
                            self.finished = true;
                            return Poll::Ready(Ok(StreamResult::Cancelled));
                        } else {
                            delivery.observe_read(&mut store, &mut dst)?;
                            self.finished = true;
                            return Poll::Ready(Ok(StreamResult::Dropped));
                        }
                    }
                    Poll::Ready(Ok(HttpBodyChunkReply::Cancelled { delivery })) => {
                        let cancel_ack = pending.cancel_ack.take();
                        self.pending = None;
                        delivery.observed();
                        if let Some(cancel_ack) = cancel_ack {
                            self.pending_cancel = Some(cancel_ack);
                            continue;
                        }
                        self.finished = true;
                        return Poll::Ready(Ok(StreamResult::Cancelled));
                    }
                    Poll::Ready(Ok(HttpBodyChunkReply::Failed {
                        message,
                        trap_context,
                    })) => {
                        self.pending = None;
                        self.finished = true;
                        return Poll::Ready(Err(wasmtime::Error::from_anyhow(
                            mark_durable_call_trap_context(
                                anyhow::Error::msg(message),
                                trap_context,
                            ),
                        )));
                    }
                    Poll::Ready(Err(_)) => {
                        self.finished = true;
                        return Poll::Ready(Err(wasmtime::Error::msg(
                            "consume-body durable task dropped before replying",
                        )));
                    }
                }
            }

            // No demand in flight.
            if dst.remaining(&mut store) == Some(0) {
                // Zero-length read: the guest is probing readiness, not reading.
                // Do not turn this into a durable body read.
                return Poll::Ready(Ok(StreamResult::Completed));
            }
            if finish {
                // The guest is cancelling the stream and we have nothing
                // buffered and no demand in flight. Ask the durable task to
                // finalize the parent scope and wait for that acknowledgement;
                // otherwise a component that drops the body and returns
                // immediately can leave an incomplete consume-body scope in the
                // oplog and fail on replay.
                let (tx, rx) = oneshot::channel();
                match self.demand_tx.try_send(HttpBodyDemand::Cancel(tx)) {
                    Ok(()) => self.demand_signal.observe(),
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        self.finished = true;
                        return Poll::Ready(Ok(StreamResult::Cancelled));
                    }
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        // The producer keeps at most one demand in flight, so the
                        // capacity-1 channel can never be full here.
                        self.finished = true;
                        return Poll::Ready(Err(wasmtime::Error::msg(
                            "consume-body demand channel unexpectedly full",
                        )));
                    }
                }
                self.pending_cancel = Some(rx);
                continue;
            }

            let (reply_tx, reply_rx) = oneshot::channel();
            let (cancel_tx, cancel_rx) = oneshot::channel();
            let (cancel_ack_tx, cancel_ack_rx) = oneshot::channel();
            match self.demand_tx.try_send(HttpBodyDemand::Read {
                reply: reply_tx,
                cancel: cancel_rx,
                cancel_ack: cancel_ack_tx,
                guest: dst.remaining(&mut store).is_some(),
            }) {
                Ok(()) => self.demand_signal.observe(),
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    self.finished = true;
                    return Poll::Ready(Err(wasmtime::Error::msg(
                        "consume-body durable task is gone",
                    )));
                }
                Err(mpsc::error::TrySendError::Full(_)) => {
                    // The producer keeps at most one demand in flight, so the
                    // capacity-1 channel can never be full here.
                    self.finished = true;
                    return Poll::Ready(Err(wasmtime::Error::msg(
                        "consume-body demand channel unexpectedly full",
                    )));
                }
            }
            self.pending = Some(PendingHttpBodyRead {
                reply: reply_rx,
                ready_reply: None,
                cancel: Some(cancel_tx),
                cancel_ack: Some(cancel_ack_rx),
                cancelling: false,
            });
            // Loop to register the receiver's waker (the reply is not ready yet).
        }
    }
}

/// Guest-facing trailers `FutureReader` producer. Awaits the terminal trailers
/// from the durable task and, only when read, materializes a `trailers`
/// resource in the store table.
pub(in crate::durable_host::p3::http) struct HttpTrailersFutureProducer<Ctx, U> {
    rx: oneshot::Receiver<HttpTrailersResolution>,
    _phantom: PhantomData<fn() -> (Ctx, U)>,
}

impl<Ctx, U> HttpTrailersFutureProducer<Ctx, U> {
    pub(super) fn new(rx: oneshot::Receiver<HttpTrailersResolution>) -> Self {
        Self {
            rx,
            _phantom: PhantomData,
        }
    }
}

impl<Ctx, U> FutureProducer<U> for HttpTrailersFutureProducer<Ctx, U>
where
    Ctx: WorkerCtx,
    U: 'static,
{
    type Item = Result<Option<Resource<Trailers>>, ErrorCode>;

    fn poll_produce(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: StoreContextMut<U>,
        finish: bool,
    ) -> Poll<wasmtime::Result<Option<Self::Item>>> {
        let this = self.get_mut();
        match Pin::new(&mut this.rx).poll(cx) {
            Poll::Pending if finish => Poll::Ready(Ok(None)),
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(HttpTrailersResolution::Outcome(outcome))) => {
                let item = match outcome {
                    Ok(None) => Ok(None),
                    Ok(Some(headers)) => {
                        let view = wasi_http_view::<Ctx, U>(store.data_mut());
                        match view.table.push(FieldMap::new_immutable(headers)) {
                            Ok(resource) => Ok(Some(resource)),
                            Err(err) => {
                                return Poll::Ready(Err(wasmtime::Error::from(err)
                                    .context("failed to push consume-body trailers to table")));
                            }
                        }
                    }
                    Err(error) => Err(error),
                };
                Poll::Ready(Ok(Some(item)))
            }
            // A durability failure occurred before the terminal was recorded: the
            // trailers future must trap (carrying the failing call scope's trap
            // context) rather than resolve to a normal error that would mask it.
            Poll::Ready(Ok(HttpTrailersResolution::Trap {
                message,
                trap_context,
            })) => Poll::Ready(Err(wasmtime::Error::from_anyhow(
                mark_durable_call_trap_context(anyhow::Error::msg(message), trap_context),
            ))),
            // The channel is closed without any resolution: the durable task was
            // aborted before sending. On the normal path the task always sends a
            // resolution before dropping the sender, so a closed channel here is
            // a durability failure and must trap rather than resolve to a normal
            // error that would mask it.
            Poll::Ready(Err(_)) => Poll::Ready(Err(wasmtime::Error::msg(
                "consume-body durable task dropped before resolving trailers",
            ))),
        }
    }
}

/// Waits for the reader's delivery boundary after a child terminal is durable. Queueing a reply
/// or installing a Wasmtime buffer is not guest consumption; the read observer settles ordinary
/// data and EOF. Explicit cancellation acknowledgements settle at the producer handoff.
pub(super) async fn deliver_http_body_reply(
    activity: &TailActivity,
    mut demand: oneshot::Sender<HttpBodyChunkReply>,
    mut delivery: CompletionDelivery,
    guest: bool,
    reply: impl FnOnce(HttpBodyChunkDelivery) -> HttpBodyChunkReply,
) -> Result<DemandDelivery, WorkerExecutorError> {
    if delivery.is_replay_discarded() {
        debug!(
            "recorded consume-body completion was discarded before delivery; parking until the \
             replayed guest drops the body reader"
        );
        activity.park(demand.closed()).await;
        return Ok(DemandDelivery::Abandoned);
    }

    tokio::select! {
        biased;
        _ = demand.closed() => {
            delivery.discarded().await?;
            return Ok(DemandDelivery::Abandoned);
        }
        result = async {
            if guest {
                delivery.prepare_delivery(Some(activity)).await
            } else {
                delivery.prepare_host_delivery(Some(activity)).await
            }
        } => result?,
    }
    let (delivery, observed) = HttpBodyChunkDelivery::new(delivery);
    if let Err(reply) = demand.send(reply(delivery)) {
        // Dropping an armed delivery reserves the discard marker before waking this task.
        drop(reply);
    }

    if observed.await.is_ok() {
        Ok(DemandDelivery::Delivered)
    } else {
        debug!(
            "consume-body completion persisted but the guest dropped the body reader before \
             observing it"
        );
        Ok(DemandDelivery::Abandoned)
    }
}

#[cfg(test)]
mod tests;
