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

pub(super) fn accept_access_terminal_effect<Resp, R, A>(
    response: &Resp,
    oplog: &Arc<dyn Oplog>,
    end: OplogEntry,
    accept: A,
) -> Option<(R, crate::services::oplog::OplogAddReceipt)>
where
    A: FnOnce(&Resp, Box<dyn FnOnce(R) + '_>),
{
    let mut accepted = None;
    accept(
        response,
        Box::new(|reservation| {
            accepted = Some((reservation, oplog.enqueue_add(end)));
        }),
    );
    accepted
}

type TerminalEffectHandoff<R> = (
    oneshot::Sender<(R, crate::services::oplog::OplogAddReceipt)>,
    tokio::task::JoinHandle<Result<(), WorkerExecutorError>>,
);

pub(in crate::durable_host) fn prepare_access_terminal_effect<R, C, F, E, EF>(
    tasks: &crate::worker::tasks::WorkerTasks,
    recording: tokio_util::task::task_tracker::TaskTrackerToken,
    commit: C,
    publish: F,
    failed: E,
) -> Result<TerminalEffectHandoff<R>, WorkerExecutorError>
where
    R: Send + 'static,
    C: Future<Output = Result<(), WorkerExecutorError>> + Send + 'static,
    F: FnOnce(R, OplogIndex) + Send + 'static,
    E: FnOnce(WorkerExecutorError) -> EF + Send + 'static,
    EF: Future<Output = ()> + Send + 'static,
{
    let (accept, accepted) = oneshot::channel::<(R, crate::services::oplog::OplogAddReceipt)>();
    let (complete, completed) = oneshot::channel();
    tasks
        .spawn_metadata(async move {
            let _recording = recording;
            let Ok((reservation, append)) = accepted.await else {
                return;
            };
            let persisted = async {
                let index = append.await?;
                commit.await?;
                Ok::<_, WorkerExecutorError>(index)
            }
            .await;
            match persisted {
                Ok(index) => {
                    publish(reservation, index);
                    let _ = complete.send(Ok(()));
                }
                Err(error) => {
                    // Fence synchronously before releasing the reservation. Election may wait for
                    // cancellation cleanup to join this receipt, so resolve it before polling cleanup.
                    let cleanup = failed(error.clone());
                    drop(reservation);
                    let _ = complete.send(Err(error));
                    cleanup.await;
                }
            }
        })
        .map_err(WorkerExecutorError::runtime)?;
    let receipt = tokio::spawn(async move {
        completed.await.map_err(|error| {
            WorkerExecutorError::runtime(format!(
                "durable effect coordinator stopped without a receipt: {error}"
            ))
        })?
    });
    Ok((accept, receipt))
}

enum AccessTerminalGuardState {
    BeforeTerminal {
        call: DroppedCall,
    },
    CleanupAfterTerminal {
        function_type: DurableFunctionType,
        durable_begin_index: OplogIndex,
        terminal: Option<tokio::task::JoinHandle<Result<(), WorkerExecutorError>>>,
        trap_context: DurableCallTrapContext,
        /// Moved out of the armed [`DroppedCall`] when the terminal is handed to an owned task,
        /// so the call stays counted as in flight while the append is still pending.
        live_call_permit: Option<LiveCallPermit>,
        /// `Some` while a torn drop must record a `CompletionDiscarded` marker (successful `End`
        /// path, no error observed by the caller yet); see [`CompletionMarkerRecord`].
        discard_marker: Option<CompletionMarkerRecord>,
    },
    Disarmed,
}

pub(super) struct AccessTerminalGuard<P: DropPolicy> {
    state: AccessTerminalGuardState,
    teardown: TeardownProbe,
    /// Policy-controlled sink for unfinished drops (`BeforeTerminal`): `None` for policies (e.g.
    /// `NotCancellable`) that treat an unfinished drop as a programming error instead of queueing
    /// a cancellation.
    sink: Option<UnboundedSender<DropEvent>>,
    /// Unconditional sink for `CleanupAfterTerminal`: once a terminal task has been spawned, its
    /// join + scope close (and the in-flight permit it carries) must be handed to the next drain
    /// regardless of the drop policy.
    cleanup_sink: Option<UnboundedSender<DropEvent>>,
    _phantom: PhantomData<P>,
}

impl<P: DropPolicy> AccessTerminalGuard<P> {
    pub(super) fn new(
        call: DroppedCall,
        sink: Option<UnboundedSender<DropEvent>>,
        cleanup_sink: Option<UnboundedSender<DropEvent>>,
        teardown: TeardownProbe,
    ) -> Self {
        Self {
            state: AccessTerminalGuardState::BeforeTerminal { call },
            teardown,
            sink,
            cleanup_sink,
            _phantom: PhantomData,
        }
    }

    pub(super) fn call(&self) -> Option<&DroppedCall> {
        match &self.state {
            AccessTerminalGuardState::BeforeTerminal { call } => Some(call),
            _ => None,
        }
    }

    /// Hands the terminal append to an owned task. `discard_marker` must be `Some` only on the
    /// successful-`End` path (a torn drop from that state means the guest silently discarded the
    /// persisted completion); cancellation terminals pass `None`.
    pub(super) fn cleanup_after_terminal(
        &mut self,
        terminal: tokio::task::JoinHandle<Result<(), WorkerExecutorError>>,
        discard_marker: Option<CompletionMarkerRecord>,
    ) {
        let call = match std::mem::replace(&mut self.state, AccessTerminalGuardState::Disarmed) {
            AccessTerminalGuardState::BeforeTerminal { call } => call,
            _ => panic!("cleanup_after_terminal called before the terminal is armed"),
        };
        self.state = AccessTerminalGuardState::CleanupAfterTerminal {
            function_type: call.function_type().clone(),
            durable_begin_index: call.begin_index(),
            terminal: Some(terminal),
            trap_context: call.trap_context(),
            // The permit moves with the state transition: the call remains counted as in flight
            // while the owned terminal append is pending.
            live_call_permit: call.live_call_permit,
            discard_marker,
        };
    }

    /// Disarms only the `CompletionDiscarded` marker while keeping the terminal cleanup armed.
    /// Called on internal-error returns *after* the `End` was persisted (durable-scope close
    /// failure): the caller observes the error and the worker traps,
    /// so the completion was not silently discarded by the guest and no marker may be recorded —
    /// but the pending terminal join / permit release must still reach the drain.
    pub(super) fn suppress_discard_marker(&mut self) {
        if let AccessTerminalGuardState::CleanupAfterTerminal { discard_marker, .. } =
            &mut self.state
        {
            *discard_marker = None;
        }
    }

    pub(super) async fn wait_terminal(&mut self) -> Result<(), WorkerExecutorError> {
        if let AccessTerminalGuardState::CleanupAfterTerminal { terminal, .. } = &mut self.state {
            if let Some(handle) = terminal {
                handle.await.map_err(|err| {
                    WorkerExecutorError::runtime(format!(
                        "durable call terminal recorder task failed: {err}"
                    ))
                })??;
            }
            *terminal = None;
        }
        Ok(())
    }

    pub(super) fn disarm(&mut self) {
        self.state = AccessTerminalGuardState::Disarmed;
    }

    /// Converts a fully completed terminal guard (terminal joined, scope closed) into a deferred
    /// guest-delivery token: the armed discard marker, the in-flight permit and the trap context
    /// move to the [`CompletionDelivery`], which owns the silent-discard reconciliation from here
    /// to the final guest-facing boundary. Falls back to an unarmed token when the call was not
    /// persisted (no marker to reconcile).
    pub(super) fn take_completion_delivery(&mut self) -> CompletionDelivery {
        match std::mem::replace(&mut self.state, AccessTerminalGuardState::Disarmed) {
            AccessTerminalGuardState::CleanupAfterTerminal {
                terminal,
                trap_context,
                live_call_permit,
                discard_marker: Some(marker),
                ..
            } => CompletionDelivery {
                state: CompletionDeliveryState::Live(Box::new(LiveDelivery {
                    marker,
                    trap_context,
                    live_call_permit,
                    cleanup_sink: self.cleanup_sink.clone(),
                    // Normally already joined (`wait_terminal`) by the time the guard is
                    // converted; chained defensively so a still-pending terminal append can never
                    // be overtaken by a marker append.
                    pending_append: terminal.map(OrderedAppend::Task),
                    teardown: self.teardown.clone(),
                })),
            },
            _ => CompletionDelivery::unarmed(),
        }
    }
}

impl<P: DropPolicy> Drop for AccessTerminalGuard<P> {
    fn drop(&mut self) {
        match std::mem::replace(&mut self.state, AccessTerminalGuardState::Disarmed) {
            AccessTerminalGuardState::BeforeTerminal { mut call } => {
                if self.teardown.is_executor_shutting_down() {
                    tracing::debug!(
                        start_idx = %call.start_idx(),
                        "durable call terminal abandoned during executor shutdown"
                    );
                } else if self.teardown.is_runtime_tearing_down() {
                    // Runtime abandonment is structural: release resident ownership without
                    // applying the guest-drop policy or recording a semantic terminal.
                    call.span_finished = None;
                } else {
                    if let Some(span) = &mut call.span_finished {
                        span.finished_at = Timestamp::now_utc();
                    }
                    P::unfinished_drop(call, self.sink.as_ref());
                }
            }
            AccessTerminalGuardState::CleanupAfterTerminal {
                function_type,
                durable_begin_index,
                terminal,
                trap_context,
                live_call_permit,
                discard_marker,
            } => {
                // A torn drop with the marker still armed: queue a discarded marker after the
                // terminal task, then let a receipt-waiting task take the terminal's place in the
                // cleanup event so invocation completion awaits the marker too. A tear caused by
                // runtime teardown is not a guest discard: the `End` stays markerless so replay
                // delivers it (see [`TeardownProbe`]).
                let terminal = match discard_marker {
                    Some(marker) if !self.teardown.is_tearing_down() => {
                        Some(task_for_marker_receipt(marker.record(
                            CompletionMarkerKind::Discarded,
                            terminal.map(OrderedAppend::Task),
                        )))
                    }
                    Some(marker) => {
                        tracing::debug!(
                            start_idx = %marker.start_idx,
                            "terminal guard abandoned during runtime teardown; leaving the end markerless"
                        );
                        terminal
                    }
                    None => terminal,
                };
                if let Some(sink) = &self.cleanup_sink {
                    let _ = sink.send(DropEvent::CleanupAfterTerminal {
                        function_type,
                        durable_begin_index,
                        terminal,
                        trap_context,
                        live_call_permit,
                    });
                }
            }
            AccessTerminalGuardState::Disarmed => {}
        }
    }
}
