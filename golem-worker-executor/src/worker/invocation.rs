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

use golem_common::model::InvocationId;

use crate::durable_host::durability::{
    ClassifiedHostError, DurabilityHost, DurableCallTrapContextMarker, DurableRecoveryFailure,
    DurableRecoveryFailureKind, SemanticTrapRetryOverrideMarker,
};
use crate::durable_host::schema_value_stream::StoreValueResolver;
use crate::durable_host::tool::operation::OwnerFailureWinner;
use crate::metrics::wasm::{record_invocation, record_invocation_consumption};
use crate::model::{FailureProvenance, TrapType};
use crate::preview2::exports::golem::agent::guest as guest_exports;
use crate::preview2::exports::golem::api1_5_0::load_snapshot as load_snapshot_exports;
use crate::preview2::exports::golem::api1_5_0::save_snapshot as save_snapshot_exports;
use crate::preview2::oplog_processor_plugin::exports::golem::api1_5_0::oplog_processor as oplog_processor_exports;
use crate::preview2::{golem_agent, golem_api_1_x};
use crate::services::HasWorker;
use crate::services::oplog::UnreadableOplog;
use crate::workerctx::{PublicWorkerIo, ResultRecordedAt, WorkerCtx};
use futures::FutureExt;
use golem_common::model::AgentId;
use golem_common::model::InterruptKind;
use golem_common::model::agent::{AgentMode, ParsedAgentKey, ResolvedOwnerContext};
use golem_common::model::component_metadata::{AgentMethodStreamMetadata, ComponentMetadata};
use golem_common::model::oplog::AgentError as OplogAgentError;
use golem_common::model::tool::{ToolActivationSnapshot, ToolName};
use golem_common::model::{
    AgentInvocation, AgentInvocationOutput, AgentInvocationResult, OplogIndex,
};
use golem_common::schema::SchemaValue;
#[cfg(test)]
use golem_common::schema::agent::InputSchema;
use golem_common::schema::agent::wit::decode_agent_error_rejecting_quota_with;
use golem_common::schema::agent::{AgentMethodSchema, AgentTypeSchema};
use golem_common::schema::graph::SchemaGraph;
use golem_common::schema::graph::TypedSchemaValue;
use golem_common::schema::schema_type::SchemaType;
use golem_common::schema::validation::value::validate_value;
use golem_schema::schema::wit::wire as core_wire;
use golem_schema::schema::wit::{decode_value_with, encode_value_with, encode_value_with_streams};
use golem_service_base::error::worker_executor::{GolemSpecificWasmTrap, WorkerExecutorError};
use std::future::Future;
use std::pin::Pin;
use tracing::{Instrument, Level, debug, span};
use wasmtime::component::Accessor;
use wasmtime::{AsContextMut, StoreContextMut};

pub(crate) const INVOCATION_STACK_SIZE: usize = 16 * 1024 * 1024;

impl<Ctx: WorkerCtx> super::Worker<Ctx> {
    pub(crate) async fn start_pending_invocation(
        self: &std::sync::Arc<Self>,
        idempotency_key: InvocationId,
        operation: golem_common::model::InvocationRequestOperation,
        payload: golem_common::model::oplog::OplogPayload<
            golem_common::model::StoredAgentInvocationPayload,
        >,
        context: golem_common::model::invocation_context::InvocationContextStack,
        wallet_pin: golem_common::model::card::InvocationWalletPin,
        selected_preparation: Option<(super::DurableStreamingInvocationRequest, Option<String>)>,
    ) -> Result<Option<OplogIndex>, WorkerExecutorError> {
        if let Some((request, _)) = &selected_preparation {
            let memory = golem_common::serialization::serialize(&(
                &payload,
                &request.attempt,
                &request.foreign_mappings,
            ))
            .map_err(WorkerExecutorError::runtime)?
            .len();
            let worker = self.clone();
            return self
                .durable_stream_producer()
                .await?
                .run_admitted(None, memory, false, move |_, admission| async move {
                    Ok::<_, crate::durable_host::durable_stream::SessionError>(
                        worker
                            .start_pending_invocation_owned(
                                idempotency_key,
                                operation,
                                payload,
                                context,
                                wallet_pin,
                                selected_preparation,
                                Some(admission),
                            )
                            .await,
                    )
                })
                .await
                .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?;
        }
        self.start_pending_invocation_owned(
            idempotency_key,
            operation,
            payload,
            context,
            wallet_pin,
            None,
            None,
        )
        .await
    }

    async fn start_pending_invocation_owned(
        &self,
        idempotency_key: InvocationId,
        operation: golem_common::model::InvocationRequestOperation,
        payload: golem_common::model::oplog::OplogPayload<
            golem_common::model::StoredAgentInvocationPayload,
        >,
        context: golem_common::model::invocation_context::InvocationContextStack,
        wallet_pin: golem_common::model::card::InvocationWalletPin,
        selected_preparation: Option<(super::DurableStreamingInvocationRequest, Option<String>)>,
        admission: Option<
            std::sync::Arc<crate::durable_host::durable_stream::StreamWriteAdmission>,
        >,
    ) -> Result<Option<OplogIndex>, WorkerExecutorError> {
        use crate::services::oplog::{CommitLevel, OplogOps};

        let session_guard = if selected_preparation.is_some() {
            let producer = self.durable_stream_producer().await?;
            let key = producer.qualify_session(
                &golem_common::model::durable_stream::StreamRegistrationInvocation::Local(
                    idempotency_key,
                ),
            );
            Some(producer.session_lock(&key).lock_owned().await)
        } else {
            None
        };
        // The running loop must not wait for a stopping instance to join this loop.
        let instance = self.instance.lock().await;
        if !matches!(instance.state(), super::WorkerInstance::Running(_))
            || self.owner_retirement_requested.is_cancelled()
        {
            return Ok(None);
        }
        let status = self.get_repaired_status().await?;
        if !status
            .pending_invocations
            .iter()
            .any(|pending| pending.has_idempotency_key(&idempotency_key))
        {
            return Ok(None);
        }
        if session_guard.is_some()
            && self
                .durable_stream_session_status(&idempotency_key)
                .await?
                .is_some_and(|status| {
                    status.expired
                        || status.cancellation_requested
                        || status.expiry_deadline_millis.is_some_and(|deadline| {
                            deadline <= golem_common::model::Timestamp::now_utc().to_millis()
                        })
                })
        {
            self.add_and_commit_oplog_internal(
                &instance,
                golem_common::model::oplog::OplogEntry::cancel_pending_invocation(idempotency_key),
                Some(super::WorkerCommand::WorkAvailable),
            )
            .await?;
            return Ok(None);
        }
        if let Some((request, route_scope)) = selected_preparation {
            use golem_common::model::durable_stream::{
                DURABLE_STREAM_FORMAT_VERSION, StreamBindingRecord, StreamRegistrationInvocation,
                StreamSessionPreparedRecord, StreamTopologyPreparedRecord,
            };
            let producer = self.durable_stream_producer().await?;
            let streams = crate::durable_host::durable_session::StreamSession::new(
                producer.clone(),
                self.oplog.clone(),
                StreamRegistrationInvocation::Local(idempotency_key),
                std::iter::empty(),
            );
            let topologies = request
                .foreign_mappings
                .iter()
                .filter(|mapping| !producer.owns_handle_identity(&mapping.handle))
                .map(|mapping| {
                    Ok(StreamTopologyPreparedRecord {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        session_key: request.attempt.session_key.clone(),
                        attachment: streams
                            .attachment_key(&mapping.handle, producer.attachment_epoch_floor())
                            .map_err(|error| {
                                error.into_worker_executor_error(WorkerExecutorError::runtime)
                            })?,
                        mapping: mapping.clone(),
                    })
                })
                .collect::<Result<Vec<_>, WorkerExecutorError>>()?;
            let foreign = request
                .foreign_mappings
                .iter()
                .map(StreamBindingRecord::foreign)
                .collect::<Vec<_>>();
            let mut attempt = request.attempt;
            attempt.invocation.stream_handles = request
                .foreign_mappings
                .iter()
                .map(|mapping| mapping.handle.clone())
                .collect();
            let started = self.oplog.agent_invocation_started_entry_from_payload(
                idempotency_key,
                operation,
                payload,
                context,
                wallet_pin,
            );
            let admission = admission.expect("selected preparation holds producer admission");
            admission
                .submit(move |producer, write_context| async move {
                    producer
                        .prepare_session(
                            Some(&write_context),
                            request.registrations,
                            topologies,
                            started,
                            request.acceptance_committed,
                            move |bindings| StreamSessionPreparedRecord {
                                format_version: DURABLE_STREAM_FORMAT_VERSION,
                                session_key: idempotency_key,
                                public_session_id: request.public_session_id,
                                route_scope,
                                expiry_policy: request.expiry_policy,
                                expiry_deadline_millis: request.expiry_deadline_millis,
                                stream_mappings: if foreign.is_empty() {
                                    bindings
                                } else {
                                    foreign
                                },
                                attempt,
                            },
                        )
                        .await
                })
                .await
                .map_err(
                    |error: crate::durable_host::durable_stream::StreamStoreError| {
                        error.into_worker_executor_error(WorkerExecutorError::runtime)
                    },
                )?;
            drop(instance);
            drop(session_guard);
            admission
                .wait_published()
                .await
                .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?;
            let status = self.get_repaired_status().await?;
            return status
                .running
                .as_ref()
                .filter(|running| running.key == idempotency_key)
                .map(|running| Some(running.request.index))
                .ok_or_else(|| {
                    WorkerExecutorError::runtime(
                        "committed streaming start is absent from agent status",
                    )
                });
        }
        let index = self
            .oplog
            .add_agent_invocation_started_from_pending(
                idempotency_key,
                operation,
                payload,
                context,
                wallet_pin,
            )
            .await
            .map_err(|err| match err.into_refusal() {
                Ok(refusal) => self.refused_by(refusal),
                Err(err) => panic!(
                    "could not encode agent invocation on {}: {err}",
                    self.agent_id()
                ),
            })?;
        // The commit also stores a deferred `AgentInvocationSettled` of the previous invocation,
        // and its fold covers both entries in order.
        self.commit_oplog_and_update_state(CommitLevel::Always)
            .await?;
        self.settlement_committed();
        Ok(Some(index))
    }
}

/// Grows smaller native thread stacks for worker construction and nested invocation futures.
/// The temporary stack is released after each poll, before the future yields. A poll must not
/// suspend a Wasmtime fiber while on the temporary stack. Remaining-stack estimation uses native
/// thread bounds, so growth is not guaranteed when already running on a Wasmtime fiber stack.
pub(crate) async fn with_invocation_stack<F: std::future::Future>(future: F) -> F::Output {
    let mut future = Box::pin(future);
    std::future::poll_fn(|cx| {
        stacker::maybe_grow(INVOCATION_STACK_SIZE, INVOCATION_STACK_SIZE, || {
            future.as_mut().poll(cx)
        })
    })
    .await
}

/// Admits a pending invocation before executing it. `None` means admission declined to start.
pub async fn invoke_live_observed_and_traced<Ctx: WorkerCtx>(
    mut lowered: LoweredInvocation,
    store: &mut impl AsContextMut<Data = Ctx>,
    instance: &wasmtime::component::Instance,
    invocation: crate::worker::HydratedInvocation,
    stream_session: Option<&InvocationId>,
    recorder: Option<&mut dyn ReturnRecorder<Ctx>>,
) -> Result<Option<InvokeResult>, WorkerExecutorError> {
    let mut store = store.as_context_mut();
    let was_live_before = store.data().is_live();
    let worker = store.data().get_public_state().worker();
    let original = invocation.invocation.clone();
    let needs_binding_restore = if matches!(original, AgentInvocation::AgentMethod { .. }) {
        worker
            .prepared_stream_session(original.idempotency_key().expect("method invocation key"))
            .await?
            .is_none()
    } else {
        false
    };
    let started = async {
        store
            .data_mut()
            .on_agent_invocation_started(invocation, lowered.input())
            .await
    }
    .instrument(span!(Level::INFO, "on_agent_invocation_started"))
    .await;
    match started {
        Ok(true) => {
            if needs_binding_restore
                && worker
                    .prepared_stream_session(
                        original.idempotency_key().expect("method invocation key"),
                    )
                    .await?
                    .is_some()
            {
                let execution_input = worker
                    .rehydrate_durable_streaming_input(&original)
                    .await
                    .map_err(|error| {
                        WorkerExecutorError::runtime(format!(
                            "committed input reconstruction failed: {error}"
                        ))
                    })?;
                lowered = lower_invocation(
                    original,
                    &store.data().component_metadata().metadata,
                    &worker.owner_context,
                    execution_input,
                )
                .map_err(|error| {
                    WorkerExecutorError::runtime(format!(
                        "committed invocation lowering failed: {error}"
                    ))
                })?;
            }
            invoke_observed_and_traced(lowered, &mut store, instance, stream_session, recorder)
                .await
                .map(Some)
        }
        Ok(false) => {
            store.data_mut().on_agent_invocation_finished().await;
            Ok(None)
        }
        Err(error) => {
            store.data_mut().on_agent_invocation_finished().await;
            record_invocation(was_live_before, "failed");
            Err(error)
        }
    }
}

/// Invokes a function on a worker.
///
/// The context is held until the invocation finishes
/// The caller owns suspension after output streams and invocation bookkeeping settle.
///
/// Arguments:
/// - `lowered`: the lowered invocation describing what to invoke
/// - `store`: reference to the wasmtime instance's store
/// - `instance`: reference to the wasmtime instance
/// - `stream_session`: the session to materialize before ending the invocation scope
/// - `recorder`: records the result of an agent method that uses no streams, or of the
///   constructor, at the export's return, before the background work of the invocation drains
///
/// Pending live invocations must first pass admission through
/// [`invoke_live_observed_and_traced`]. Replay and snapshot calls need no pending admission.
pub async fn invoke_observed_and_traced<Ctx: WorkerCtx>(
    lowered: LoweredInvocation,
    store: &mut impl AsContextMut<Data = Ctx>,
    instance: &wasmtime::component::Instance,
    stream_session: Option<&InvocationId>,
    recorder: Option<&mut dyn ReturnRecorder<Ctx>>,
) -> Result<InvokeResult, WorkerExecutorError> {
    let mut store = store.as_context_mut();
    let was_live_before = store.data().is_live();

    let result = invoke_observed(lowered, &mut store, instance, stream_session, recorder).await;

    match &result {
        Err(_) => {
            record_invocation(was_live_before, "failed");
            result
        }
        Ok(InvokeResult::Exited { .. }) => {
            record_invocation(was_live_before, "exited");
            result
        }
        Ok(InvokeResult::Interrupted {
            interrupt_kind: InterruptKind::Interrupt(_),
            ..
        }) => {
            record_invocation(was_live_before, "interrupted");
            result
        }
        Ok(InvokeResult::Interrupted {
            interrupt_kind: InterruptKind::Suspend(_),
            ..
        }) => {
            record_invocation(was_live_before, "suspended");
            result
        }
        Ok(InvokeResult::Interrupted { .. }) => {
            record_invocation(was_live_before, "restarted");
            result
        }
        Ok(InvokeResult::Failed { .. } | InvokeResult::MethodFailed { .. }) => {
            record_invocation(was_live_before, "failed");
            result
        }
        Ok(InvokeResult::Succeeded { .. }) => {
            // this invocation finished and produced a result
            record_invocation(was_live_before, "success");
            result
        }
    }
}

/// Invokes a worker and calls the appropriate hooks to observe the invocation
async fn invoke_observed<Ctx: WorkerCtx>(
    lowered: LoweredInvocation,
    store: &mut impl AsContextMut<Data = Ctx>,
    instance: &wasmtime::component::Instance,
    stream_session: Option<&InvocationId>,
    recorder: Option<&mut dyn ReturnRecorder<Ctx>>,
) -> Result<InvokeResult, WorkerExecutorError> {
    let mut store = store.as_context_mut();

    let records_at_return = recorder.is_some() && lowered.records_result_at_return();
    let LoweredInvocation {
        display_name,
        read_only_method,
        call,
    } = lowered;
    let operator_authorized_oplog_processor =
        matches!(&call, LoweredCall::ProcessOplogEntries { .. });

    // The invocation start records the authority that admits any secret handles
    // carried by the input. Materialize those handles only after that admission.
    let call = match materialize_call(&mut store, call) {
        Ok(call) => call,
        Err(error) => {
            store.data_mut().on_agent_invocation_finished().await;
            return Err(error);
        }
    };
    let initializes = matches!(&call, PreparedCall::Initialize { .. });
    let defers_producer_settlement = matches!(
        &call,
        PreparedCall::Invoke { expected_output, .. } if expected_output.uses_streams()
    );

    let mut primary_body = match store
        .data()
        .durable_ctx()
        .enter_primary_invocation_body()
        .await
    {
        Ok(primary_body) => primary_body,
        Err(error) => {
            store.data_mut().on_agent_invocation_finished().await;
            return Err(error);
        }
    };

    let manages_owner_execution_status = primary_body.is_some();
    if manages_owner_execution_status {
        store.data_mut().set_running();
    }

    // Arm the optional per-invocation wall-clock deadline (`limits.max_invocation_duration`).
    // When it fires, a synthetic interrupt wakes every cooperative host park point and traps
    // executing wasm via the epoch callback; the resulting `InvokeResult::Interrupted` is
    // converted below into a typed timeout failure, so the timeout follows the normal
    // `TrapType::Error` retry handling instead of leaving the worker externally `Interrupted`.
    // An invocation that records its result at the export's return disarms it there.
    let mut deadline = Some(store.data().durable_ctx().arm_invocation_deadline());

    // If the invocation targets a read-only AgentMethod, enable the read-only invocation
    // strictness for the duration of the call. We restore the mode on every exit path:
    // normal `Ok` / `Err` returns from the wasmtime call site as well as panics that
    // unwind through the call. This is the only place where strictness is enabled.
    if let Some(method_name) = &read_only_method {
        store.data_mut().enter_read_only_mode(method_name.clone());
    }
    let operator_authorization = operator_authorized_oplog_processor.then(|| {
        store
            .data_mut()
            .durable_ctx_mut()
            .enter_operator_authorized_oplog_processor_invocation()
    });

    let invocation_caller = call.caller();
    let durable_ctx = store.data_mut().durable_ctx_mut();
    durable_ctx.set_invocation_caller(Some(invocation_caller));
    let invocation_identity = durable_ctx.set_invocation_id(call.has_invocation_id());
    let call_future = async {
        invocation_identity?;
        let mut at_return_usage = None;
        let mut result = dispatch_call(
            &mut store,
            instance,
            call,
            &display_name,
            records_at_return.then_some(&mut at_return_usage),
        )
        .await;
        if let Some(recorder) = recorder.filter(|_| records_at_return) {
            // The export returned, unless the constructor returned an error, whose background
            // work drained before the result.
            let returned = matches!(&result, Ok(InvokeResult::Succeeded { .. }))
                || (!initializes && matches!(&result, Ok(InvokeResult::MethodFailed { .. })));
            if returned {
                stash_replayed_invocation_terminal(&mut store, &mut result).await;
                // Live, and at the replay tail, the tool children that run settle before the
                // result is recorded, so that their failures still fail the invocation. Replay
                // that read the recorded terminal consumed their entries already: they lie
                // before it.
                if !store
                    .data()
                    .durable_ctx()
                    .has_replayed_invocation_terminal()
                    && let Some(parent) = primary_body
                        .as_ref()
                        .and_then(|body| body.invocation().cloned())
                {
                    settle_tool_children_before_result(&mut store, parent, &mut result).await;
                }
            }
            record_at_return_and_drain(
                &mut store,
                &display_name,
                result,
                recorder,
                &mut deadline,
                &mut primary_body,
            )
            .await
        } else if let Some(idempotency_key) = stream_session {
            // Returning a stream handle does not end its producer's invocation. Keep its
            // authority and lane registration until production and tail work settle.
            materialize_streaming_result(&mut store, result, &display_name, idempotency_key).await
        } else {
            result
        }
    };

    let call_outcome = std::panic::AssertUnwindSafe(call_future)
        .catch_unwind()
        .await;
    let durable_ctx = store.data_mut().durable_ctx_mut();
    durable_ctx.set_invocation_caller(None);
    durable_ctx.set_invocation_id(false)?;

    if read_only_method.is_some() {
        store.data_mut().exit_read_only_mode();
    }
    drop(operator_authorization);

    let mut call_result = match call_outcome {
        Ok(result) => result,
        Err(payload) => std::panic::resume_unwind(payload),
    };

    if matches!(
        &call_result,
        Ok(InvokeResult::Interrupted {
            interrupt_kind: InterruptKind::Jump,
            ..
        })
    ) {
        let ctx = store.data().durable_ctx();
        ctx.begin_stream_runtime_teardown();
        crate::durable_host::tool::fence_tool_operations_for_jump(&mut store).await?;
    }

    // A result recorded at the export's return settled its producers at the end of its drain.
    if !defers_producer_settlement
        && !store.data().durable_ctx().result_recorded_at_return()
        && matches!(&call_result, Ok(InvokeResult::Succeeded { .. }))
        && store.data().check_interrupt().is_none()
    {
        store
            .data_mut()
            .durable_ctx_mut()
            .settle_schema_producers(Ok(()));
    }

    let mut settled_schema_trap = false;
    if let Ok(InvokeResult::Failed {
        error: error @ (OplogAgentError::DeterministicTrap(_) | OplogAgentError::StackOverflow),
        timed_out: false,
        semantic_trap_retry_override: None,
        ..
    }) = &call_result
        && store
            .data()
            .durable_ctx()
            .selected_tool_owner_failure()
            .is_none()
        && store.data().check_interrupt().is_none()
    {
        store
            .data_mut()
            .durable_ctx_mut()
            .settle_schema_producers(Err(error.to_string("")));
        settled_schema_trap = true;
    }

    let output_watch = tokio_util::sync::CancellationToken::new();
    let output_watch_guard = output_watch.clone().drop_guard();
    let watch_schema_outputs = defers_producer_settlement
        && !matches!(
            &call_result,
            Ok(InvokeResult::Interrupted {
                interrupt_kind: InterruptKind::Jump,
                ..
            })
        );
    if watch_schema_outputs {
        let outputs = store.data().durable_ctx().schema_output_drain_monitor()?;
        let runtime = store.data().durable_ctx().runtime_suspension.clone();
        crate::durable_host::stream_outcome::supervise_schema_outputs(
            &mut store,
            outputs,
            runtime,
            output_watch,
        )
        .map_err(anyhow::Error::from)?;
    }

    if let Some(body) = primary_body.take_if(|body| body.invocation().is_some()) {
        end_tool_parent(&mut store, body, &mut call_result).await;
    }

    drop(output_watch_guard);
    if watch_schema_outputs
        && let Some(error) = store
            .data()
            .durable_ctx()
            .schema_output_drain_monitor()?
            .wait_for_failure()
            .now_or_never()
    {
        apply_selected_owner_failure(&mut call_result, OwnerFailureWinner::Infrastructure(error));
    }

    if defers_producer_settlement
        && let Ok(InvokeResult::Succeeded { consumed_fuel, .. }) = &call_result
    {
        let consumed_fuel = *consumed_fuel;
        match run_guest_call_settled(&mut store, async |accessor| {
            crate::durable_host::stream_outcome::drain_schema_outputs(accessor).await
        })
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(error @ WorkerExecutorError::Interrupted { .. })) => {
                call_result =
                    invoke_result_from_trap(&mut store, consumed_fuel, wasmtime::Error::new(error))
                        .await;
            }
            Ok(Err(error)) | Err(GuestCallSettlementError::Infrastructure(error)) => {
                call_result = Err(error);
            }
            Err(
                GuestCallSettlementError::Trap(error)
                | GuestCallSettlementError::Interrupted(error),
            ) => {
                call_result = invoke_result_from_trap(&mut store, consumed_fuel, error).await;
            }
        }
    }

    if settled_schema_trap
        && matches!(
            &call_result,
            Ok(InvokeResult::Failed {
                error: OplogAgentError::DeterministicTrap(_) | OplogAgentError::StackOverflow,
                timed_out: false,
                semantic_trap_retry_override: None,
                ..
            })
        )
        && store.data().check_interrupt().is_none()
        && store
            .data()
            .durable_ctx()
            .selected_tool_owner_failure()
            .is_none()
    {
        // Settled schema lanes and trapped projections no longer require Store progress.
        // Join before replay failure bookkeeping can finish their durable session.
        let outputs = store.data().durable_ctx().schema_output_drain_monitor()?;
        let interrupt = store.data().durable_ctx().create_interrupt_signal();
        if let Err(error) = outputs.drain_interruptibly(interrupt).await {
            call_result = Err(error);
        }
    }

    store.data_mut().on_agent_invocation_finished().await;

    apply_invocation_deadline(&mut store, deadline, call_result).await
}

/// Ends the primary body's tool parent: releases its deferred tool admissions, waits for its tool
/// children to settle, and turns `result` into the failure of a tool child or of the selected
/// owner failure.
async fn end_tool_parent<Ctx: WorkerCtx>(
    store: &mut StoreContextMut<'_, Ctx>,
    primary_body: crate::durable_host::PrimaryInvocationBody,
    result: &mut Result<InvokeResult, WorkerExecutorError>,
) {
    let parent = primary_body
        .invocation()
        .cloned()
        .expect("only a primary body with an invocation ends a tool parent");
    for error in crate::durable_host::tool::prepare_tool_parent_end(store, parent.clone()).await {
        let consumed_fuel = result
            .as_ref()
            .map(InvokeResult::consumed_fuel)
            .unwrap_or_default();
        let failure =
            invoke_result_from_trap(store, consumed_fuel, wasmtime::Error::from_anyhow(error))
                .await;
        retain_invocation_failure(result, failure);
    }
    primary_body.complete();
    for error in crate::durable_host::tool::settle_tool_children(store, parent).await {
        let consumed_fuel = result
            .as_ref()
            .map(InvokeResult::consumed_fuel)
            .unwrap_or_default();
        let failure =
            invoke_result_from_trap(store, consumed_fuel, wasmtime::Error::from_anyhow(error))
                .await;
        retain_invocation_failure(result, failure);
    }
    if let Some(owner_failure) = store.data().durable_ctx().selected_tool_owner_failure() {
        apply_selected_owner_failure(result, owner_failure);
    }
}

/// Waits at the export's return, with the tool parent open, for the tool children of `parent`
/// that run or need no admission, and turns `result` into the failure of a tool child or of the
/// selected owner failure. A capable child that waits for its parent's end stays staged: the end
/// of the parent, after the background work drained, admits it.
async fn settle_tool_children_before_result<Ctx: WorkerCtx>(
    store: &mut StoreContextMut<'_, Ctx>,
    parent: crate::worker::owner_lane::OwnerInvocationId,
    result: &mut Result<InvokeResult, WorkerExecutorError>,
) {
    for error in crate::durable_host::tool::settle_admitted_tool_children(store, parent).await {
        let consumed_fuel = result
            .as_ref()
            .map(InvokeResult::consumed_fuel)
            .unwrap_or_default();
        let failure =
            invoke_result_from_trap(store, consumed_fuel, wasmtime::Error::from_anyhow(error))
                .await;
        retain_invocation_failure(result, failure);
    }
    if let Some(owner_failure) = store.data().durable_ctx().selected_tool_owner_failure() {
        apply_selected_owner_failure(result, owner_failure);
    }
}

fn apply_selected_owner_failure(
    result: &mut Result<InvokeResult, WorkerExecutorError>,
    owner_failure: OwnerFailureWinner,
) {
    if matches!(
        result,
        Ok(InvokeResult::Interrupted {
            interrupt_kind: InterruptKind::ShardLost,
            ..
        }) | Err(WorkerExecutorError::Interrupted {
            kind: InterruptKind::ShardLost
        } | WorkerExecutorError::OplogFenced { .. })
    ) {
        return;
    }
    let consumed_fuel = result
        .as_ref()
        .map(InvokeResult::consumed_fuel)
        .unwrap_or_default();
    *result = match owner_failure {
        OwnerFailureWinner::Trap(trap) => Ok(InvokeResult::from_trap_type(consumed_fuel, trap)),
        OwnerFailureWinner::Lifecycle(interrupt_kind) => Ok(InvokeResult::Interrupted {
            consumed_fuel,
            interrupt_kind,
        }),
        OwnerFailureWinner::Infrastructure(error) => Err(error),
    };
}

fn retain_invocation_failure(
    result: &mut Result<InvokeResult, WorkerExecutorError>,
    failure: Result<InvokeResult, WorkerExecutorError>,
) {
    let shard_lost = matches!(
        result,
        Ok(InvokeResult::Interrupted {
            interrupt_kind: InterruptKind::ShardLost,
            ..
        }) | Err(WorkerExecutorError::Interrupted {
            kind: InterruptKind::ShardLost
        } | WorkerExecutorError::OplogFenced { .. })
    );
    // Lifecycle interruption must stop the generation even after a body failure;
    // retaining that failure must not keep a stopped or fenced generation running.
    if !shard_lost
        && (matches!(
            result,
            Ok(InvokeResult::Succeeded { .. }
                | InvokeResult::Exited { .. }
                | InvokeResult::MethodFailed { .. })
        ) || matches!(
            &failure,
            Ok(InvokeResult::Interrupted {
                interrupt_kind: InterruptKind::ShardLost,
                ..
            }) | Err(WorkerExecutorError::Interrupted {
                kind: InterruptKind::ShardLost
            } | WorkerExecutorError::OplogFenced { .. })
        ) || (matches!(&failure, Ok(InvokeResult::Interrupted { .. }))
            && !matches!(result, Ok(InvokeResult::Interrupted { .. }))))
    {
        *result = failure;
    }
}

/// Publishes the early stream-bearing result and drives its producers to completion.
/// Live execution and replay use the same trap classification as the guest export.
async fn materialize_streaming_result<Ctx: WorkerCtx>(
    store: &mut impl AsContextMut<Data = Ctx>,
    result: Result<InvokeResult, WorkerExecutorError>,
    display_name: &str,
    idempotency_key: &InvocationId,
) -> Result<InvokeResult, WorkerExecutorError> {
    let Ok(InvokeResult::Succeeded {
        result: invocation_result,
        consumed_fuel,
    }) = result
    else {
        return result;
    };
    let AgentInvocationResult::AgentMethod { output } = *invocation_result else {
        return Ok(InvokeResult::Succeeded {
            result: invocation_result,
            consumed_fuel,
        });
    };
    let mut store = store.as_context_mut();
    if let Some(interrupt_kind) = store.data().check_interrupt() {
        return Ok(InvokeResult::Interrupted {
            consumed_fuel,
            interrupt_kind,
        });
    }
    let _usage = store.data().durable_ctx().begin_guest_resource_usage();
    store.data().durable_ctx().activate_live_resource_usage();
    let component = store.data().component_metadata();
    let agent_id = store.data().parsed_agent_id();
    let agent_type = agent_id
        .as_ref()
        .and_then(|id| {
            component
                .metadata
                .find_agent_type_by_name_ref(&id.agent_type)
        })
        .ok_or_else(|| {
            WorkerExecutorError::runtime("durable invocation result schema is unavailable")
        })?;
    let method = agent_type
        .methods
        .iter()
        .find(|method| method.name == display_name)
        .ok_or_else(|| {
            WorkerExecutorError::runtime("durable invocation result method schema is unavailable")
        })?;
    let graph = agent_type.schema.clone();
    let root = method
        .output_schema
        .schema()
        .cloned()
        .unwrap_or_else(|| SchemaType::tuple(Vec::new()));
    let component_revision = component.revision;
    let worker = store.data().get_public_state().worker();
    let interrupt = store.data().durable_ctx().create_interrupt_signal();
    let runtime = store.data().durable_ctx().runtime_suspension.clone();
    let entity_invocation = store
        .data()
        .durable_ctx()
        .entity_invocation_scope()
        .map(|scope| scope.invocation_id().clone());
    let policy = store.data().durable_ctx().suspension_policy();
    let runtime_for_call = runtime.clone();
    let preparation = runtime.as_ref().map(|runtime| runtime.external_activity());
    let output_tasks = store.data().durable_ctx().schema_output_spawner()?;
    let output_monitor = store.data().durable_ctx().schema_output_drain_monitor()?;
    let output_watch = tokio_util::sync::CancellationToken::new();
    let output_watch_guard = output_watch.clone().drop_guard();
    let attribution = store.data().durable_ctx().attribution.clone();
    let invocation_key = store.data().durable_ctx().invocation_key();
    let future = store
        .as_context_mut()
        .run_concurrent(async move |accessor| {
            if let (Some(runtime), Some(activity)) =
                (&runtime_for_call, accessor.runtime_activity())
            {
                runtime.certify_root(activity);
            }
            let materialize = async move {
                if let Err(error) = accessor.with(|mut access| {
                    crate::durable_host::stream_outcome::supervise_schema_outputs(
                        &mut access,
                        output_monitor,
                        runtime_for_call.clone(),
                        output_watch,
                    )
                }) {
                    return Ok(Err(WorkerExecutorError::runtime(error.to_string())));
                }
                tokio::select! {
                    biased;
                    result = worker.materialize_durable_streaming_result(
                        idempotency_key,
                        output,
                        &graph,
                        &root,
                        component_revision,
                        runtime_for_call.as_ref().map(|runtime| runtime.source()),
                        preparation,
                        &output_tasks,
                    ) => Ok(result),
                    interrupt_kind = interrupt => Err(interrupt_kind),
                }
            };
            attribution.scoped(Some(invocation_key), materialize).await
        });
    let materialized = match runtime {
        Some(runtime) => {
            runtime
                .drive(future, entity_invocation.as_ref(), policy, false)
                .await
        }
        None => future.await,
    };
    drop(output_watch_guard);
    if let Some(error) = store
        .data()
        .durable_ctx()
        .schema_output_drain_monitor()?
        .wait_for_failure()
        .now_or_never()
    {
        return Err(error);
    }
    let interrupted = store.data().durable_ctx().is_interrupting();
    let output = match classify_guest_call_settlement(
        store.data().agent_id(),
        store.data().durable_ctx().last_replayed_index(),
        materialized,
        None,
        interrupted,
    ) {
        Ok(Ok(result)) => result?,
        Ok(Err(interrupt_kind)) => {
            return Ok(InvokeResult::Interrupted {
                consumed_fuel,
                interrupt_kind,
            });
        }
        Err(GuestCallSettlementError::Infrastructure(error)) => return Err(error),
        Err(
            GuestCallSettlementError::Trap(error) | GuestCallSettlementError::Interrupted(error),
        ) => {
            return invoke_result_from_trap(&mut store, consumed_fuel, error).await;
        }
    };
    let output_monitor = store.data().durable_ctx().schema_output_drain_monitor()?;
    let output_runtime = store.data().durable_ctx().runtime_suspension.clone();
    let output_watch = tokio_util::sync::CancellationToken::new();
    let output_watch_guard = output_watch.clone().drop_guard();
    let settled = run_stream_producers_settled(&mut store, async move |accessor| {
        accessor.with(|mut access| {
            crate::durable_host::stream_outcome::supervise_schema_outputs(
                &mut access,
                output_monitor,
                output_runtime,
                output_watch,
            )
        })
    })
    .await;
    drop(output_watch_guard);
    if let Some(error) = store
        .data()
        .durable_ctx()
        .schema_output_drain_monitor()?
        .wait_for_failure()
        .now_or_never()
    {
        return Err(error);
    }
    match settled {
        Ok(Ok(())) => {}
        Ok(Err(error))
        | Err(GuestCallSettlementError::Trap(error))
        | Err(GuestCallSettlementError::Interrupted(error)) => {
            return invoke_result_from_trap(&mut store, consumed_fuel, error).await;
        }
        Err(GuestCallSettlementError::Infrastructure(error)) => return Err(error),
    }
    store
        .data_mut()
        .durable_ctx_mut()
        .settle_schema_producers(Ok(()));
    Ok(InvokeResult::Succeeded {
        result: Box::new(AgentInvocationResult::AgentMethod { output }),
        consumed_fuel,
    })
}

/// Converts the synthetic interrupt raised by an exceeded invocation deadline into a typed
/// timeout failure at the invocation boundary.
///
/// The deadline (see [`crate::durable_host::InvocationDeadline`]) wakes cooperative host park
/// points and the epoch callback through the same signal a real interrupt uses, so the guest
/// call unwinds as `InvokeResult::Interrupted`. Here — and only here — that synthetic unwind is
/// replaced with an `InvokeResult::Failed` carrying a timeout error, which flows through the
/// regular `TrapType::Error` retry handling (no `AgentInvocationFinished` is written and the
/// invocation is retried per policy — the same contract as a crash).
///
/// First cause wins: a genuine external interrupt sets `ExecutionStatus::Interrupting`, which
/// persists until the invocation settles, so if one arrived the result is left as a real
/// interrupt even when the deadline also fired.
async fn apply_invocation_deadline<Ctx: WorkerCtx>(
    store: &mut StoreContextMut<'_, Ctx>,
    deadline: Option<crate::durable_host::InvocationDeadline>,
    call_result: Result<InvokeResult, WorkerExecutorError>,
) -> Result<InvokeResult, WorkerExecutorError> {
    let Some(deadline) = deadline else {
        return call_result;
    };
    if !deadline.exceeded() || store.data().durable_ctx().is_interrupting() {
        return call_result;
    }
    match call_result {
        Ok(InvokeResult::Interrupted {
            consumed_fuel,
            interrupt_kind: InterruptKind::Interrupt(_),
        }) => Ok(timed_out_failure(store, consumed_fuel, deadline.duration()).await),
        other => other,
    }
}

/// The failure of an invocation whose `limits.max_invocation_duration` deadline fired.
async fn timed_out_failure<Ctx: WorkerCtx>(
    store: &mut StoreContextMut<'_, Ctx>,
    consumed_fuel: u64,
    duration: Option<std::time::Duration>,
) -> InvokeResult {
    let retry_from = store.data().get_current_retry_point().await;
    InvokeResult::Failed {
        consumed_fuel,
        timed_out: true,
        error: OplogAgentError::InternalError(format!(
            "invocation exceeded the configured maximum invocation duration of {:?}",
            duration.expect("an exceeded deadline always has a configured duration")
        )),
        retry_from,
        semantic_trap_retry_override: None,
        provenance: FailureProvenance::Host,
    }
}

/// Pure settlement predicate for [`run_guest_call_settled`]: an invocation's tail work is
/// settled when no tracked spawned Store task is active (every one has finished or parked at a
/// safe park point).
fn tail_work_settled(active_spawned_tasks: usize) -> bool {
    active_spawned_tasks == 0
}

#[derive(Debug)]
pub(crate) enum GuestCallSettlementError {
    Interrupted(wasmtime::Error),
    Trap(wasmtime::Error),
    Infrastructure(WorkerExecutorError),
}

/// Permanent guest instruction failures and uncaught wasm exceptions that a tool boundary can
/// record as its own terminal: the failures whose provenance is
/// [`FailureProvenance::TargetInstruction`]. Agent and tool classification share the predicate.
pub(crate) fn permanent_tool_guest_trap(error: &wasmtime::Error) -> Option<String> {
    (failure_provenance(error) == FailureProvenance::TargetInstruction).then(|| {
        error
            .chain()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(": ")
    })
}

/// An error whose causes [`failure_provenance`] reads. Both `anyhow::Error` and
/// `wasmtime::Error` keep typed context outside the source chain, so a cause is either the typed
/// context or an item of the chain.
pub(crate) trait FailureCauses {
    fn causes<T: std::error::Error + Send + Sync + 'static>(&self) -> impl Iterator<Item = &T>;
    fn root(&self) -> &(dyn std::error::Error + 'static);
}

impl FailureCauses for anyhow::Error {
    fn causes<T: std::error::Error + Send + Sync + 'static>(&self) -> impl Iterator<Item = &T> {
        self.downcast_ref::<T>()
            .into_iter()
            .chain(self.chain().filter_map(|cause| cause.downcast_ref::<T>()))
    }

    fn root(&self) -> &(dyn std::error::Error + 'static) {
        self.root_cause()
    }
}

impl FailureCauses for wasmtime::Error {
    fn causes<T: std::error::Error + Send + Sync + 'static>(&self) -> impl Iterator<Item = &T> {
        self.downcast_ref::<T>()
            .into_iter()
            .chain(self.chain().filter_map(|cause| cause.downcast_ref::<T>()))
    }

    fn root(&self) -> &(dyn std::error::Error + 'static) {
        self.root_cause()
    }
}

/// The provenance of the failure `error`. An interrupt or an exit is `Lifecycle`. Any host
/// marker, executor error, Golem-specific trap or classified host error is `Host`; host-call
/// markers take precedence over an underlying Wasmtime trap. Only then is a deterministic guest
/// trap at the root, such as a Wasm instruction trap or an uncaught Wasm exception,
/// `TargetInstruction`; a Wasm backtrace alone does not establish guest origin, and every other
/// failure is `Host`.
pub(crate) fn failure_provenance(error: &impl FailureCauses) -> FailureProvenance {
    fn has<T: std::error::Error + Send + Sync + 'static>(error: &impl FailureCauses) -> bool {
        error.causes::<T>().next().is_some()
    }

    if has::<InterruptKind>(error)
        || has::<wasmtime_wasi::I32Exit>(error)
        || error
            .causes::<WorkerExecutorError>()
            .any(|error| matches!(error, WorkerExecutorError::Interrupted { .. }))
    {
        return FailureProvenance::Lifecycle;
    }
    if has::<GolemSpecificWasmTrap>(error)
        || has::<ClassifiedHostError>(error)
        || has::<SemanticTrapRetryOverrideMarker>(error)
        || has::<DurableCallTrapContextMarker>(error)
        || has::<DurableRecoveryFailure>(error)
        || has::<UnreadableOplog>(error)
        || has::<WorkerExecutorError>(error)
    {
        return FailureProvenance::Host;
    }
    if crate::model::deterministic_guest_trap(error.root()) {
        FailureProvenance::TargetInstruction
    } else {
        FailureProvenance::Host
    }
}

fn is_guest_semantic_trap(error: &wasmtime::Error) -> bool {
    let chain_contains = |predicate: &dyn Fn(&(dyn std::error::Error + 'static)) -> bool| {
        error.chain().any(predicate)
    };

    if let Some(trap) = error.root_cause().downcast_ref::<wasmtime::Trap>() {
        return !matches!(trap, wasmtime::Trap::AsyncDeadlock);
    }
    if error.root_cause().is::<wasmtime::GcHeapOutOfMemory<()>>() {
        return true;
    }
    if error.is::<ClassifiedHostError>()
        || chain_contains(&|cause| {
            cause.is::<GolemSpecificWasmTrap>()
                || cause.is::<ClassifiedHostError>()
                || cause.is::<SemanticTrapRetryOverrideMarker>()
                || cause.is::<DurableCallTrapContextMarker>()
                || cause.is::<wasmtime_wasi::I32Exit>()
                || matches!(
                    cause.downcast_ref::<WorkerExecutorError>(),
                    Some(
                        WorkerExecutorError::InvalidRequest { .. }
                            | WorkerExecutorError::UnexpectedOplogEntry { .. }
                            | WorkerExecutorError::ParamTypeMismatch { .. }
                            | WorkerExecutorError::ValueMismatch { .. }
                            | WorkerExecutorError::InvocationFailed { .. }
                            | WorkerExecutorError::ReadOnlyViolation { .. }
                            | WorkerExecutorError::PermissionDenied { .. }
                    )
                )
        })
    {
        return true;
    }
    if chain_contains(&|cause| cause.is::<WorkerExecutorError>()) {
        return false;
    }

    error.downcast_ref::<wasmtime::WasmBacktrace>().is_some()
}

/// The interruption that unwound a guest execution, when `error` is one. The epoch deadline
/// callback traps with a bare `InterruptKind`; a durable host call that observed the worker's
/// interrupt signal, or the fencing of its entity bodies, fails with
/// `WorkerExecutorError::Interrupted`. Both are the root cause of the trap that Wasmtime reports.
pub(crate) fn interrupt_kind_in(error: &wasmtime::Error) -> Option<InterruptKind> {
    let root_cause = error.root_cause();
    if let Some(kind) = root_cause.downcast_ref::<InterruptKind>() {
        return Some(*kind);
    }
    match root_cause.downcast_ref::<WorkerExecutorError>() {
        Some(WorkerExecutorError::Interrupted { kind }) => Some(*kind),
        _ => None,
    }
}

/// The oplog that a host call could not read, in the chain of `error`, with the agent it belongs
/// to: either already a `WorkerExecutorError`, or the read failure itself. The error is
/// `OplogFailed` for good, or `RecoveryRequired` when a later read can pass.
fn unreadable_oplog_in(error: &wasmtime::Error) -> Option<(AgentId, WorkerExecutorError)> {
    error
        .chain()
        .find_map(|cause| match cause.downcast_ref::<WorkerExecutorError>() {
            Some(error @ WorkerExecutorError::OplogFailed { agent_id, .. }) => {
                Some((*agent_id, error.clone()))
            }
            _ => cause
                .downcast_ref::<UnreadableOplog>()
                .map(|error| (error.agent_id, error.clone().into())),
        })
}

fn classify_guest_call_settlement<R>(
    own_agent_id: &AgentId,
    replay_index: OplogIndex,
    result: wasmtime::Result<R>,
    tail_timeout: Option<(std::time::Duration, usize)>,
    interrupted: bool,
) -> Result<R, GuestCallSettlementError> {
    if let Some((timeout, active)) = tail_timeout {
        return Err(GuestCallSettlementError::Infrastructure(
            WorkerExecutorError::runtime(format!(
                "invocation tail work did not settle within {timeout:?}: {active} spawned task(s) still active"
            )),
        ));
    }
    result.map_err(|error| {
        if interrupted || interrupt_kind_in(&error).is_some() {
            GuestCallSettlementError::Interrupted(error)
        } else if let Some((agent_id, unreadable)) = unreadable_oplog_in(&error) {
            // The oplog of this agent is a failure of the agent. The oplog of another agent that
            // failed for good fails only the call that read it, as a trap that the retry policy
            // of this agent handles; when a later read can pass, this agent recovers and retries.
            if agent_id == *own_agent_id
                || !matches!(unreadable, WorkerExecutorError::OplogFailed { .. })
            {
                GuestCallSettlementError::Infrastructure(unreadable)
            } else {
                GuestCallSettlementError::Trap(error)
            }
        } else if let Some(recovery_failure) = error
            .chain()
            .find_map(|cause| cause.downcast_ref::<DurableRecoveryFailure>())
        {
            let retry_from = error
                .chain()
                .find_map(|cause| cause.downcast_ref::<DurableCallTrapContextMarker>())
                .map(|marker| marker.payload.retry_from);
            let details = format!(
                "durable runtime reconstruction failed: {:#}",
                recovery_failure.inner
            );
            GuestCallSettlementError::Infrastructure(match recovery_failure.kind {
                DurableRecoveryFailureKind::Retryable => match retry_from {
                    Some(retry_from) => {
                        WorkerExecutorError::recovery_required_from(details, retry_from)
                    }
                    None => WorkerExecutorError::recovery_required(details),
                },
                DurableRecoveryFailureKind::Permanent => {
                    WorkerExecutorError::unexpected_oplog_entry(
                        retry_from.unwrap_or(replay_index),
                        "valid durable recovery data",
                        details,
                    )
                }
            })
        } else if let Some(error) = error.chain().find_map(|cause| {
            let error = cause.downcast_ref::<WorkerExecutorError>()?;
            (matches!(error, WorkerExecutorError::RecoveryRequired { .. })
                || crate::durable_host::is_tool_attachment_live_admission_rejection(error))
            .then(|| error.clone())
        }) {
            GuestCallSettlementError::Infrastructure(error)
        } else if is_guest_semantic_trap(&error) {
            GuestCallSettlementError::Trap(error)
        } else {
            GuestCallSettlementError::Infrastructure(WorkerExecutorError::runtime(format!(
                "guest call settlement task failed: {error}"
            )))
        }
    })
}

struct ToolInitiationRun<'a, 's, Ctx: WorkerCtx> {
    store: &'a mut StoreContextMut<'s, Ctx>,
    owner: std::sync::Arc<crate::durable_host::concurrent::initiation_owner::InitiationOwner<Ctx>>,
    interrupted: bool,
}

impl<Ctx: WorkerCtx> Drop for ToolInitiationRun<'_, '_, Ctx> {
    fn drop(&mut self) {
        if self.interrupted {
            self.store
                .data()
                .durable_ctx()
                .begin_stream_runtime_teardown();
            self.owner.close();
        }
    }
}

/// Returns at the host-observed export result without draining guest tasks.
pub(crate) async fn run_tool_guest_call<Ctx: WorkerCtx, R>(
    store: &mut StoreContextMut<'_, Ctx>,
    fun: impl AsyncFnOnce(&Accessor<Ctx>) -> wasmtime::Result<R>,
) -> Result<wasmtime::Result<R>, GuestCallSettlementError> {
    let initiations = crate::durable_host::concurrent::initiation_owner::InitiationOwner::new();
    store.data_mut().durable_ctx_mut().host_initiations = Some(initiations.clone());
    let mut run = ToolInitiationRun {
        store,
        owner: initiations.clone(),
        interrupted: true,
    };
    let store = &mut *run.store;
    let service = initiations.clone();
    let ctx = store.data().durable_ctx();
    let runtime = ctx.runtime_suspension.clone();
    let root_runtime = runtime.clone();
    let entity = ctx
        .entity_invocation_scope()
        .map(|scope| scope.invocation_id().clone());
    let policy = ctx.suspension_policy();
    let driver = store
        .as_context_mut()
        .run_concurrent(async move |accessor| {
            if let (Some(runtime), Some(activity)) = (&root_runtime, accessor.runtime_activity()) {
                runtime.certify_root(activity);
            }
            tokio::select! {
                result = fun(accessor) => result,
                result = service.serve(accessor, |ctx: &mut Ctx| ctx.durable_ctx_mut()) => {
                    result?;
                    Err(wasmtime::Error::msg("tool initiation service stopped before return"))
                }
            }
        });
    let result = match runtime {
        Some(runtime) => runtime.drive(driver, entity.as_ref(), policy, false).await,
        None => driver.await,
    };
    let ctx = store.data().durable_ctx();
    if matches!(&result, Ok(Ok(_))) {
        ctx.entity_body_finalization.begin_guest_return();
    }
    if matches!(&result, Ok(Ok(_))) {
        let started = std::sync::Arc::new(tokio::sync::Notify::new());
        let _deadline = ctx.arm_tail_work_deadline(started.clone());
        let interrupt = ctx.create_interrupt_signal();
        started.notify_one();
        let drained = tokio::select! {
            drained = initiations.drain(store.data_mut().durable_ctx_mut()) => drained,
            interruption = interrupt => Err(interruption.into()),
        };
        if let Err(error) = drained {
            let ctx = store.data().durable_ctx();
            if matches!(
                &error,
                WorkerExecutorError::Interrupted {
                    kind: InterruptKind::EntityCancellation
                }
            ) {
                ctx.tool_trap_disposal
                    .store(true, std::sync::atomic::Ordering::Release);
            }
            ctx.begin_stream_runtime_teardown();
            initiations.close();
            initiations.wait_closed().await;
            return Err(match error {
                WorkerExecutorError::Interrupted { kind } => {
                    GuestCallSettlementError::Interrupted(kind.into())
                }
                error => GuestCallSettlementError::Infrastructure(error),
            });
        }
        initiations.close();
    }
    let ctx = store.data().durable_ctx();
    let interrupted = ctx.is_interrupting() || ctx.invocation_deadline_exceeded();
    let result = classify_guest_call_settlement(
        store.data().agent_id(),
        store.data().durable_ctx().last_replayed_index(),
        result,
        None,
        interrupted,
    );
    // Normal errors retain the service until Store disposal selects trap versus teardown cleanup.
    run.interrupted = false;
    result
}

/// Runs a guest export call on the store's event loop, draining Golem-spawned tail work before
/// returning.
///
/// Plain `run_concurrent` returns as soon as the root future resolves, while store-spawned
/// durable tasks (HTTP body recorders, TCP stream drivers, stdio/filesystem consumers) may still
/// be active — their durable `Start`/`End` entries would then land after (or never before)
/// `AgentInvocationFinished`, breaking positional replay. This wrapper uses
/// `run_concurrent_and_settle`: after the root future completes, the event loop keeps running
/// until, at an idle observation point (all runnable host futures polled, no queued work, no
/// remaining guest tasks), no tracked task is active — every Golem-spawned store task has either
/// finished or parked at a designated safe park point (a wait on future guest action, which may
/// legitimately span invocations). Safe-parked tasks are left parked in the store.
///
/// Tracked tasks park only outside replay-cursor transactions. A task waiting for or holding the
/// cursor therefore remains active until it releases the transaction. The cursor itself is shared
/// by every owner Store, so its global lock state cannot be used as a Store-local settlement
/// condition: a sibling sidecar may legitimately hold it while this Store has settled.
///
/// The drain phase is bounded by `limits.tail_work_settle_timeout`. It normally settles as soon as
/// the tasks' pending durable appends and I/O complete; the bound only fires when such work is
/// stuck (e.g. an unresponsive peer without its own timeout). Hitting it cooperatively interrupts
/// the store tasks and fails the guest call like a trap after they unwind: no unfinished event-loop
/// future is dropped, no `AgentInvocationFinished` entry is written, and normal retry handling
/// replays any calls left incomplete — the same contract as a crash at this point.
///
/// The event loop future itself is never dropped while unfinished: durable `DurableCallSession`s owned
/// by parked host futures are not cancellation-safe (`NotCancellable` handles panic when dropped
/// unfinished, and even `Cancellable` drops may leave terminal oplog effects unwritten).
/// External cancellation — worker interruption and the optional max-invocation-duration limit —
/// is instead delivered *cooperatively*: every blocking host park point races the worker's
/// interrupt signal, abandons its durable call handles for the trap, and unwinds the event loop
/// with the interrupt from within.
pub(crate) async fn run_guest_call_settled<Ctx: WorkerCtx, R>(
    store: &mut StoreContextMut<'_, Ctx>,
    fun: impl AsyncFnOnce(&Accessor<Ctx>) -> R,
) -> Result<R, GuestCallSettlementError> {
    run_guest_call_settled_at(store, fun, SettlementDeadline::RootReturned).await
}

/// Streaming producers may continue in guest tasks after the method returns its mappings.
/// Their lifetime is invocation work, not the bounded cleanup of a completed producer.
pub(crate) async fn run_stream_producers_settled<Ctx: WorkerCtx, R>(
    store: &mut StoreContextMut<'_, Ctx>,
    fun: impl AsyncFnOnce(&Accessor<Ctx>) -> R,
) -> Result<R, GuestCallSettlementError> {
    run_guest_call_settled_at(store, fun, SettlementDeadline::GuestTasksExited).await
}

#[derive(Clone, Copy)]
pub(crate) enum SettlementDeadline {
    RootReturned,
    GuestTasksExited,
    /// The drain of an invocation whose result was recorded at the export's return. It is not
    /// bounded by `limits.tail_work_settle_timeout`.
    AfterResult,
}

pub(crate) async fn run_guest_call_settled_at<Ctx: WorkerCtx, R>(
    store: &mut StoreContextMut<'_, Ctx>,
    fun: impl AsyncFnOnce(&Accessor<Ctx>) -> R,
    deadline: SettlementDeadline,
) -> Result<R, GuestCallSettlementError> {
    let runtime = store.data().durable_ctx().runtime_suspension.clone();
    let entity_invocation = store
        .data()
        .durable_ctx()
        .entity_invocation_scope()
        .map(|scope| scope.invocation_id().clone());
    let policy = store.data().durable_ctx().suspension_policy();
    let tracker = store.data().durable_ctx().tail_work_tracker();
    let tracker_for_error = tracker.clone();
    let drain_started = std::sync::Arc::new(tokio::sync::Notify::new());
    let attribution = store.data().durable_ctx().attribution.clone();
    let invocation_key = store.data().durable_ctx().invocation_key();
    let fun = {
        let drain_started = drain_started.clone();
        let runtime = runtime.clone();
        let finalization = store.data().durable_ctx().entity_body_finalization.clone();
        async move |accessor: &Accessor<Ctx>| {
            if let (Some(runtime), Some(activity)) = (&runtime, accessor.runtime_activity()) {
                runtime.certify_root(activity);
            }
            let result = attribution
                .scoped(Some(invocation_key), fun(accessor))
                .await;
            finalization.begin();
            if matches!(deadline, SettlementDeadline::RootReturned) {
                drain_started.notify_one();
            }
            result
        }
    };
    let mut guest_tasks_exited = false;
    let cleanup_started = drain_started.clone();
    // Wasmtime consults this predicate only after all guest tasks have exited.
    let mut settled = move |_store: StoreContextMut<'_, Ctx>| {
        if !guest_tasks_exited && matches!(deadline, SettlementDeadline::GuestTasksExited) {
            guest_tasks_exited = true;
            cleanup_started.notify_one();
        }
        let active = tracker.active_count();
        tracing::debug!("invocation tail-work settlement check: {active} spawned task(s) active");
        tail_work_settled(active)
    };
    let tail_work_deadline = (!matches!(deadline, SettlementDeadline::AfterResult)).then(|| {
        store
            .data()
            .durable_ctx()
            .arm_tail_work_deadline(drain_started)
    });
    let future = store
        .as_context_mut()
        .run_concurrent_and_settle(fun, &mut settled);
    let result = match runtime {
        Some(runtime) => {
            runtime
                .drive(future, entity_invocation.as_ref(), policy, false)
                .await
        }
        None => future.await,
    };
    let interrupted = store.data().durable_ctx().is_interrupting()
        || store.data().durable_ctx().invocation_deadline_exceeded();
    let tail_timeout = tail_work_deadline
        .as_ref()
        .filter(|deadline| deadline.exceeded() && !interrupted)
        .map(|deadline| (deadline.duration(), tracker_for_error.active_count()));
    classify_guest_call_settlement(
        store.data().agent_id(),
        store.data().durable_ctx().last_replayed_index(),
        result,
        tail_timeout,
        interrupted,
    )
}

/// Records the result of an agent method with a non-streaming result, or of the constructor,
/// when its export returns, before the background work of the invocation drains.
///
/// The invocation loop and the replay driver supply it. It keeps the outcome of the recording,
/// which the caller reads after [`invoke_observed_and_traced`] returns.
pub trait ReturnRecorder<Ctx: WorkerCtx>: Send {
    fn uses_streams(&self) -> bool;

    /// Records `result`, a `Succeeded` or `MethodFailed` result, and gives it back. `true` means
    /// the result is recorded and the background work drains; `false` means it is not recorded,
    /// and the invocation returns `result` without a drain.
    fn record<'a>(
        &'a mut self,
        store: &'a mut StoreContextMut<'_, Ctx>,
        display_name: &'a str,
        result: InvokeResult,
    ) -> Pin<Box<dyn Future<Output = (InvokeResult, bool)> + Send + 'a>>;
}

/// The outcome of recording a result at the export's return.
pub(crate) enum ReturnRecorded {
    Succeeded(Result<(), WorkerExecutorError>),
    MethodFailed(Result<(), crate::worker::MethodFailureSettlementError>),
}

impl ReturnRecorded {
    /// Whether the result is recorded.
    pub(crate) fn is_recorded(&self) -> bool {
        match self {
            Self::Succeeded(recorded) => recorded.is_ok(),
            Self::MethodFailed(recorded) => recorded.is_ok(),
        }
    }
}

/// Records a `Succeeded` or `MethodFailed` result at the export's return, live or by consuming
/// the recorded terminal in replay, and gives `result` back. Any other result records nothing.
pub(crate) async fn record_result_at_return<Ctx: WorkerCtx>(
    store: &mut StoreContextMut<'_, Ctx>,
    display_name: &str,
    result: InvokeResult,
    uses_streams: bool,
) -> (InvokeResult, Option<ReturnRecorded>) {
    match result {
        InvokeResult::Succeeded {
            result,
            consumed_fuel,
        } => {
            let mut output = AgentInvocationOutput {
                result: *result,
                consumed_fuel: Some(consumed_fuel),
                invocation_status: None,
                component_revision: Some(store.data().component_metadata().revision),
                agent_id: None,
                idempotency_key: None,
                oplog_index: None,
                agent_fingerprint: None,
                background_failure: None,
            };
            let recorded = store
                .data_mut()
                .on_agent_invocation_success(
                    display_name,
                    consumed_fuel,
                    &mut output,
                    ResultRecordedAt::ExportReturn,
                )
                .await;
            (
                InvokeResult::Succeeded {
                    result: Box::new(output.result),
                    consumed_fuel,
                },
                Some(ReturnRecorded::Succeeded(recorded)),
            )
        }
        InvokeResult::MethodFailed { ref error, .. } => {
            let recorded = crate::durable_host::DurableWorkerCtx::<Ctx>::on_agent_method_failure(
                store,
                display_name,
                error.clone(),
                uses_streams,
                ResultRecordedAt::ExportReturn,
            )
            .await;
            (result, Some(ReturnRecorded::MethodFailed(recorded)))
        }
        other => (other, None),
    }
}

/// Runs a guest export call on the store's event loop until the root future resolves, leaving
/// the spawned guest and host work of the call in the Store. A poll that blocks while the root
/// is the only open activity of the Store traps with [`wasmtime::Trap::AsyncDeadlock`].
async fn run_guest_call_until_return<Ctx: WorkerCtx, R>(
    store: &mut StoreContextMut<'_, Ctx>,
    fun: impl AsyncFnOnce(&Accessor<Ctx>) -> R,
) -> Result<R, GuestCallSettlementError> {
    let runtime = store.data().durable_ctx().runtime_suspension.clone();
    let entity_invocation = store
        .data()
        .durable_ctx()
        .entity_invocation_scope()
        .map(|scope| scope.invocation_id().clone());
    let policy = store.data().durable_ctx().suspension_policy();
    let attribution = store.data().durable_ctx().attribution.clone();
    let invocation_key = store.data().durable_ctx().invocation_key();
    let runtime_for_call = runtime.clone();
    let future = store
        .as_context_mut()
        .run_concurrent(async move |accessor: &Accessor<Ctx>| {
            if let (Some(runtime), Some(activity)) =
                (&runtime_for_call, accessor.runtime_activity())
            {
                runtime.certify_root(activity);
            }
            attribution
                .scoped(Some(invocation_key), fun(accessor))
                .await
        });
    let result = match runtime {
        Some(runtime) => {
            runtime
                .drive(future, entity_invocation.as_ref(), policy, true)
                .await
        }
        None => future.await,
    };
    let interrupted = store.data().durable_ctx().is_interrupting()
        || store.data().durable_ctx().invocation_deadline_exceeded();
    classify_guest_call_settlement(
        store.data().agent_id(),
        store.data().durable_ctx().last_replayed_index(),
        result,
        None,
        interrupted,
    )
}

/// In replay, after the export returned, waits with the event loop running for the recorded
/// terminal of the invocation and stashes it for the result reader. The background work and the
/// tool children of the invocation keep replaying meanwhile and consume the entries recorded
/// before the terminal. At the replay tail, and in a live run, nothing is stashed. A failure of
/// the background work while it waits replaces `result`.
async fn stash_replayed_invocation_terminal<Ctx: WorkerCtx>(
    store: &mut StoreContextMut<'_, Ctx>,
    result: &mut Result<InvokeResult, WorkerExecutorError>,
) {
    let Some(reader) = store.data().durable_ctx().invocation_terminal_reader() else {
        return;
    };
    let read = run_guest_call_until_return(store, async |accessor| {
        // A replayed call that the guest dropped resolves through its claim or a
        // `ReplayCancelWaiter`, never through the dropped-call queue, so no drain has to record
        // an entry before the recorded terminal is read.
        if let Some(terminal) = reader.read().await.transpose() {
            accessor.with(|mut access| {
                access
                    .data_mut()
                    .durable_ctx_mut()
                    .stash_replayed_invocation_terminal(terminal)
            });
        }
    })
    .await;
    match read {
        Ok(()) => {}
        Err(GuestCallSettlementError::Infrastructure(error)) => *result = Err(error),
        Err(GuestCallSettlementError::Trap(error))
        | Err(GuestCallSettlementError::Interrupted(error)) => {
            let consumed_fuel = result
                .as_ref()
                .map(InvokeResult::consumed_fuel)
                .unwrap_or_default();
            *result = invoke_result_from_trap(store, consumed_fuel, error).await;
        }
    }
}

/// Records `result` at the export's return and drains the background work of the invocation.
///
/// A result other than `Succeeded` or `MethodFailed`, and a result the recorder does not record,
/// returns at once. The invocation deadline is disarmed before the recording; a deadline that
/// fired first turns the result into the timeout failure. A recorded result drains the background
/// work without the tail-work settlement timeout, and settles the invocation: the end of the
/// drain ends the tool parent of `primary_body`, which admits and settles the tool children made
/// after the return and the staged ones, then settles the schema producers of a success and
/// records `AgentInvocationSettled` with the fuel of the drain. A failed drain, or a failed tool
/// child at its end, returns its failure; the result stays recorded. `primary_body` stays with
/// the caller when the result is not recorded.
async fn record_at_return_and_drain<Ctx: WorkerCtx>(
    store: &mut StoreContextMut<'_, Ctx>,
    display_name: &str,
    result: Result<InvokeResult, WorkerExecutorError>,
    recorder: &mut dyn ReturnRecorder<Ctx>,
    deadline: &mut Option<crate::durable_host::InvocationDeadline>,
    primary_body: &mut Option<crate::durable_host::PrimaryInvocationBody>,
) -> Result<InvokeResult, WorkerExecutorError> {
    let result = match result {
        Ok(result @ (InvokeResult::Succeeded { .. } | InvokeResult::MethodFailed { .. })) => result,
        other => return unrecorded_at_return(store, other),
    };
    if let Some(armed) = deadline.take() {
        let duration = armed.duration();
        if !armed.disarm() {
            let failure = timed_out_failure(store, result.consumed_fuel(), duration).await;
            return unrecorded_at_return(store, Ok(failure));
        }
    }
    let (result, recorded) = recorder.record(store, display_name, result).await;
    if !recorded {
        return unrecorded_at_return(store, Ok(result));
    }
    store
        .data_mut()
        .durable_ctx_mut()
        .mark_result_recorded_at_return();

    rearm_fuel_check(store);
    let current_level = store.get_fuel().unwrap_or(0);
    let agent_mode = store.data().agent_mode();
    if let Err(error) = store.data_mut().ensure_fuel(current_level) {
        return Err(resource_capacity_error(agent_mode, error));
    }
    let drained =
        run_guest_call_settled_at(store, async |_accessor| (), SettlementDeadline::AfterResult)
            .await;
    let mut ended = Ok(result);
    if let Some(body) = primary_body.take_if(|body| body.invocation().is_some()) {
        end_tool_parent(store, body, &mut ended).await;
    }
    let settled_fuel = finish_invocation_and_get_fuel_consumption(store, display_name).await?;
    // A tool child that failed at the end of the drain fails the drain, like a trap of the
    // background work: its failure carries the fuel of the drain.
    if let Ok(
        InvokeResult::Failed { consumed_fuel, .. }
        | InvokeResult::Interrupted { consumed_fuel, .. },
    ) = &mut ended
    {
        *consumed_fuel = settled_fuel;
    }
    match drained {
        Ok(()) => {}
        Err(GuestCallSettlementError::Infrastructure(error)) => return Err(error),
        Err(GuestCallSettlementError::Trap(error))
        | Err(GuestCallSettlementError::Interrupted(error)) => {
            return invoke_result_from_trap(store, settled_fuel, error).await;
        }
    }
    let result = match ended {
        Ok(result @ (InvokeResult::Succeeded { .. } | InvokeResult::MethodFailed { .. })) => result,
        failure => return failure,
    };
    if matches!(result, InvokeResult::Succeeded { .. }) && store.data().check_interrupt().is_none()
    {
        store
            .data_mut()
            .durable_ctx_mut()
            .settle_schema_producers(Ok(()));
    }
    if recorder.uses_streams()
        && let Some(key) = store.data().get_current_idempotency_key().await
    {
        let worker = store.data().get_public_state().worker();
        let failure = match &result {
            InvokeResult::Succeeded { .. } => None,
            InvokeResult::MethodFailed { error, .. } => Some(error.message().to_owned()),
            _ => unreachable!("only recorded results reach the end of the drain"),
        };
        let attribution = store.data().durable_ctx().attribution.clone();
        let invocation_key = store.data().durable_ctx().invocation_key();
        // Completion can acquire locks held by Store-owned input cleanup.
        let completion = store
            .as_context_mut()
            .run_concurrent(async move |_accessor| {
                attribution
                    .scoped(Some(invocation_key), async {
                        match failure {
                            None => worker.complete_durable_streaming_session(&key).await,
                            Some(error) => worker.fail_durable_streaming_session(&key, error).await,
                        }
                    })
                    .await
            })
            .await
            .map_err(|error| WorkerExecutorError::from(anyhow::Error::from(error)))
            .and_then(|result| result);
        if let Err(error) = completion {
            let worker = store.data().get_public_state().worker();
            if worker.retire_if_shard_lost(&error) {
                return Err(WorkerExecutorError::Interrupted {
                    kind: InterruptKind::ShardLost,
                });
            }
            // Session persistence is infrastructure, not a failure of the recorded result or
            // its background work. Reconstruct without consuming the guest's retry policy.
            return Err(match error {
                error @ (WorkerExecutorError::RecoveryRequired { .. }
                | WorkerExecutorError::OplogFailed { .. }
                | WorkerExecutorError::Interrupted { .. }) => error,
                error => WorkerExecutorError::recovery_required(format!(
                    "failed to settle durable streaming session: {error}"
                )),
            });
        }
    }
    store
        .data_mut()
        .durable_ctx_mut()
        .on_agent_invocation_settled(settled_fuel)
        .await?;
    Ok(result)
}

/// Returns `result` of an invocation whose result is not recorded at the export's return. The
/// terminal that replay stashed for the result reader is discarded; when reading it failed, that
/// failure is returned instead.
fn unrecorded_at_return<Ctx: WorkerCtx>(
    store: &mut StoreContextMut<'_, Ctx>,
    result: Result<InvokeResult, WorkerExecutorError>,
) -> Result<InvokeResult, WorkerExecutorError> {
    match store
        .data_mut()
        .durable_ctx_mut()
        .take_replayed_invocation_terminal()
    {
        Some(Err(error)) => Err(error),
        Some(Ok(_)) | None => result,
    }
}

/// Dispatches a single lowered invocation to the matching typed guest export
/// accessor (`golem:agent/guest@2.0.0`, `golem:api/save-snapshot`,
/// `golem:api/load-snapshot`, or `golem:api/oplog-processor`) and maps its
/// typed result into an [`InvokeResult`].
///
/// With `at_return_usage`, the constructor and an agent method with a non-streaming result run
/// until the export returns and leave their background work in the Store; the caller records the
/// result and drains it ([`record_at_return_and_drain`]). They leave the resource usage guard of
/// the call in `at_return_usage`, so that it covers the drain. Every other call drains its
/// background work before it returns its result.
async fn dispatch_call<Ctx: WorkerCtx>(
    store: &mut StoreContextMut<'_, Ctx>,
    instance: &wasmtime::component::Instance,
    call: PreparedCall,
    display_name: &str,
    at_return_usage: Option<&mut Option<crate::services::linear_memory::GuestResourceUsageGuard>>,
) -> Result<InvokeResult, WorkerExecutorError> {
    let attribution = store.data().durable_ctx().attribution.clone();
    let invocation_key = store.data().durable_ctx().invocation_key();
    match call {
        PreparedCall::Initialize {
            agent_type, input, ..
        } if at_return_usage.is_some() => {
            let guest = load_agent_guest(store, instance)?;
            *at_return_usage.expect("the guard checked the usage slot") =
                prepare_metered_guest_call(store, display_name, false).await?;
            let returned = attribution.export_returned(invocation_key);
            let result = run_guest_call_until_return(store, async |accessor| {
                returned
                    .observe(attribution.armed(
                        invocation_key,
                        guest.call_initialize(accessor, agent_type, input),
                    ))
                    .await
            })
            .await;
            store
                .data_mut()
                .durable_ctx_mut()
                .apply_return_cut_off(invocation_key);
            // A constructor error is not recorded at the return, so its background work drains
            // before the failure is handled, as for every other unrecorded result.
            let result = match result {
                Ok(Ok(Err(wire_err))) => {
                    match run_guest_call_settled(store, async |_accessor| ()).await {
                        Ok(()) => Ok(Ok(Err(wire_err))),
                        Err(error) => Err(error),
                    }
                }
                other => other,
            };
            let consumed_fuel =
                finish_invocation_and_get_fuel_consumption(store, display_name).await?;
            match result {
                Ok(Ok(Ok(()))) => Ok(InvokeResult::Succeeded {
                    consumed_fuel,
                    result: Box::new(AgentInvocationResult::AgentInitialization),
                }),
                Ok(Ok(Err(wire_err))) => invoke_result_from_agent_error(
                    store,
                    golem_common::model::agent::AgentInvocationKind::AgentInitialization,
                    consumed_fuel,
                    wire_err,
                ),
                Ok(Err(err))
                | Err(GuestCallSettlementError::Interrupted(err))
                | Err(GuestCallSettlementError::Trap(err)) => {
                    invoke_result_from_trap::<Ctx>(store, consumed_fuel, err).await
                }
                Err(GuestCallSettlementError::Infrastructure(error)) => Err(error),
            }
        }
        PreparedCall::Invoke {
            method_name,
            input,
            expected_output,
            ..
        } if !expected_output.uses_streams() && at_return_usage.is_some() => {
            let guest = load_agent_guest(store, instance)?;
            *at_return_usage.expect("the guard checked the usage slot") =
                prepare_metered_guest_call(store, display_name, false).await?;
            let returned = attribution.export_returned(invocation_key);
            let result = run_guest_call_until_return(store, async |accessor| {
                returned
                    .observe(attribution.armed(
                        invocation_key,
                        guest.call_invoke(accessor, method_name, input),
                    ))
                    .await
            })
            .await;
            store
                .data_mut()
                .durable_ctx_mut()
                .apply_return_cut_off(invocation_key);
            let consumed_fuel =
                finish_invocation_and_get_fuel_consumption(store, display_name).await?;
            match result {
                Ok(Ok(Ok(invoke_output))) => {
                    decode_invoke_output(store, invoke_output).and_then(|output| {
                        validate_invoke_output(display_name, &expected_output, &output)?;
                        Ok(InvokeResult::Succeeded {
                            consumed_fuel,
                            result: Box::new(AgentInvocationResult::AgentMethod { output }),
                        })
                    })
                }
                Ok(Ok(Err(wire_err))) => invoke_result_from_agent_error(
                    store,
                    golem_common::model::agent::AgentInvocationKind::AgentMethod,
                    consumed_fuel,
                    wire_err,
                ),
                Ok(Err(err))
                | Err(GuestCallSettlementError::Interrupted(err))
                | Err(GuestCallSettlementError::Trap(err)) => {
                    invoke_result_from_trap::<Ctx>(store, consumed_fuel, err).await
                }
                Err(GuestCallSettlementError::Infrastructure(error)) => Err(error),
            }
        }
        PreparedCall::Initialize {
            agent_type, input, ..
        } => {
            let guest = load_agent_guest(store, instance)?;
            let _usage = prepare_metered_guest_call(store, display_name, false).await?;
            let result = run_guest_call_settled(store, async |accessor| {
                attribution
                    .armed(
                        invocation_key,
                        guest.call_initialize(accessor, agent_type, input),
                    )
                    .await
            })
            .await;
            let consumed_fuel =
                finish_invocation_and_get_fuel_consumption(store, display_name).await?;
            match result {
                Ok(Ok(Ok(()))) => Ok(InvokeResult::Succeeded {
                    consumed_fuel,
                    result: Box::new(AgentInvocationResult::AgentInitialization),
                }),
                Ok(Ok(Err(wire_err))) => invoke_result_from_agent_error(
                    store,
                    golem_common::model::agent::AgentInvocationKind::AgentInitialization,
                    consumed_fuel,
                    wire_err,
                ),
                Ok(Err(err))
                | Err(GuestCallSettlementError::Interrupted(err))
                | Err(GuestCallSettlementError::Trap(err)) => {
                    invoke_result_from_trap::<Ctx>(store, consumed_fuel, err).await
                }
                Err(GuestCallSettlementError::Infrastructure(error)) => Err(error),
            }
        }
        PreparedCall::Invoke {
            method_name,
            input,
            expected_output,
            ..
        } => {
            let guest = load_agent_guest(store, instance)?;
            let _usage = prepare_metered_guest_call(store, display_name, false).await?;
            let result = if expected_output.uses_streams() {
                let runtime = store.data().durable_ctx().runtime_suspension.clone();
                let entity_invocation = store
                    .data()
                    .durable_ctx()
                    .entity_invocation_scope()
                    .map(|scope| scope.invocation_id().clone());
                let policy = store.data().durable_ctx().suspension_policy();
                let runtime_for_call = runtime.clone();
                let future = store
                    .as_context_mut()
                    .run_concurrent(async move |accessor| {
                        if let (Some(runtime), Some(activity)) =
                            (&runtime_for_call, accessor.runtime_activity())
                        {
                            runtime.certify_root(activity);
                        }
                        let call = attribution.armed(
                            invocation_key,
                            guest.call_invoke(accessor, method_name, input),
                        );
                        attribution.scoped(Some(invocation_key), call).await
                    });
                let result = match runtime {
                    Some(runtime) => {
                        runtime
                            .drive(future, entity_invocation.as_ref(), policy, false)
                            .await
                    }
                    None => future.await,
                };
                let interrupted = store.data().durable_ctx().is_interrupting()
                    || store.data().durable_ctx().invocation_deadline_exceeded();
                match classify_guest_call_settlement(
                    store.data().agent_id(),
                    store.data().durable_ctx().last_replayed_index(),
                    result,
                    None,
                    interrupted,
                ) {
                    // A failed export has no stream result to materialize, but its tail
                    // work must settle before the invocation-local terminal is published.
                    Ok(Ok(Err(error))) => {
                        run_guest_call_settled(store, async |_accessor| Ok(Err(error))).await
                    }
                    other => other,
                }
            } else {
                run_guest_call_settled(store, async |accessor| {
                    attribution
                        .armed(
                            invocation_key,
                            guest.call_invoke(accessor, method_name, input),
                        )
                        .await
                })
                .await
            };
            let result = if expected_output.uses_streams() && matches!(&result, Ok(Ok(Err(_)))) {
                match run_stream_producers_settled(store, async |_accessor| Ok(())).await {
                    Ok(Ok(())) => result,
                    Ok(Err(error)) => Ok(Err(error)),
                    Err(error) => Err(error),
                }
            } else {
                result
            };
            let consumed_fuel =
                finish_invocation_and_get_fuel_consumption(store, display_name).await?;
            match result {
                Ok(Ok(Ok(invoke_output))) => {
                    let output = decode_invoke_output(store, invoke_output)?;
                    validate_invoke_output(display_name, &expected_output, &output)?;
                    Ok(InvokeResult::Succeeded {
                        consumed_fuel,
                        result: Box::new(AgentInvocationResult::AgentMethod { output }),
                    })
                }
                Ok(Ok(Err(wire_err))) => invoke_result_from_agent_error(
                    store,
                    golem_common::model::agent::AgentInvocationKind::AgentMethod,
                    consumed_fuel,
                    wire_err,
                ),
                Ok(Err(err))
                | Err(GuestCallSettlementError::Interrupted(err))
                | Err(GuestCallSettlementError::Trap(err)) => {
                    invoke_result_from_trap::<Ctx>(store, consumed_fuel, err).await
                }
                Err(GuestCallSettlementError::Infrastructure(error)) => Err(error),
            }
        }
        PreparedCall::SaveSnapshot { .. } => {
            let guest = load_save_snapshot_guest(store, instance)?;
            let _usage = prepare_metered_guest_call(store, display_name, false).await?;
            let result = run_guest_call_settled(store, async |accessor| {
                attribution
                    .armed(invocation_key, guest.call_save(accessor))
                    .await
            })
            .await;
            let consumed_fuel =
                finish_invocation_and_get_fuel_consumption(store, display_name).await?;
            match result {
                Ok(Ok(snapshot)) => Ok(InvokeResult::Succeeded {
                    consumed_fuel,
                    result: Box::new(AgentInvocationResult::SaveSnapshot {
                        snapshot: snapshot.into(),
                    }),
                }),
                Ok(Err(err))
                | Err(GuestCallSettlementError::Interrupted(err))
                | Err(GuestCallSettlementError::Trap(err)) => {
                    invoke_result_from_trap::<Ctx>(store, consumed_fuel, err).await
                }
                Err(GuestCallSettlementError::Infrastructure(error)) => Err(error),
            }
        }
        PreparedCall::LoadSnapshot {
            snapshot, input, ..
        } => {
            let guest = load_load_snapshot_guest(store, instance)?;
            let _usage = prepare_metered_guest_call(store, display_name, true).await?;
            let result = run_guest_call_settled(store, async |accessor| {
                attribution
                    .armed(invocation_key, guest.call_load(accessor, snapshot, input))
                    .await
            })
            .await;
            let consumed_fuel =
                finish_invocation_and_get_fuel_consumption(store, display_name).await?;
            match result {
                Ok(Ok(inner)) => Ok(InvokeResult::Succeeded {
                    consumed_fuel,
                    result: Box::new(AgentInvocationResult::LoadSnapshot { error: inner.err() }),
                }),
                Ok(Err(err))
                | Err(GuestCallSettlementError::Interrupted(err))
                | Err(GuestCallSettlementError::Trap(err)) => {
                    invoke_result_from_trap::<Ctx>(store, consumed_fuel, err).await
                }
                Err(GuestCallSettlementError::Infrastructure(error)) => Err(error),
            }
        }
        PreparedCall::ProcessOplogEntries {
            account_info,
            config,
            agent_id,
            metadata,
            first_entry_index,
            entries,
            ..
        } => {
            let guest = load_oplog_processor_guest(store, instance)?;
            let _usage = prepare_metered_guest_call(store, display_name, false).await?;
            let result = run_guest_call_settled(store, async |accessor| {
                attribution
                    .armed(
                        invocation_key,
                        guest.call_process(
                            accessor,
                            account_info,
                            config,
                            agent_id,
                            metadata,
                            first_entry_index,
                            entries,
                        ),
                    )
                    .await
            })
            .await;
            let consumed_fuel =
                finish_invocation_and_get_fuel_consumption(store, display_name).await?;
            match result {
                Ok(Ok(inner)) => Ok(InvokeResult::Succeeded {
                    consumed_fuel,
                    result: Box::new(AgentInvocationResult::ProcessOplogEntries {
                        error: inner.err(),
                    }),
                }),
                Ok(Err(err))
                | Err(GuestCallSettlementError::Interrupted(err))
                | Err(GuestCallSettlementError::Trap(err)) => {
                    invoke_result_from_trap::<Ctx>(store, consumed_fuel, err).await
                }
                Err(GuestCallSettlementError::Infrastructure(error)) => Err(error),
            }
        }
        PreparedCall::ExternalTool {
            activation,
            tool_name,
            command_path,
            input,
            stdin,
            stdout,
            stderr,
            caller,
        } => {
            let _usage = store.data().durable_ctx().begin_guest_resource_usage();
            store.data().durable_ctx().activate_live_resource_usage();
            prepare_guest_call(store, display_name).await;
            let result = crate::durable_host::tool::invoke_external_tool(
                store,
                activation,
                tool_name,
                command_path,
                *input,
                stdin,
                stdout,
                stderr,
                caller,
            )
            .await;
            let consumed_fuel =
                finish_invocation_and_get_fuel_consumption(store, display_name).await?;
            match result {
                Ok(Ok(result)) => Ok(InvokeResult::Succeeded {
                    consumed_fuel,
                    result: Box::new(AgentInvocationResult::ExternalTool { result }),
                }),
                Ok(Err(error)) => {
                    let retry_from = store.data().get_current_retry_point().await;
                    Ok(InvokeResult::from_error::<Ctx>(
                        consumed_fuel,
                        &error,
                        retry_from,
                        store.data().agent_mode(),
                    ))
                }
                Err(GuestCallSettlementError::Interrupted(error))
                | Err(GuestCallSettlementError::Trap(error)) => {
                    invoke_result_from_trap::<Ctx>(store, consumed_fuel, error).await
                }
                Err(GuestCallSettlementError::Infrastructure(error)) => Err(error),
            }
        }
    }
}

/// Resets call counters and emits the invocation-start event before a guest
/// call. Mirrors the bookkeeping the legacy dynamic dispatch performed.
pub(crate) async fn prepare_guest_call<Ctx: WorkerCtx>(
    store: &mut StoreContextMut<'_, Ctx>,
    display_name: &str,
) {
    rearm_fuel_check(store);
    store
        .data()
        .durable_ctx()
        .linear_memory_tracker()
        .clear_memory_growth_failure();
    store.data_mut().reset_invocation_call_counts();

    let idempotency_key = store.data().get_current_idempotency_key().await;
    if let Some(idempotency_key) = &idempotency_key {
        store
            .data()
            .get_public_state()
            .event_service()
            .emit_invocation_start(display_name, idempotency_key, store.data().is_live());
    }
}

/// Rearms enabled resource enforcement before entering an agent guest call.
async fn prepare_metered_guest_call<Ctx: WorkerCtx>(
    store: &mut StoreContextMut<'_, Ctx>,
    display_name: &str,
    reconstruction: bool,
) -> Result<Option<crate::services::linear_memory::GuestResourceUsageGuard>, WorkerExecutorError> {
    let usage = (!reconstruction).then(|| store.data().durable_ctx().begin_guest_resource_usage());
    rearm_fuel_check(store);
    let current_level = store.get_fuel().unwrap_or(0);
    let agent_mode = store.data().agent_mode();
    if let Err(error) = store.data_mut().ensure_fuel(current_level) {
        return Err(resource_capacity_error(agent_mode, error));
    }
    prepare_guest_call(store, display_name).await;
    if !reconstruction {
        store.data().durable_ctx().activate_live_resource_usage();
    }
    Ok(usage)
}

pub(crate) fn resource_capacity_error(
    agent_mode: AgentMode,
    error: OplogAgentError,
) -> WorkerExecutorError {
    match (agent_mode, &error) {
        (
            AgentMode::Durable,
            OplogAgentError::EphemeralFuelExhausted(_) | OplogAgentError::EphemeralCannotSuspend(_),
        ) => WorkerExecutorError::Interrupted {
            kind: InterruptKind::Suspend(golem_common::model::Timestamp::now_utc()),
        },
        _ => WorkerExecutorError::InvocationFailed {
            error,
            stderr: String::new(),
        },
    }
}

pub(crate) fn rearm_fuel_check<T>(store: &mut StoreContextMut<'_, T>) {
    store.set_epoch_deadline(0);
}

/// Builds an [`InvokeResult`] from a wasmtime trap (guest panic, interrupt,
/// exit, or runtime error) raised by a typed export call.
pub(crate) async fn invoke_result_from_trap<Ctx: WorkerCtx>(
    store: &mut StoreContextMut<'_, Ctx>,
    consumed_fuel: u64,
    err: wasmtime::Error,
) -> Result<InvokeResult, WorkerExecutorError> {
    let retry_from = store.data().get_current_retry_point().await;
    let agent_mode = store.data().agent_mode();
    let err = store
        .data()
        .durable_ctx()
        .linear_memory_tracker()
        .annotate_gc_heap_error(err);
    let err: anyhow::Error = err.into();
    if let Some(error) = replay_divergence_from_trap(&err, store.data().is_live()) {
        return Err(error);
    }
    Ok(InvokeResult::from_error::<Ctx>(
        consumed_fuel,
        &err,
        retry_from,
        agent_mode,
    ))
}

fn replay_divergence_from_trap(
    error: &anyhow::Error,
    is_live: bool,
) -> Option<WorkerExecutorError> {
    (!is_live)
        .then(|| {
            error
                .chain()
                .find_map(|error| error.downcast_ref::<WorkerExecutorError>())
        })
        .flatten()
        .filter(|error| matches!(error, WorkerExecutorError::UnexpectedOplogEntry { .. }))
        .cloned()
}

/// Maps a guest-returned `agent-error` (the `Err` arm of `initialize` /
/// `invoke`) into a failed [`InvokeResult`].
///
/// The guest export has already returned, so the instance and its resource
/// table stay alive while we decode the result. The `custom-error` payload is
/// decoded through the rejecting path so any owned `quota-token` handle the
/// guest smuggled into a domain error is deleted from the table rather than
/// leaked.
fn invoke_result_from_agent_error<Ctx: WorkerCtx>(
    store: &mut StoreContextMut<'_, Ctx>,
    kind: golem_common::model::agent::AgentInvocationKind,
    consumed_fuel: u64,
    wire_err: golem_agent::common::AgentError,
) -> Result<InvokeResult, WorkerExecutorError> {
    let agent_error =
        decode_agent_error_rejecting_quota_with(wire_err, store.data_mut().durable_ctx_mut())
            .map_err(|e| {
                WorkerExecutorError::runtime(format!(
                    "Failed to decode agent-error from guest: {e}"
                ))
            })?;
    if store.data().check_interrupt().is_none()
        && store
            .data()
            .durable_ctx()
            .selected_tool_owner_failure()
            .is_none()
    {
        store
            .data_mut()
            .durable_ctx_mut()
            .settle_schema_producers(Ok(()));
    }
    Ok(InvokeResult::from_agent_error(
        kind,
        consumed_fuel,
        agent_error,
    ))
}

/// Decodes the optional value returned by `invoke` into the schema-native
/// [`SchemaValue`] carried across the gRPC / oplog boundary.
///
/// A `none` result (the declared `unit` output) is represented by the
/// canonical empty tuple, matching the `unit` projection used on the caller
/// side ([`schema_value_to_wire_output`](crate::durable_host::rpc)).
fn decode_invoke_output<Ctx: WorkerCtx>(
    store: &mut StoreContextMut<'_, Ctx>,
    output: Option<core_wire::SchemaValueTree>,
) -> Result<SchemaValue, WorkerExecutorError> {
    match output {
        // `none` is the declared `unit` output.
        None => Ok(SchemaValue::Tuple {
            elements: Vec::new(),
        }),
        // Quota-token handles are lifted into trusted snapshots. Durable stream
        // sessions materialize stream handles before the invocation result is
        // committed.
        Some(tree) => {
            let output =
                decode_value_with(tree, store.data_mut().durable_ctx_mut()).map_err(|e| {
                    WorkerExecutorError::runtime(format!(
                        "Failed to decode agent method output: {e}"
                    ))
                })?;
            Ok(output)
        }
    }
}

#[cfg(test)]
fn reject_stream_at_materializing_boundary(
    value: SchemaValue,
) -> Result<SchemaValue, WorkerExecutorError> {
    if crate::durable_host::schema_value_stream::contains_stream(&value) {
        Err(WorkerExecutorError::runtime(
            "live stream at a materializing invocation boundary without a durable Stream Session",
        ))
    } else {
        Ok(value)
    }
}

/// Declared output shape of an agent method, carried from [`lower_invocation`]
/// (where the agent type and method are resolved) to [`dispatch_call`] (where
/// the guest's returned value is validated against it).
#[derive(Debug)]
pub struct ExpectedInvokeOutput {
    /// The agent type's shared definition graph; `SchemaType::Ref` nodes in
    /// [`root`](Self::root) resolve against its defs (the graph's own root is
    /// a placeholder — see [`AgentTypeSchema::schema`]).
    graph: SchemaGraph,
    /// The method's declared output type; the canonical empty tuple for
    /// `unit` outputs (see [`decode_invoke_output`]).
    root: SchemaType,
    uses_streams: bool,
}

impl ExpectedInvokeOutput {
    fn uses_streams(&self) -> bool {
        self.uses_streams
    }
}

/// Validates the decoded output of an agent method invocation against the
/// method's declared output schema, so a guest returning a mismatched value
/// fails deterministically at the invocation boundary instead of surfacing
/// later as a confusing shape mismatch in a consumer of the result.
fn validate_invoke_output(
    method_name: &str,
    expected: &ExpectedInvokeOutput,
    output: &SchemaValue,
) -> Result<(), WorkerExecutorError> {
    validate_value(&expected.graph, &expected.root, output).map_err(|errors| {
        WorkerExecutorError::runtime(format!(
            "Agent method '{method_name}' returned a value that does not match its declared output schema: {}",
            errors
                .iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("; ")
        ))
    })
}

/// Per-instance cache of typed guest export handles.
///
/// Resolving a typed export (`GuestIndices::new` + `load`) performs name-based
/// export lookups and typed function signature checks against the component.
/// Both the wasmtime [`Instance`](wasmtime::component::Instance) and the
/// [`Store`](wasmtime::Store) holding this cache live for the entire worker
/// instance lifetime and are reused across every invocation, so the resolved
/// [`Guest`](guest_exports::Guest) handles (cheaply cloneable bundles of
/// `Func` handles) can be resolved once and reused. Each interface is cached
/// independently and resolved lazily on first use, because not every component
/// exports every interface (e.g. `oplog-processor` is optional).
#[derive(Clone, Default)]
pub(crate) struct AgentExportFuncs {
    agent_guest: Option<guest_exports::Guest>,
    save_snapshot: Option<save_snapshot_exports::Guest>,
    load_snapshot: Option<load_snapshot_exports::Guest>,
    oplog_processor: Option<oplog_processor_exports::Guest>,
}

/// Generates a per-instance cached loader for a typed guest export interface.
///
/// On the first call for a given worker instance the export is resolved and
/// stored in the [`AgentExportFuncs`] cache held by the worker's
/// `DurableWorkerCtx`; subsequent calls return the cached handle, skipping the
/// name-based lookup and typed signature checks.
macro_rules! cached_guest_loader {
    ($vis:vis $fn_name:ident, $exports:ident, $field:ident, $missing_msg:literal, $load_msg:literal) => {
        $vis fn $fn_name<Ctx: WorkerCtx>(
            store: &mut StoreContextMut<'_, Ctx>,
            instance: &wasmtime::component::Instance,
        ) -> Result<$exports::Guest, WorkerExecutorError> {
            if let Some(guest) = store
                .data()
                .durable_ctx()
                .agent_export_funcs()
                .$field
                .clone()
            {
                return Ok(guest);
            }

            let instance_pre = instance.instance_pre(&*store);
            let indices = $exports::GuestIndices::new(&instance_pre).map_err(|e| {
                WorkerExecutorError::invalid_request(format!(concat!($missing_msg, ": {}"), e))
            })?;
            let guest = indices.load(&mut *store, instance).map_err(|e| {
                WorkerExecutorError::invalid_request(format!(concat!($load_msg, ": {}"), e))
            })?;

            store
                .data_mut()
                .durable_ctx_mut()
                .agent_export_funcs_mut()
                .$field = Some(guest.clone());
            Ok(guest)
        }
    };
}

cached_guest_loader!(
    load_agent_guest,
    guest_exports,
    agent_guest,
    "agent guest export not available",
    "failed to load agent guest export"
);
cached_guest_loader!(
    load_save_snapshot_guest,
    save_snapshot_exports,
    save_snapshot,
    "save-snapshot export not available",
    "failed to load save-snapshot export"
);
cached_guest_loader!(
    pub(crate) load_load_snapshot_guest,
    load_snapshot_exports,
    load_snapshot,
    "load-snapshot export not available",
    "failed to load load-snapshot export"
);
cached_guest_loader!(
    load_oplog_processor_guest,
    oplog_processor_exports,
    oplog_processor,
    "oplog-processor export not available",
    "failed to load oplog-processor export"
);

pub(crate) async fn finish_invocation_and_get_fuel_consumption<Ctx: WorkerCtx>(
    store: &mut StoreContextMut<'_, Ctx>,
    display_name: &str,
) -> Result<u64, WorkerExecutorError> {
    if !store.data().fuel_metering_enabled() {
        return Ok(0);
    }
    let current_fuel_level = store.get_fuel().unwrap_or(0);
    let consumed_fuel_for_call = store.data_mut().return_fuel(current_fuel_level);

    if consumed_fuel_for_call > 0 {
        debug!(
            "Fuel consumed for call {display_name}: {}",
            consumed_fuel_for_call
        );
    }

    record_invocation_consumption(consumed_fuel_for_call);

    Ok(consumed_fuel_for_call)
}

#[derive(Debug, Clone)]
pub enum InvokeResult {
    /// The invoked function exited with exit code 0
    Exited { consumed_fuel: u64 },
    /// The method returned a decoded guest error without trapping the instance.
    MethodFailed {
        consumed_fuel: u64,
        error: OplogAgentError,
    },
    /// The invoked function has failed
    Failed {
        consumed_fuel: u64,
        error: OplogAgentError,
        /// Deadline failures share the public internal-error representation but are not evidence
        /// that a restored snapshot disagrees with recorded execution.
        timed_out: bool,
        retry_from: OplogIndex,
        /// Ephemeral semantic-retry override extracted from the failing
        /// `anyhow::Error` chain. Round-tripped via `as_trap_type` so the
        /// post-trap recovery path can honour it.
        semantic_trap_retry_override:
            Option<crate::durable_host::durability::SemanticTrapRetryOverride>,
        /// Where the failure came from.
        provenance: FailureProvenance,
    },
    /// The invoked function succeeded and produced a result
    Succeeded {
        consumed_fuel: u64,
        result: Box<AgentInvocationResult>,
    },
    /// The function was running but got interrupted
    Interrupted {
        consumed_fuel: u64,
        interrupt_kind: InterruptKind,
    },
}

impl InvokeResult {
    pub fn from_agent_error(
        kind: golem_common::model::agent::AgentInvocationKind,
        consumed_fuel: u64,
        error: golem_common::model::agent::AgentError,
    ) -> Self {
        let error = OplogAgentError::InternalError(error.to_string());
        if kind == golem_common::model::agent::AgentInvocationKind::AgentMethod {
            Self::MethodFailed {
                consumed_fuel,
                error,
            }
        } else {
            Self::Failed {
                consumed_fuel,
                error,
                timed_out: false,
                retry_from: OplogIndex::INITIAL,
                semantic_trap_retry_override: None,
                provenance: FailureProvenance::TargetReturned,
            }
        }
    }

    fn from_trap_type(consumed_fuel: u64, trap: TrapType) -> Self {
        match trap {
            TrapType::Interrupt(interrupt_kind) => Self::Interrupted {
                consumed_fuel,
                interrupt_kind,
            },
            TrapType::Exit => Self::Exited { consumed_fuel },
            TrapType::Error {
                error,
                retry_from,
                semantic_trap_retry_override,
                provenance,
            } => Self::Failed {
                consumed_fuel,
                timed_out: false,
                error,
                retry_from,
                semantic_trap_retry_override,
                provenance,
            },
        }
    }

    pub fn from_error<Ctx: WorkerCtx>(
        consumed_fuel: u64,
        error: &anyhow::Error,
        fallback_retry_from: OplogIndex,
        agent_mode: AgentMode,
    ) -> Self {
        match TrapType::from_error::<Ctx>(error, fallback_retry_from, agent_mode) {
            TrapType::Interrupt(kind) => InvokeResult::Interrupted {
                consumed_fuel,
                interrupt_kind: kind,
            },
            TrapType::Exit => InvokeResult::Exited { consumed_fuel },
            TrapType::Error {
                error,
                retry_from,
                semantic_trap_retry_override,
                provenance,
            } => InvokeResult::Failed {
                consumed_fuel,
                timed_out: false,
                error,
                retry_from,
                semantic_trap_retry_override,
                provenance,
            },
        }
    }

    pub(crate) fn is_snapshot_replay_divergence(&self) -> bool {
        matches!(
            self,
            Self::Failed {
                timed_out: false,
                error: OplogAgentError::DeterministicTrap(_)
                    | OplogAgentError::PermanentError(_)
                    | OplogAgentError::InternalError(_)
                    | OplogAgentError::ReadOnlyViolation(_)
                    | OplogAgentError::StackOverflow,
                ..
            }
        )
    }

    pub fn consumed_fuel(&self) -> u64 {
        match self {
            InvokeResult::Exited { consumed_fuel, .. }
            | InvokeResult::Failed { consumed_fuel, .. }
            | InvokeResult::MethodFailed { consumed_fuel, .. }
            | InvokeResult::Succeeded { consumed_fuel, .. }
            | InvokeResult::Interrupted { consumed_fuel, .. } => *consumed_fuel,
        }
    }

    pub fn as_trap_type<Ctx: WorkerCtx>(&self) -> Option<TrapType> {
        match self {
            InvokeResult::Failed {
                error,
                retry_from,
                semantic_trap_retry_override,
                provenance,
                ..
            } => Some(TrapType::Error {
                error: error.clone(),
                retry_from: *retry_from,
                semantic_trap_retry_override: semantic_trap_retry_override.clone(),
                provenance: *provenance,
            }),
            InvokeResult::Interrupted { interrupt_kind, .. } => {
                Some(TrapType::Interrupt(*interrupt_kind))
            }
            InvokeResult::Exited { .. } => Some(TrapType::Exit),
            _ => None,
        }
    }
}

/// A single agent invocation lowered to the typed `golem:agent@2.0.0` /
/// `golem:api` guest-export call it dispatches to.
///
/// This is the single place that maps a high-level [`AgentInvocation`] to the
/// schema-native wire arguments the typed `bindgen!` export accessors expect.
pub struct LoweredInvocation {
    /// A human-readable name for tracing/spans/oplog display
    /// (e.g., the agent method name "do-something")
    pub display_name: String,
    /// `Some(method_name)` when the invocation targets an `AgentMethod` whose
    /// `read_only` metadata is set, or `Some("load-snapshot")` for the specially
    /// restricted snapshot-loading lifecycle call. The worker-executor uses this
    /// to enable read-only invocation strictness for the duration of the call,
    /// trapping outgoing HTTP / RPC host calls with `AgentError::ReadOnlyViolation`.
    pub read_only_method: Option<String>,
    /// The typed export call to perform.
    call: LoweredCall,
}

impl LoweredInvocation {
    /// Whether the invocation records its result at the export's return, before its background
    /// work drains, when its caller supplies a [`ReturnRecorder`]: the constructor and an agent
    /// method whose declared result type contains no stream in the component the Store executes. A stream-free method
    /// admitted through a durable streaming session records at the return too; the session only
    /// carries its result.
    pub(crate) fn records_result_at_return(&self) -> bool {
        match &self.call {
            LoweredCall::Initialize { .. } => true,
            LoweredCall::Invoke {
                expected_output, ..
            } => !expected_output.uses_streams(),
            _ => false,
        }
    }

    fn input(&self) -> Option<&SchemaValue> {
        match &self.call {
            LoweredCall::Initialize { input, .. } | LoweredCall::Invoke { input, .. } => {
                Some(input)
            }
            LoweredCall::ExternalTool { input, .. } => Some(input.value()),
            _ => None,
        }
    }
}

/// The typed guest-export call an [`AgentInvocation`] lowers to.
///
/// `Initialize`/`Invoke` carry the schema-native input as a [`SchemaValue`]
/// rather than the encoded wire tree, because lowering it can mint owned
/// `quota-token` handles, which requires the guest store's resource table. The
/// encoding is therefore deferred to [`materialize_call`] at the actual
/// guest-call site (see [`PreparedCall`]).
enum LoweredCall {
    Initialize {
        agent_type: String,
        input: SchemaValue,
        caller: golem_common::model::caller::Caller,
    },
    Invoke {
        method_name: String,
        input: SchemaValue,
        caller: golem_common::model::caller::Caller,
        expected_output: Box<ExpectedInvokeOutput>,
    },
    SaveSnapshot {
        caller: golem_common::model::caller::Caller,
    },
    LoadSnapshot {
        snapshot: golem_api_1_x::host::Snapshot,
        input: SchemaValue,
        caller: golem_common::model::caller::Caller,
    },
    ProcessOplogEntries {
        account_info: oplog_processor_exports::AccountInfo,
        config: Vec<(String, String)>,
        agent_id: core_wire::AgentId,
        metadata: golem_api_1_x::host::AgentMetadata,
        first_entry_index: u64,
        entries: Vec<golem_api_1_x::oplog::OplogEntry>,
        caller: golem_common::model::caller::Caller,
    },
    ExternalTool {
        activation: Box<ToolActivationSnapshot>,
        tool_name: ToolName,
        command_path: Vec<String>,
        input: Box<TypedSchemaValue>,
        stdin: bool,
        stdout: bool,
        stderr: bool,
        caller: golem_common::model::caller::Caller,
    },
}

/// A [`LoweredCall`] whose schema-native inputs have been materialized into the
/// `golem:core/types@2.0.0` wire trees the `bindgen!`-generated guest accessors
/// expect. Produced by [`materialize_call`] once the guest store is available so
/// that `quota-token` snapshots can be lowered into owned handles in the guest's
/// resource table.
enum PreparedCall {
    Initialize {
        agent_type: String,
        input: core_wire::SchemaValueTree,
        caller: golem_common::model::caller::Caller,
    },
    Invoke {
        method_name: String,
        input: core_wire::SchemaValueTree,
        caller: golem_common::model::caller::Caller,
        expected_output: Box<ExpectedInvokeOutput>,
    },
    SaveSnapshot {
        caller: golem_common::model::caller::Caller,
    },
    LoadSnapshot {
        snapshot: golem_api_1_x::host::Snapshot,
        input: core_wire::SchemaValueTree,
        caller: golem_common::model::caller::Caller,
    },
    ProcessOplogEntries {
        account_info: oplog_processor_exports::AccountInfo,
        config: Vec<(String, String)>,
        agent_id: core_wire::AgentId,
        metadata: golem_api_1_x::host::AgentMetadata,
        first_entry_index: u64,
        entries: Vec<golem_api_1_x::oplog::OplogEntry>,
        caller: golem_common::model::caller::Caller,
    },
    ExternalTool {
        activation: std::sync::Arc<ToolActivationSnapshot>,
        tool_name: ToolName,
        command_path: Vec<String>,
        input: Box<TypedSchemaValue>,
        stdin: bool,
        stdout: bool,
        stderr: bool,
        caller: golem_common::model::caller::Caller,
    },
}

impl PreparedCall {
    /// Whether the call is an invocation with an invocation ID. Snapshot save and load,
    /// and oplog processing are not.
    fn has_invocation_id(&self) -> bool {
        matches!(
            self,
            Self::Initialize { .. } | Self::Invoke { .. } | Self::ExternalTool { .. }
        )
    }

    fn caller(&self) -> golem_common::model::caller::Caller {
        match self {
            Self::Initialize { caller, .. }
            | Self::Invoke { caller, .. }
            | Self::ExternalTool { caller, .. }
            | Self::SaveSnapshot { caller }
            | Self::LoadSnapshot { caller, .. }
            | Self::ProcessOplogEntries { caller, .. } => caller.clone(),
        }
    }
}

/// Encode the schema-native inputs of a [`LoweredCall`] into the wire trees the
/// guest expects, minting any capability handles into the guest store's resource
/// table through the resolvers implemented by
/// [`DurableWorkerCtx`](crate::durable_host::DurableWorkerCtx).
///
/// For live invocations this runs after the invocation input's capability
/// snapshots have been admitted, so no guest-owned capability handle is minted
/// before its required permissions are checked. Replay materializes the
/// previously admitted snapshots without consulting current authority.
fn materialize_call<Ctx: WorkerCtx>(
    store: &mut StoreContextMut<'_, Ctx>,
    call: LoweredCall,
) -> Result<PreparedCall, WorkerExecutorError> {
    Ok(match call {
        LoweredCall::Initialize {
            agent_type,
            input,
            caller,
        } => {
            let input =
                encode_value_with(&input, store.data_mut().durable_ctx_mut()).map_err(|e| {
                    WorkerExecutorError::runtime(format!(
                        "Failed to encode agent initialization input: {e}"
                    ))
                })?;
            PreparedCall::Initialize {
                agent_type,
                input,
                caller,
            }
        }
        LoweredCall::Invoke {
            method_name,
            input,
            caller,
            expected_output,
            ..
        } => {
            let input = {
                let mut resolver = StoreValueResolver::new(store);
                encode_value_with_streams(&input, &mut resolver).map_err(|e| {
                    WorkerExecutorError::runtime(format!(
                        "Failed to encode agent method input: {e}"
                    ))
                })?
            };
            PreparedCall::Invoke {
                method_name,
                input,
                caller,
                expected_output,
            }
        }
        LoweredCall::SaveSnapshot { caller } => PreparedCall::SaveSnapshot { caller },
        LoweredCall::LoadSnapshot {
            snapshot,
            input,
            caller,
        } => {
            let input =
                encode_value_with(&input, store.data_mut().durable_ctx_mut()).map_err(|e| {
                    WorkerExecutorError::runtime(format!(
                        "Failed to encode load-snapshot constructor input: {e}"
                    ))
                })?;
            PreparedCall::LoadSnapshot {
                snapshot,
                input,
                caller,
            }
        }
        LoweredCall::ProcessOplogEntries {
            account_info,
            config,
            agent_id,
            metadata,
            first_entry_index,
            entries,
            caller,
        } => PreparedCall::ProcessOplogEntries {
            account_info,
            config,
            agent_id,
            metadata,
            first_entry_index,
            entries,
            caller,
        },
        LoweredCall::ExternalTool {
            activation,
            tool_name,
            command_path,
            input,
            stdin,
            stdout,
            stderr,
            caller,
        } => PreparedCall::ExternalTool {
            activation: std::sync::Arc::from(activation),
            tool_name,
            command_path,
            input,
            stdin,
            stdout,
            stderr,
            caller,
        },
    })
}

pub(crate) fn native_invocation_start(
    method_name: &str,
    input: &SchemaValue,
    allow_streams: bool,
) -> Result<golem_api_grpc::proto::golem::worker::InvocationStart, WorkerExecutorError> {
    let mut ordinal = 0;
    let input = golem_common::schema::streaming::encode_recursive_stream_value(input, |_, _| {
        let id = ordinal;
        ordinal += 1;
        Ok(id)
    })
    .map_err(WorkerExecutorError::invalid_request)?;
    Ok(golem_api_grpc::proto::golem::worker::InvocationStart {
        method_name: Some(method_name.to_string()),
        input: Some(input),
        allow_streams,
        ..Default::default()
    })
}

/// Checks only the request against the component chosen for a missing owner. This neither
/// registers streams nor enters a guest, and must run before Create is persisted.
pub(crate) fn validate_fresh_invocation(
    start: &golem_api_grpc::proto::golem::worker::InvocationStart,
    component_metadata: &ComponentMetadata,
    owner_context: &ResolvedOwnerContext,
) -> Result<(), WorkerExecutorError> {
    let input = crate::grpc::capture_method_input(start)?;
    let invocation = AgentInvocation::AgentMethod {
        idempotency_key: golem_common::model::InvocationId(uuid::Uuid::nil()),
        method_name: start
            .method_name
            .clone()
            .ok_or_else(|| WorkerExecutorError::invalid_request("method_name is required"))?,
        input,
        allow_streams: start.allow_streams,
        invocation_context: golem_common::model::invocation_context::InvocationContextStack::fresh(
        ),
        caller: golem_common::model::caller::Caller::anonymous(),
        scope_card: None,
    };
    lower_invocation(invocation, component_metadata, owner_context, None).map(|_| ())
}

pub fn lower_invocation(
    invocation: AgentInvocation,
    component_metadata: &ComponentMetadata,
    owner_context: &ResolvedOwnerContext,
    execution_input: Option<TypedSchemaValue>,
) -> Result<LoweredInvocation, WorkerExecutorError> {
    let agent_id = owner_context.agent();
    match invocation {
        AgentInvocation::AgentInitialization { input, caller, .. } => {
            let agent_type = resolve_agent_type(component_metadata, agent_id)?;
            // The input carrier is already the schema-native parameter-record
            // value the guest export expects. Encoding to the wire tree (which
            // may mint owned capability handles) is deferred to
            // `materialize_call` where the guest store is available.
            Ok(LoweredInvocation {
                display_name: "initialize".to_string(),
                read_only_method: None,
                call: LoweredCall::Initialize {
                    agent_type: agent_type.type_name.to_string(),
                    input,
                    caller,
                },
            })
        }
        AgentInvocation::AgentMethod {
            method_name,
            input,
            allow_streams,
            caller,
            ..
        } => {
            let agent_type = resolve_agent_type(component_metadata, agent_id)?;
            let method = agent_type
                .methods
                .iter()
                .find(|m| m.name == method_name)
                .ok_or_else(|| {
                    WorkerExecutorError::invalid_request(format!(
                        "Agent method '{method_name}' not found in agent type '{}'",
                        agent_type.type_name
                    ))
                })?;

            if matches!(&input, golem_common::model::agent::AgentMethodInput::Native(input) if input.is_external_rest())
                && let Some(output) = method.output_schema.schema()
                && let Some(occurrence) =
                    golem_schema::schema::host_managed::find_host_managed_type(
                        &agent_type.schema,
                        output,
                    )
                    .map_err(|error| WorkerExecutorError::invalid_request(error.to_string()))?
            {
                return Err(WorkerExecutorError::invalid_request(format!(
                    "Agent method output schema contains host-managed capability {} at {}; this method cannot be invoked through external REST",
                    occurrence.kind.kind_name(),
                    occurrence.path,
                )));
            }
            let input = decode_method_input(&input, agent_type, method, execution_input.as_ref())?;
            let read_only_method = method.read_only.is_some().then(|| method_name.clone());
            let stream_metadata = validate_method_invocation(
                component_metadata,
                agent_type,
                method,
                &input,
                &method_name,
            )?;
            if !allow_streams && stream_metadata.uses_streams() {
                return Err(WorkerExecutorError::invalid_request(
                    "the selected method requires a streaming invocation",
                ));
            }

            let expected_output = Box::new(ExpectedInvokeOutput {
                graph: agent_type.schema.clone(),
                uses_streams: stream_metadata.output,
                root: match method.output_schema.schema() {
                    Some(ty) => ty.clone(),
                    None => SchemaType::tuple(Vec::new()),
                },
            });

            Ok(LoweredInvocation {
                display_name: method_name.clone(),
                read_only_method,
                call: LoweredCall::Invoke {
                    method_name,
                    input,
                    caller,
                    expected_output,
                },
            })
        }
        AgentInvocation::ExternalTool {
            tool_name,
            command_path,
            input,
            stdin,
            stdout,
            stderr,
            activation,
            caller,
            ..
        } => Ok(LoweredInvocation {
            display_name: format!("{tool_name}:{}", command_path.join("/")),
            read_only_method: None,
            call: LoweredCall::ExternalTool {
                activation,
                tool_name,
                command_path,
                input: execution_input.map(Box::new).unwrap_or(input),
                stdin,
                stdout,
                stderr,
                caller,
            },
        }),
        AgentInvocation::ManualUpdate { .. } => Err(WorkerExecutorError::invalid_request(
            "ManualUpdate should not be invoked as a wasm function directly".to_string(),
        )),
        AgentInvocation::SaveSnapshot { caller, .. } => Ok(LoweredInvocation {
            display_name: "save-snapshot".to_string(),
            read_only_method: None,
            call: LoweredCall::SaveSnapshot { caller },
        }),
        AgentInvocation::LoadSnapshot {
            snapshot, caller, ..
        } => {
            let input = match owner_context {
                ResolvedOwnerContext::Agent(agent) => agent.parameters.value().clone(),
                ResolvedOwnerContext::OplogProcessor => SchemaValue::Record { fields: Vec::new() },
                ResolvedOwnerContext::ComponentBaseline => {
                    return Err(WorkerExecutorError::invalid_request(
                        "load-snapshot needs an agent with constructor parameters".to_string(),
                    ));
                }
            };
            Ok(LoweredInvocation {
                display_name: "load-snapshot".to_string(),
                read_only_method: Some("load-snapshot".to_string()),
                call: LoweredCall::LoadSnapshot {
                    snapshot: snapshot.into(),
                    input,
                    caller,
                },
            })
        }
        AgentInvocation::ProcessOplogEntries {
            account_id,
            config,
            metadata,
            first_entry_index,
            entries,
            caller,
            ..
        } => {
            let agent_id: core_wire::AgentId = metadata.agent_id.into();
            let account_info = oplog_processor_exports::AccountInfo {
                account_id: account_id.into(),
            };
            let metadata = metadata.into();
            let entries = entries
                .into_iter()
                .map(golem_api_1_x::oplog::OplogEntry::try_from)
                .collect::<Result<Vec<_>, String>>()
                .map_err(|e| {
                    WorkerExecutorError::runtime(format!(
                        "Failed to convert oplog entry for processing: {e}"
                    ))
                })?;

            Ok(LoweredInvocation {
                display_name: "process-oplog-entries".to_string(),
                read_only_method: None,
                call: LoweredCall::ProcessOplogEntries {
                    account_info,
                    config,
                    agent_id,
                    metadata,
                    first_entry_index: u64::from(first_entry_index),
                    entries,
                    caller,
                },
            })
        }
    }
}

pub(crate) fn decode_method_input(
    input: &golem_common::model::agent::AgentMethodInput,
    agent_type: &AgentTypeSchema,
    method: &AgentMethodSchema,
    prepared: Option<&TypedSchemaValue>,
) -> Result<SchemaValue, WorkerExecutorError> {
    use crate::grpc::stream_element_schema;
    use golem_common::model::agent::AgentMethodInput;
    use golem_common::model::invocation_session_public::PublicErrorCode;
    use golem_common::schema::public_json::{
        PublicSchemaValueError, PublicStreamReferencePolicy, decode_public_schema_value,
    };
    use golem_common::schema::streaming::encode_recursive_stream_value_with_schema;
    use golem_common::schema::{FieldSource, NamedFieldType};
    use golem_schema::schema::fingerprint::schema_fingerprint_v1;

    let graph = SchemaGraph {
        defs: agent_type.schema.defs.clone(),
        root: SchemaType::record(
            method
                .input_schema
                .fields()
                .iter()
                .filter(|field| matches!(field.source, FieldSource::UserSupplied))
                .map(|field| NamedFieldType {
                    name: field.name.clone(),
                    body: field.schema.clone(),
                    metadata: field.metadata.clone(),
                })
                .collect(),
        ),
    };
    let mut bindings = Vec::new();
    if let Some(prepared) = prepared {
        encode_recursive_stream_value_with_schema(
            prepared.value(),
            prepared.graph(),
            prepared.root_type(),
            |stream, path| {
                let element = stream_element_schema(prepared.graph(), prepared.root_type(), path)?;
                let fingerprint = schema_fingerprint_v1(prepared.graph(), element)
                    .map_err(|error| error.to_string())?;
                let ordinal = bindings.len() as u64;
                bindings.push((stream.clone(), fingerprint));
                Ok(ordinal)
            },
        )
        .map_err(WorkerExecutorError::invalid_request)?;
    }
    let value = match input {
        AgentMethodInput::Native(input) => input.decode(|ordinal, _| {
            if prepared.is_none() {
                return Ok(
                    golem_common::schema::stream::SchemaValueStream::from_host_endpoint(
                        crate::grpc::TransportStreamId(ordinal as u64),
                    ),
                );
            }
            bindings
                .get(ordinal)
                .map(|(stream, _)| stream.clone())
                .ok_or_else(|| "stream-bearing input requires committed execution bindings".into())
        }),
        AgentMethodInput::PublicJson(input) => {
            let json = serde_json::from_str(input.as_str())
                .map_err(|error| WorkerExecutorError::invalid_request(error.to_string()))?;
            let mut streams = bindings.iter();
            let mut ordinal = 0u64;
            decode_public_schema_value(
                &graph,
                &graph.root,
                &json,
                PublicStreamReferencePolicy::Provisional,
                |_, _| {
                    if prepared.is_none() {
                        let stream =
                            golem_common::schema::stream::SchemaValueStream::from_host_endpoint(
                                crate::grpc::TransportStreamId(ordinal),
                            );
                        ordinal += 1;
                        return Ok(stream);
                    }
                    streams
                        .next()
                        .map(|(stream, _)| stream.clone())
                        .ok_or_else(|| {
                            PublicSchemaValueError::new(
                                PublicErrorCode::ValidationError,
                                "stream-bearing input requires committed execution bindings",
                            )
                        })
                },
            )
            .map_err(|error| error.to_string())
        }
    };
    let value = value.map_err(WorkerExecutorError::invalid_request)?;
    if !bindings.is_empty() {
        let mut ordinal = 0;
        encode_recursive_stream_value_with_schema(&value, &graph, &graph.root, |_, path| {
            let element = stream_element_schema(&graph, &graph.root, path)?;
            let fingerprint =
                schema_fingerprint_v1(&graph, element).map_err(|error| error.to_string())?;
            if bindings.get(ordinal).map(|(_, fingerprint)| *fingerprint) != Some(fingerprint) {
                return Err(
                    "recorded input stream is incompatible with the selected method".into(),
                );
            }
            ordinal += 1;
            Ok((ordinal - 1) as u64)
        })
        .map_err(WorkerExecutorError::invalid_request)?;
        if ordinal != bindings.len() {
            return Err(WorkerExecutorError::invalid_request(
                "selected method does not use every recorded input stream",
            ));
        }
    }
    Ok(value)
}

pub fn validate_agent_method_invocation(
    component_metadata: &ComponentMetadata,
    agent_id: Option<&ParsedAgentKey>,
    method_name: &str,
    input: &SchemaValue,
) -> Result<bool, WorkerExecutorError> {
    let agent_type = resolve_agent_type(component_metadata, agent_id)?;
    let method = agent_type
        .methods
        .iter()
        .find(|method| method.name == method_name)
        .ok_or_else(|| {
            WorkerExecutorError::invalid_request(format!(
                "Agent method '{method_name}' not found in agent type '{}'",
                agent_type.type_name
            ))
        })?;

    validate_method_invocation(component_metadata, agent_type, method, input, method_name)
        .map(AgentMethodStreamMetadata::uses_streams)
}

/// Classifies session work using the executing component's input and output schemas, including
/// input-only streams and optional streams that are absent from this invocation's values.
pub(crate) fn invocation_uses_streams(
    invocation: &AgentInvocation,
    component_metadata: &ComponentMetadata,
    agent_id: Option<&ParsedAgentKey>,
) -> bool {
    if matches!(invocation, AgentInvocation::ExternalTool { .. }) {
        return true;
    }
    let AgentInvocation::AgentMethod { method_name, .. } = invocation else {
        return false;
    };
    let method = resolve_agent_type(component_metadata, agent_id)
        .ok()
        .and_then(|agent_type| {
            agent_type
                .methods
                .iter()
                .find(|method| method.name == *method_name)
                .map(|method| (agent_type, method))
        });
    // Missing metadata is not proof of a scalar invocation; preserve session failure handling
    // while the normal lowering path reports the schema error.
    method.is_none_or(|(agent_type, method)| {
        component_metadata
            .agent_method_stream_metadata(&agent_type.type_name, &method.name)
            .is_none_or(|metadata| metadata.uses_streams())
    })
}

fn validate_method_invocation(
    component_metadata: &ComponentMetadata,
    agent_type: &AgentTypeSchema,
    method: &AgentMethodSchema,
    input: &SchemaValue,
    method_name: &str,
) -> Result<AgentMethodStreamMetadata, WorkerExecutorError> {
    method
        .validate_input(&agent_type.schema, input)
        .map_err(|error| {
            WorkerExecutorError::invalid_request(format!(
                "Method '{method_name}': invalid input parameter value: {error}"
            ))
        })?;
    component_metadata
        .agent_method_stream_metadata(&agent_type.type_name, &method.name)
        .ok_or_else(|| {
            WorkerExecutorError::invalid_request(format!(
                "Streaming classification for method '{method_name}' is missing from component metadata"
            ))
        })
}

/// Resolves the [`AgentTypeSchema`] an invocation targets: by name when an agent id
/// is available, otherwise the single declared agent type (or an error when the
/// component declares zero or multiple types and no id was provided).
fn resolve_agent_type<'a>(
    component_metadata: &'a ComponentMetadata,
    agent_id: Option<&ParsedAgentKey>,
) -> Result<&'a AgentTypeSchema, WorkerExecutorError> {
    match agent_id {
        Some(id) => component_metadata
            .find_agent_type_by_name_ref(&id.agent_type)
            .ok_or_else(|| {
                WorkerExecutorError::invalid_request(format!(
                    "Agent type '{}' not found in component",
                    id.agent_type
                ))
            }),
        None => match component_metadata.agent_types() {
            [single] => Ok(single),
            [] => Err(WorkerExecutorError::invalid_request(
                "component declares no agent types".to_string(),
            )),
            _ => Err(WorkerExecutorError::invalid_request(
                "agent id is required to resolve the agent type (component declares multiple)"
                    .to_string(),
            )),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::base_model::agent::Snapshotting;
    use golem_common::base_model::component_metadata::KnownExports;
    use golem_common::model::agent::AgentTypeName;
    use golem_common::model::caller::Caller;
    use golem_common::model::invocation_context::InvocationContextStack;
    use golem_common::model::oplog::{RawSnapshotData, ReplayStep};
    use golem_common::schema::TypedSchemaValue;
    use golem_common::schema::agent::{
        AgentConstructorSchema, AgentMethodSchema, NamedField, OutputSchema,
    };
    use golem_common::schema::graph::SchemaGraph;
    use golem_common::schema::schema_type::SchemaType;
    use golem_schema::Empty;
    use std::collections::BTreeMap;
    use test_r::test;
    use uuid::Uuid;

    #[test]
    fn decoded_method_error_is_not_a_trap_but_constructor_error_is() {
        use crate::workerctx::default::Context;
        use golem_common::model::agent::{AgentError, AgentInvocationKind};

        for kind in [
            AgentInvocationKind::AgentMethod,
            AgentInvocationKind::AgentInitialization,
        ] {
            let result = InvokeResult::from_agent_error(
                kind,
                37,
                AgentError::InvalidInput("method gave up".into()),
            );
            assert_eq!(result.consumed_fuel(), 37);
            match kind {
                AgentInvocationKind::AgentMethod => {
                    assert!(result.as_trap_type::<Context>().is_none());
                    let InvokeResult::MethodFailed { error, .. } = result else {
                        panic!("expected a method failure");
                    };
                    assert_eq!(
                        error,
                        OplogAgentError::InternalError("Invalid input: method gave up".into())
                    );
                }
                _ => assert!(matches!(
                    result.as_trap_type::<Context>(),
                    Some(TrapType::Error {
                        error: OplogAgentError::InternalError(_),
                        ..
                    })
                )),
            }
        }
    }

    #[test]
    async fn finalization_selected_owner_failure_cannot_replace_later_shard_loss() {
        let operations = crate::durable_host::tool::operation::OwnerToolOperations::new();
        assert!(
            operations
                .select_owner_failure(OwnerFailureWinner::Infrastructure(
                    WorkerExecutorError::runtime("earlier failure")
                ))
                .await
        );
        let mut result = Ok(InvokeResult::Interrupted {
            consumed_fuel: 41,
            interrupt_kind: InterruptKind::ShardLost,
        });
        apply_selected_owner_failure(&mut result, operations.selected_owner_failure().unwrap());
        assert!(matches!(
            result,
            Ok(InvokeResult::Interrupted {
                consumed_fuel: 41,
                interrupt_kind: InterruptKind::ShardLost
            })
        ));

        let mut ordinary = Err(WorkerExecutorError::unexpected_oplog_entry(
            OplogIndex::from_u64(1),
            "expected",
            "actual",
        ));
        apply_selected_owner_failure(&mut ordinary, operations.selected_owner_failure().unwrap());
        assert!(matches!(ordinary, Err(WorkerExecutorError::Runtime { .. })));
    }

    #[test]
    fn finalization_preserves_original_failure_and_semantic_retry_fields() {
        use crate::durable_host::durability::{
            SemanticTrapRetryOverride, SemanticTrapRetryVerdict,
        };
        use golem_common::model::RetryPolicyState;

        let mut result = Ok(InvokeResult::Failed {
            consumed_fuel: 42,
            timed_out: true,
            error: OplogAgentError::InternalError("original".into()),
            retry_from: OplogIndex::from_u64(17),
            semantic_trap_retry_override: Some(SemanticTrapRetryOverride {
                retry_from: OplogIndex::from_u64(19),
                policy_name: "selected".into(),
                verdict: SemanticTrapRetryVerdict::GiveUp,
                retry_policy_state: RetryPolicyState::Counter(3),
            }),
            provenance: crate::model::FailureProvenance::Host,
        });
        retain_invocation_failure(&mut result, Err(WorkerExecutorError::runtime("cleanup")));
        let Ok(InvokeResult::Failed {
            consumed_fuel,
            timed_out,
            error,
            retry_from,
            semantic_trap_retry_override: Some(policy),
            ..
        }) = result
        else {
            panic!("cleanup replaced the original failure");
        };
        assert_eq!(consumed_fuel, 42);
        assert!(timed_out);
        assert_eq!(error, OplogAgentError::InternalError("original".into()));
        assert_eq!(retry_from, OplogIndex::from_u64(17));
        assert_eq!(policy.retry_from, OplogIndex::from_u64(19));
        assert_eq!(policy.policy_name, "selected");
        assert_eq!(policy.verdict, SemanticTrapRetryVerdict::GiveUp);
        assert_eq!(policy.retry_policy_state, RetryPolicyState::Counter(3));
    }

    #[test]
    fn finalization_preserves_divergence_but_not_over_shard_loss() {
        let mut result = Err(WorkerExecutorError::unexpected_oplog_entry(
            OplogIndex::from_u64(1),
            "observation",
            "disposal",
        ));
        retain_invocation_failure(&mut result, Err(WorkerExecutorError::runtime("cleanup")));
        assert!(matches!(
            result,
            Err(WorkerExecutorError::UnexpectedOplogEntry { .. })
        ));
        retain_invocation_failure(
            &mut result,
            Ok(InvokeResult::Interrupted {
                consumed_fuel: 0,
                interrupt_kind: InterruptKind::ShardLost,
            }),
        );
        retain_invocation_failure(
            &mut result,
            Err(WorkerExecutorError::runtime("later cleanup")),
        );
        assert!(matches!(
            result,
            Ok(InvokeResult::Interrupted {
                interrupt_kind: InterruptKind::ShardLost,
                ..
            })
        ));

        for failure in [
            WorkerExecutorError::Interrupted {
                kind: InterruptKind::ShardLost,
            },
            WorkerExecutorError::OplogFenced {
                agent_id: golem_common::model::AgentId::from_parts(7, 1),
                expected_epoch: 1,
                actual_epoch: Some(2),
            },
        ] {
            let mut result = Err(WorkerExecutorError::unexpected_oplog_entry(
                OplogIndex::from_u64(1),
                "observation",
                "disposal",
            ));
            retain_invocation_failure(&mut result, Err(failure.clone()));
            retain_invocation_failure(
                &mut result,
                Ok(InvokeResult::Interrupted {
                    consumed_fuel: 0,
                    interrupt_kind: InterruptKind::Restart,
                }),
            );
            assert_eq!(result.unwrap_err(), failure);
        }
    }

    #[test]
    fn finalization_failure_replaces_only_a_successful_body() {
        let mut result = Ok(InvokeResult::Exited { consumed_fuel: 7 });
        retain_invocation_failure(
            &mut result,
            Err(WorkerExecutorError::unexpected_oplog_entry(
                OplogIndex::from_u64(1),
                "observation",
                "disposal",
            )),
        );
        retain_invocation_failure(&mut result, Err(WorkerExecutorError::runtime("settlement")));
        assert!(matches!(
            result,
            Err(WorkerExecutorError::UnexpectedOplogEntry { .. })
        ));

        let mut result = Ok(InvokeResult::MethodFailed {
            consumed_fuel: 11,
            error: OplogAgentError::InternalError("method error".into()),
        });
        retain_invocation_failure(&mut result, Err(WorkerExecutorError::runtime("cleanup")));
        assert_eq!(result.unwrap_err(), WorkerExecutorError::runtime("cleanup"));
    }

    #[test]
    fn snapshot_divergence_excludes_deadlines_and_infrastructure_failures() {
        for (error, timed_out, expected) in [
            (
                OplogAgentError::InternalError("boundary mismatch".into()),
                false,
                true,
            ),
            (
                OplogAgentError::InternalError("deadline".into()),
                true,
                false,
            ),
            (
                OplogAgentError::DeterministicTrap("unreachable".into()),
                false,
                true,
            ),
            (
                OplogAgentError::ReadOnlyViolation(
                    golem_common::model::oplog::ReadOnlyViolationError {
                        method: "load-snapshot".into(),
                        host_function: "keyvalue.set".into(),
                    },
                ),
                false,
                true,
            ),
            (
                OplogAgentError::TransientError("storage unavailable".into()),
                false,
                false,
            ),
            (
                OplogAgentError::Unknown("unclassified host failure".into()),
                false,
                false,
            ),
            (OplogAgentError::OutOfMemory, false, false),
            (OplogAgentError::ExceededMemoryLimit, false, false),
        ] {
            let result = InvokeResult::Failed {
                consumed_fuel: 0,
                error,
                timed_out,
                retry_from: OplogIndex::INITIAL,
                semantic_trap_retry_override: None,
                provenance: crate::model::FailureProvenance::Host,
            };
            assert_eq!(
                result.is_snapshot_replay_divergence(),
                expected,
                "{result:?}"
            );
        }
        assert!(
            !InvokeResult::Interrupted {
                consumed_fuel: 0,
                interrupt_kind: InterruptKind::Restart,
            }
            .is_snapshot_replay_divergence()
        );
    }

    #[test]
    fn unexpected_oplog_trap_is_propagated_during_replay_only() {
        let divergence =
            WorkerExecutorError::unexpected_oplog_entry(OplogIndex::from_u64(4), "End", "Start");
        let error = anyhow::Error::new(divergence.clone()).context("guest call failed");

        assert_eq!(replay_divergence_from_trap(&error, false), Some(divergence));
        assert_eq!(replay_divergence_from_trap(&error, true), None);
    }

    #[test]
    fn invocation_poll_reuses_production_thread_stack() {
        let runtime = crate::bootstrap::create_runtime().unwrap();
        runtime.block_on(async {
            tokio::spawn(async {
                let mut polls = 0;
                with_invocation_stack(std::future::poll_fn(|cx| {
                    // A temporary stack would have less than INVOCATION_STACK_SIZE remaining.
                    // Check every poll, including after suspension and possible task migration.
                    assert!(stacker::remaining_stack().unwrap() >= INVOCATION_STACK_SIZE);
                    polls += 1;
                    if polls < 100 {
                        cx.waker().wake_by_ref();
                        std::task::Poll::Pending
                    } else {
                        std::task::Poll::Ready(())
                    }
                }))
                .await;
                assert_eq!(polls, 100);
            })
            .await
            .unwrap();
        });
    }

    #[test]
    fn invocation_poll_stack_is_reestablished_after_suspension() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_stack_size(256 * 1024)
            .build()
            .unwrap();
        runtime.block_on(async {
            tokio::spawn(async {
                assert!(stacker::remaining_stack().unwrap() < 1024 * 1024);
                let mut polls = 0;
                let result = with_invocation_stack(std::future::poll_fn(|cx| {
                    assert!(stacker::remaining_stack().unwrap() > 8 * 1024 * 1024);
                    polls += 1;
                    if polls < 3 {
                        cx.waker().wake_by_ref();
                        std::task::Poll::Pending
                    } else {
                        std::task::Poll::Ready(42)
                    }
                }))
                .await;
                assert_eq!(result, 42);
                assert_eq!(polls, 3);
                assert!(stacker::remaining_stack().unwrap() < 1024 * 1024);
            })
            .await
            .unwrap();
        });
    }

    const AGENT_TYPE: &str = "test-agent";
    const METHOD_NAME: &str = "do-work";

    #[test]
    fn unavailable_capacity_error_is_not_converted_to_quota_suspension() {
        for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
            let unavailable = OplogAgentError::Unknown(
                crate::services::resource_limits::MonthlyResourceCapacityError::UNAVAILABLE_REASON
                    .to_string(),
            );
            let error = resource_capacity_error(mode, unavailable.clone());
            assert_eq!(
                error,
                WorkerExecutorError::InvocationFailed {
                    error: unavailable,
                    stderr: String::new(),
                },
                "unavailable capacity must remain an infrastructure failure for {mode:?}"
            );
        }
    }

    #[test]
    fn compute_exhaustion_maps_to_suspend_or_invocation_failure_by_agent_mode() {
        let durable = resource_capacity_error(
            AgentMode::Durable,
            OplogAgentError::EphemeralCannotSuspend(
                golem_common::model::oplog::EphemeralCannotSuspendError {
                    reason: "fuel exhausted".to_string(),
                },
            ),
        );
        assert!(matches!(
            durable,
            WorkerExecutorError::Interrupted {
                kind: InterruptKind::Suspend(_)
            }
        ));

        let ephemeral = resource_capacity_error(
            AgentMode::Ephemeral,
            OplogAgentError::EphemeralFuelExhausted(
                golem_common::model::oplog::EphemeralFuelExhaustedError { overdraft_limit: 0 },
            ),
        );
        assert!(matches!(
            ephemeral,
            WorkerExecutorError::InvocationFailed { .. }
        ));
    }

    #[test]
    async fn live_streaming_response_is_published_exactly_once() {
        let value = SchemaValue::U64(42);
        assert_eq!(
            reject_stream_at_materializing_boundary(value.clone()).unwrap(),
            value
        );
    }

    /// Component metadata with one agent type whose `do-work` method takes two
    /// parameters (`count: u32`, `label: string`).
    fn metadata() -> ComponentMetadata {
        let method = AgentMethodSchema {
            name: METHOD_NAME.to_string(),
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(vec![
                NamedField::user_supplied("count", SchemaType::u32()),
                NamedField::user_supplied("label", SchemaType::string()),
            ]),
            output_schema: OutputSchema::Unit,
            http_endpoint: Vec::new(),
            read_only: None,
        };
        metadata_with_method(method)
    }

    fn metadata_with_method(method: AgentMethodSchema) -> ComponentMetadata {
        let at = AgentTypeSchema {
            kind: golem_common::schema::agent::AgentTypeKind::Regular,
            type_name: AgentTypeName(AGENT_TYPE.to_string()),
            description: String::new(),
            source_language: String::new(),
            schema: SchemaGraph::empty(),
            constructor: AgentConstructorSchema {
                name: None,
                description: String::new(),
                prompt_hint: None,
                input_schema: InputSchema::Parameters(Vec::new()),
            },
            methods: vec![method],
            dependencies: Vec::new(),
            mode: AgentMode::Durable,
            http_mount: None,
            snapshotting: Snapshotting::Disabled(Empty {}),
            config: Vec::new(),
        };
        ComponentMetadata::from_parts(
            KnownExports::default(),
            Vec::new(),
            vec![at],
            BTreeMap::new(),
        )
    }

    fn agent_id() -> ParsedAgentKey {
        let parameters = TypedSchemaValue::new(
            SchemaGraph::anonymous(SchemaType::record(Vec::new())),
            SchemaValue::Record { fields: Vec::new() },
        );
        ParsedAgentKey::new(AgentTypeName(AGENT_TYPE.to_string()), parameters)
    }

    fn method_invocation(input: SchemaValue) -> AgentInvocation {
        let mut ordinal = 0;
        let proto =
            golem_common::schema::streaming::encode_recursive_stream_value(&input, |_, _| {
                let result = ordinal;
                ordinal += 1;
                Ok(result)
            })
            .unwrap();
        let (input, _) =
            golem_common::model::agent::NativeMethodInput::capture(proto, Vec::new()).unwrap();
        AgentInvocation::AgentMethod {
            idempotency_key: InvocationId(Uuid::from_u128(1)),
            method_name: METHOD_NAME.to_string(),
            input: golem_common::model::agent::AgentMethodInput::Native(input),
            allow_streams: true,
            invocation_context: InvocationContextStack::fresh(),
            caller: Caller::anonymous(),
            scope_card: None,
        }
    }

    #[test]
    fn method_with_valid_input_lowers_ok() {
        let metadata = metadata();
        let agent_id = agent_id();
        let input = SchemaValue::Record {
            fields: vec![SchemaValue::U32(7), SchemaValue::String("hi".to_string())],
        };
        let lowered = lower_invocation(
            method_invocation(input),
            &metadata,
            &ResolvedOwnerContext::Agent(Box::new(agent_id)),
            None,
        )
        .expect("valid input should lower");
        assert_eq!(lowered.display_name, METHOD_NAME);
        assert!(lowered.read_only_method.is_none());
    }

    #[test]
    fn restored_stream_bindings_do_not_replace_the_original_request() {
        use golem_common::schema::stream::SchemaValueStream;

        let tail = NamedField::user_supplied(
            "tail",
            SchemaType::option(SchemaType::list(SchemaType::stream(Some(
                SchemaType::string(),
            )))),
        );
        let method = AgentMethodSchema {
            name: METHOD_NAME.to_string(),
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(vec![
                NamedField::user_supplied("count", SchemaType::u32()),
                NamedField::user_supplied("items", SchemaType::stream(Some(SchemaType::u32()))),
                tail.clone(),
            ]),
            output_schema: OutputSchema::Unit,
            http_endpoint: Vec::new(),
            read_only: None,
        };
        let original = method_invocation(SchemaValue::Record {
            fields: vec![
                SchemaValue::U32(7),
                SchemaValue::Stream(SchemaValueStream::from_host_endpoint(())),
                SchemaValue::Option {
                    inner: Some(Box::new(SchemaValue::List {
                        elements: vec![SchemaValue::Stream(SchemaValueStream::from_host_endpoint(
                            (),
                        ))],
                    })),
                },
            ],
        });
        let prepared = TypedSchemaValue::new(
            SchemaGraph::anonymous(SchemaType::record(
                method
                    .input_schema
                    .fields()
                    .iter()
                    .map(|field| golem_common::schema::NamedFieldType {
                        name: field.name.clone(),
                        body: field.schema.clone(),
                        metadata: field.metadata.clone(),
                    })
                    .collect(),
            )),
            SchemaValue::Record {
                fields: vec![
                    SchemaValue::U32(999),
                    SchemaValue::Stream(SchemaValueStream::from_host_endpoint(42_u64)),
                    SchemaValue::Option {
                        inner: Some(Box::new(SchemaValue::List {
                            elements: vec![SchemaValue::Stream(
                                SchemaValueStream::from_host_endpoint(91_u64),
                            )],
                        })),
                    },
                ],
            },
        );
        let mut public_original = original.clone();
        let AgentInvocation::AgentMethod { input, .. } = &mut public_original else {
            unreachable!()
        };
        *input = golem_common::model::agent::AgentMethodInput::PublicJson(
            golem_common::model::agent::JsonMethodInput::capture(
                br#"{"kind":"record","value":{"fields":[
                    {"kind":"u32","value":7},
                    {"kind":"stream","value":{"provisionalRef":"00000000-0000-4000-8000-000000000011"}},
                    {"kind":"option","value":{"inner":{"kind":"list","value":{"elements":[
                        {"kind":"stream","value":{"provisionalRef":"00000000-0000-4000-8000-000000000012"}}
                    ]}}}}
                ]}}"#,
            )
            .unwrap(),
        );
        for original in [original, public_original] {
            let metadata = metadata_with_method(method.clone());
            let lowered = lower_invocation(
                original.clone(),
                &metadata,
                &ResolvedOwnerContext::Agent(Box::new(agent_id())),
                Some(prepared.clone()),
            )
            .unwrap();
            let LoweredCall::Invoke {
                input: SchemaValue::Record { fields },
                ..
            } = lowered.call
            else {
                panic!("expected method input");
            };
            assert_eq!(fields[0], SchemaValue::U32(7));
            let SchemaValue::Stream(stream) = &fields[1] else {
                panic!("expected restored stream")
            };
            assert_eq!(
                stream
                    .with_host_endpoint::<u64, _>(|endpoint| *endpoint)
                    .unwrap(),
                42
            );
            let SchemaValue::Option {
                inner: Some(nested),
            } = &fields[2]
            else {
                panic!("expected the original optional tail")
            };
            let SchemaValue::List { elements } = nested.as_ref() else {
                panic!("expected the original nested list")
            };
            let [SchemaValue::Stream(stream)] = elements.as_slice() else {
                panic!("expected the second restored stream")
            };
            assert_eq!(
                stream
                    .with_host_endpoint::<u64, _>(|endpoint| *endpoint)
                    .unwrap(),
                91
            );

            let mut changed = method.clone();
            changed.input_schema = InputSchema::Parameters(vec![
                NamedField::user_supplied("count", SchemaType::u32()),
                NamedField::user_supplied("items", SchemaType::stream(Some(SchemaType::string()))),
                tail.clone(),
            ]);
            let error = lower_invocation(
                original,
                &metadata_with_method(changed),
                &ResolvedOwnerContext::Agent(Box::new(agent_id())),
                Some(prepared.clone()),
            )
            .err()
            .unwrap();
            assert!(
                error
                    .to_string()
                    .contains("incompatible with the selected method"),
                "{error}"
            );
        }
    }

    #[test]
    fn deployed_method_input_keeps_its_encoding_and_selected_stream_bindings() {
        use golem_common::model::agent::{AgentMethodInput, NativeMethodInput};
        use golem_common::schema::stream::SchemaValueStream;

        let method = AgentMethodSchema {
            name: METHOD_NAME.to_string(),
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(vec![
                NamedField::user_supplied("count", SchemaType::u32()),
                NamedField::user_supplied("items", SchemaType::stream(Some(SchemaType::u32()))),
                NamedField::user_supplied("labels", SchemaType::stream(Some(SchemaType::string()))),
            ]),
            output_schema: OutputSchema::Unit,
            http_endpoint: Vec::new(),
            read_only: None,
        };
        let mut original = method_invocation(SchemaValue::Record { fields: Vec::new() });
        let AgentInvocation::AgentMethod { input, .. } = &mut original else {
            unreachable!()
        };
        let deployed_input = SchemaValue::Record {
            fields: vec![
                SchemaValue::U32(7),
                SchemaValue::Stream(SchemaValueStream::from_host_endpoint(8u64)),
                SchemaValue::Stream(SchemaValueStream::from_host_endpoint(9u64)),
            ],
        };
        let proto = golem_common::schema::streaming::encode_recursive_stream_value(
            &deployed_input,
            |stream, _| stream.with_host_endpoint::<u64, _>(|id| *id),
        )
        .unwrap();
        *input = AgentMethodInput::Native(NativeMethodInput::capture(proto, vec![]).unwrap().0);
        let prepared = TypedSchemaValue::new(
            SchemaGraph::anonymous(SchemaType::record(
                method
                    .input_schema
                    .fields()
                    .iter()
                    .map(|field| golem_common::schema::NamedFieldType {
                        name: field.name.clone(),
                        body: field.schema.clone(),
                        metadata: field.metadata.clone(),
                    })
                    .collect(),
            )),
            SchemaValue::Record {
                fields: vec![
                    SchemaValue::U32(999),
                    SchemaValue::Stream(SchemaValueStream::from_host_endpoint(42_u64)),
                    SchemaValue::Stream(SchemaValueStream::from_host_endpoint(91_u64)),
                ],
            },
        );
        let owner = ResolvedOwnerContext::Agent(Box::new(agent_id()));
        let metadata = metadata_with_method(method.clone());
        let provisional = lower_invocation(original.clone(), &metadata, &owner, None).unwrap();
        let LoweredCall::Invoke {
            input: SchemaValue::Record { fields },
            ..
        } = provisional.call
        else {
            panic!("expected provisional method input")
        };
        assert_eq!(fields[0], SchemaValue::U32(7));
        for (ordinal, field) in fields[1..].iter().enumerate() {
            let SchemaValue::Stream(stream) = field else {
                panic!("expected provisional stream")
            };
            assert_eq!(
                stream
                    .with_host_endpoint::<crate::grpc::TransportStreamId, _>(|endpoint| endpoint.0)
                    .unwrap(),
                ordinal as u64
            );
        }
        for reversed in [false, true] {
            let mut selected = method.clone();
            if reversed {
                let InputSchema::Parameters(fields) = &mut selected.input_schema;
                fields.swap(1, 2);
            }
            let metadata = metadata_with_method(selected);
            let lowered =
                lower_invocation(original.clone(), &metadata, &owner, Some(prepared.clone()));
            if reversed {
                assert!(
                    lowered
                        .err()
                        .unwrap()
                        .to_string()
                        .contains("incompatible with the selected method")
                );
                continue;
            }
            let lowered = lowered.unwrap();
            let LoweredCall::Invoke {
                input: SchemaValue::Record { fields },
                ..
            } = lowered.call
            else {
                panic!("expected method input")
            };
            assert_eq!(fields[0], SchemaValue::U32(7));
            let expected = [42_u64, 91_u64];
            for (field, expected) in fields[1..].iter().zip(expected) {
                let SchemaValue::Stream(stream) = field else {
                    panic!("expected restored stream")
                };
                assert_eq!(
                    stream
                        .with_host_endpoint::<u64, _>(|endpoint| *endpoint)
                        .unwrap(),
                    expected
                );
            }
        }
    }

    fn load_snapshot_invocation() -> AgentInvocation {
        AgentInvocation::LoadSnapshot {
            idempotency_key: InvocationId(Uuid::from_u128(2)),
            caller: crate::test_callers::sample_caller(),
            snapshot: RawSnapshotData {
                data: vec![1, 2, 3],
                mime_type: "application/octet-stream".to_string(),
            },
        }
    }

    #[test]
    fn load_snapshot_uses_read_only_strictness_and_passes_the_agents_constructor_input() {
        let constructor_input = SchemaValue::Record {
            fields: vec![SchemaValue::String("restored-key".to_string())],
        };
        let agent_id = ParsedAgentKey::new(
            AgentTypeName(AGENT_TYPE.to_string()),
            TypedSchemaValue::new(
                SchemaGraph::anonymous(SchemaType::record(Vec::new())),
                constructor_input.clone(),
            ),
        );
        let lowered = lower_invocation(
            load_snapshot_invocation(),
            &metadata(),
            &ResolvedOwnerContext::Agent(Box::new(agent_id)),
            None,
        )
        .expect("snapshot load should lower");

        assert_eq!(lowered.display_name, "load-snapshot");
        assert_eq!(lowered.read_only_method.as_deref(), Some("load-snapshot"));
        let LoweredCall::LoadSnapshot { input, .. } = lowered.call else {
            panic!("snapshot load must lower to a load-snapshot call");
        };
        assert_eq!(input, constructor_input);
    }

    #[test]
    fn load_snapshot_without_an_agent_key_is_rejected() {
        let Err(err) = lower_invocation(
            load_snapshot_invocation(),
            &metadata(),
            &ResolvedOwnerContext::ComponentBaseline,
            None,
        ) else {
            panic!("an owner without constructor parameters cannot load a snapshot");
        };
        assert!(
            err.to_string().contains("load-snapshot needs an agent"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn load_snapshot_for_a_processor_passes_an_empty_constructor_record() {
        let metadata = ComponentMetadata::from_parts(
            KnownExports {
                oplog_processor_interface: Some("golem:api/oplog-processor@1.5.0".into()),
                save_snapshot_interface: Some("golem:api/save-snapshot@1.5.0".into()),
                load_snapshot_interface: Some("golem:api/load-snapshot@1.5.0".into()),
                ..Default::default()
            },
            vec![],
            vec![],
            Default::default(),
        );
        assert!(
            metadata.has_oplog_processor()
                && metadata.has_save_snapshot()
                && metadata.has_load_snapshot()
        );
        let lowered = lower_invocation(
            load_snapshot_invocation(),
            &metadata,
            &ResolvedOwnerContext::OplogProcessor,
            None,
        )
        .expect("a snapshot-capable processor must be able to restore its snapshot");
        let LoweredCall::LoadSnapshot { input, .. } = lowered.call else {
            panic!("expected load-snapshot");
        };
        assert_eq!(input, SchemaValue::Record { fields: Vec::new() });
    }

    #[test]
    fn component_agent_without_recorded_parameters_cannot_resolve_an_owner() {
        let error = ResolvedOwnerContext::from_authoritative_kind(
            golem_common::model::agent::OwnerKind::ComponentAgent,
            &AgentId::from_parts(42, 1),
            None,
            &metadata(),
        )
        .unwrap_err();
        assert!(error.contains("no agent type"), "{error}");
    }

    #[test]
    fn save_snapshot_does_not_use_read_only_strictness() {
        let lowered = lower_invocation(
            AgentInvocation::SaveSnapshot {
                idempotency_key: InvocationId(Uuid::from_u128(3)),
                caller: Caller::anonymous(),
            },
            &metadata(),
            &ResolvedOwnerContext::Agent(Box::new(agent_id())),
            None,
        )
        .expect("snapshot save should lower");

        assert_eq!(lowered.display_name, "save-snapshot");
        assert!(lowered.read_only_method.is_none());
    }

    #[test]
    fn method_with_non_record_input_is_rejected() {
        let metadata = metadata();
        let agent_id = agent_id();
        let Err(err) = lower_invocation(
            method_invocation(SchemaValue::U32(1)),
            &metadata,
            &ResolvedOwnerContext::Agent(Box::new(agent_id)),
            None,
        ) else {
            panic!("non-record input must be rejected");
        };
        assert!(
            err.to_string().contains("expected record, found u32"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn method_with_wrong_arity_is_rejected() {
        let metadata = metadata();
        let agent_id = agent_id();
        // Only one value for two user-supplied parameters.
        let input = SchemaValue::Record {
            fields: vec![SchemaValue::U32(7)],
        };
        let Err(err) = lower_invocation(
            method_invocation(input),
            &metadata,
            &ResolvedOwnerContext::Agent(Box::new(agent_id)),
            None,
        ) else {
            panic!("arity mismatch must be rejected");
        };
        assert!(
            err.to_string().contains("has 1 field(s), expected 2"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn method_with_field_type_mismatch_is_rejected() {
        let metadata = metadata();
        let agent_id = agent_id();
        // Second field should be a string.
        let input = SchemaValue::Record {
            fields: vec![SchemaValue::U32(7), SchemaValue::Bool(true)],
        };
        let Err(err) = lower_invocation(
            method_invocation(input),
            &metadata,
            &ResolvedOwnerContext::Agent(Box::new(agent_id)),
            None,
        ) else {
            panic!("field type mismatch must be rejected");
        };
        assert!(
            err.to_string().contains("invalid input parameter value"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn streaming_output_is_accepted_at_the_invocation_boundary() {
        let metadata = metadata_with_method(AgentMethodSchema {
            name: METHOD_NAME.to_string(),
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(Vec::new()),
            output_schema: OutputSchema::Single(Box::new(SchemaType::stream(Some(
                SchemaType::u32(),
            )))),
            http_endpoint: Vec::new(),
            read_only: None,
        });
        let result = lower_invocation(
            method_invocation(SchemaValue::Record { fields: Vec::new() }),
            &metadata,
            &ResolvedOwnerContext::Agent(Box::new(agent_id())),
            None,
        );
        if let Err(error) = result {
            panic!("unexpected error: {error}");
        }
    }

    #[test]
    fn streaming_method_is_classified_while_stream_free_method_is_not() {
        let streaming = metadata_with_method(AgentMethodSchema {
            name: METHOD_NAME.to_string(),
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(Vec::new()),
            output_schema: OutputSchema::Single(Box::new(SchemaType::stream(Some(
                SchemaType::u32(),
            )))),
            http_endpoint: Vec::new(),
            read_only: None,
        });
        let empty_input = SchemaValue::Record { fields: Vec::new() };

        assert!(
            validate_agent_method_invocation(
                &streaming,
                Some(&agent_id()),
                METHOD_NAME,
                &empty_input,
            )
            .unwrap()
        );
        assert!(
            !validate_agent_method_invocation(
                &metadata_with_method(AgentMethodSchema {
                    name: METHOD_NAME.to_string(),
                    description: String::new(),
                    prompt_hint: None,
                    input_schema: InputSchema::Parameters(Vec::new()),
                    output_schema: OutputSchema::Unit,
                    http_endpoint: Vec::new(),
                    read_only: None,
                }),
                Some(&agent_id()),
                METHOD_NAME,
                &empty_input,
            )
            .unwrap()
        );
    }

    #[test]
    fn method_stream_classification_survives_metadata_roundtrips_and_copies() {
        let streaming = metadata_with_method(AgentMethodSchema {
            name: METHOD_NAME.to_string(),
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(vec![NamedField::user_supplied(
                "input",
                SchemaType::option(SchemaType::stream(Some(SchemaType::u32()))),
            )]),
            output_schema: OutputSchema::Unit,
            http_endpoint: Vec::new(),
            read_only: None,
        });
        let streaming = streaming
            .with_provision_configs(BTreeMap::new())
            .with_tools(BTreeMap::new());
        let bytes = golem_common::serialization::serialize(&streaming).unwrap();
        let stored: ComponentMetadata = golem_common::serialization::deserialize(&bytes).unwrap();
        assert_eq!(stored, streaming);
        let json = serde_json::to_value(&stored).unwrap();
        assert_eq!(
            serde_json::from_value::<ComponentMetadata>(json.clone()).unwrap(),
            stored
        );
        let mut missing = json;
        missing
            .as_object_mut()
            .unwrap()
            .remove("agentMethodStreams");
        assert!(serde_json::from_value::<ComponentMetadata>(missing).is_err());
        let proto: golem_api_grpc::proto::golem::component::ComponentMetadata =
            stored.try_into().unwrap();
        let decoded = ComponentMetadata::try_from(proto).unwrap();
        assert_eq!(decoded, streaming);
        assert_eq!(
            decoded
                .agent_method_stream_metadata(&AgentTypeName(AGENT_TYPE.to_string()), METHOD_NAME)
                .unwrap(),
            golem_common::model::component_metadata::AgentMethodStreamMetadata {
                input: true,
                output: false,
            }
        );
    }

    #[test]
    fn invocation_session_classification_covers_scalar_and_absent_streams() {
        let optional_stream = SchemaType::option(SchemaType::stream(Some(SchemaType::u32())));
        for (input_schema, output_schema, fields, expected, output_streams) in [
            (
                InputSchema::Parameters(Vec::new()),
                OutputSchema::Unit,
                Vec::new(),
                false,
                false,
            ),
            (
                InputSchema::Parameters(vec![NamedField::user_supplied(
                    "input",
                    optional_stream.clone(),
                )]),
                OutputSchema::Unit,
                vec![SchemaValue::Option { inner: None }],
                true,
                false,
            ),
            (
                InputSchema::Parameters(Vec::new()),
                OutputSchema::Single(Box::new(optional_stream)),
                Vec::new(),
                true,
                true,
            ),
        ] {
            let metadata = metadata_with_method(AgentMethodSchema {
                name: METHOD_NAME.to_string(),
                description: String::new(),
                prompt_hint: None,
                input_schema,
                output_schema,
                http_endpoint: Vec::new(),
                read_only: None,
            });
            let invocation = method_invocation(SchemaValue::Record { fields });
            assert_eq!(
                invocation_uses_streams(&invocation, &metadata, Some(&agent_id())),
                expected
            );
            let lowered = lower_invocation(
                invocation,
                &metadata,
                &ResolvedOwnerContext::Agent(Box::new(agent_id())),
                None,
            )
            .unwrap();
            assert_eq!(lowered.records_result_at_return(), !output_streams);
            let LoweredCall::Invoke {
                expected_output, ..
            } = lowered.call
            else {
                panic!("expected a lowered method invocation");
            };
            assert_eq!(expected_output.uses_streams(), output_streams);
        }
        let mut unknown = method_invocation(SchemaValue::Record { fields: Vec::new() });
        if let AgentInvocation::AgentMethod { method_name, .. } = &mut unknown {
            *method_name = "missing".to_string();
        }
        assert!(invocation_uses_streams(
            &unknown,
            &metadata(),
            Some(&agent_id())
        ));
    }

    #[test]
    fn materializing_boundary_rejects_a_real_stream_handle() {
        let stream = golem_common::schema::stream::SchemaValueStream::from_host_endpoint(());
        let output = SchemaValue::Record {
            fields: vec![SchemaValue::Stream(stream)],
        };

        let error = reject_stream_at_materializing_boundary(output)
            .expect_err("a live stream reaching materialization is a contract violation");
        assert!(
            error
                .to_string()
                .contains("live stream at a materializing invocation boundary"),
            "unexpected error: {error}"
        );
    }

    // --- validate_invoke_output ---

    fn unit_expected_output() -> ExpectedInvokeOutput {
        ExpectedInvokeOutput {
            graph: SchemaGraph::empty(),
            root: SchemaType::tuple(Vec::new()),
            uses_streams: false,
        }
    }

    #[test]
    fn unit_output_accepts_canonical_empty_tuple() {
        let output = SchemaValue::Tuple {
            elements: Vec::new(),
        };
        validate_invoke_output(METHOD_NAME, &unit_expected_output(), &output)
            .expect("empty tuple must satisfy a unit output schema");
    }

    #[test]
    fn unit_output_rejects_non_unit_value() {
        let Err(err) =
            validate_invoke_output(METHOD_NAME, &unit_expected_output(), &SchemaValue::U32(1))
        else {
            panic!("non-unit value for a unit output schema must be rejected");
        };
        assert!(
            err.to_string()
                .contains("does not match its declared output schema"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn declared_output_accepts_matching_value() {
        let expected = ExpectedInvokeOutput {
            graph: SchemaGraph::empty(),
            root: SchemaType::u32(),
            uses_streams: false,
        };
        validate_invoke_output(METHOD_NAME, &expected, &SchemaValue::U32(42))
            .expect("matching value must pass output validation");
    }

    #[test]
    fn declared_output_rejects_mismatched_value() {
        let expected = ExpectedInvokeOutput {
            graph: SchemaGraph::empty(),
            root: SchemaType::u32(),
            uses_streams: false,
        };
        let Err(err) = validate_invoke_output(METHOD_NAME, &expected, &SchemaValue::Bool(true))
        else {
            panic!("mismatched output value must be rejected");
        };
        let message = err.to_string();
        assert!(
            message.contains(METHOD_NAME)
                && message.contains("does not match its declared output schema"),
            "unexpected error: {message}"
        );
    }

    #[test]
    fn declared_output_resolves_refs_through_the_agent_graph() {
        use golem_common::schema::graph::SchemaTypeDef;
        use golem_common::schema::metadata::TypeId;

        let expected = ExpectedInvokeOutput {
            graph: SchemaGraph {
                defs: vec![SchemaTypeDef {
                    id: TypeId::new("Answer"),
                    name: None,
                    body: SchemaType::u32(),
                }],
                root: SchemaType::record(Vec::new()),
            },
            root: SchemaType::ref_to(TypeId::new("Answer")),
            uses_streams: false,
        };
        validate_invoke_output(METHOD_NAME, &expected, &SchemaValue::U32(42))
            .expect("ref output must resolve through the agent graph");
        assert!(
            validate_invoke_output(METHOD_NAME, &expected, &SchemaValue::Bool(true)).is_err(),
            "mismatched ref output must be rejected"
        );
    }

    #[test]
    fn tail_work_settlement_truth_table() {
        let cases = [
            // (active_spawned_tasks, expected)
            (0, true),
            // An active spawned task (pending durable append or I/O) blocks settlement.
            (1, false),
            (3, false),
        ];
        for (active_spawned_tasks, expected) in cases {
            assert_eq!(
                tail_work_settled(active_spawned_tasks),
                expected,
                "active_spawned_tasks: {active_spawned_tasks}"
            );
        }
    }

    #[test]
    fn wrapped_replay_jump_settles_as_interruption() {
        assert!(matches!(
            classify_guest_call_settlement::<()>(
                &own_agent_id(),
                OplogIndex::from_u64(1),
                Err(wasmtime::Error::from_anyhow(
                    anyhow::Error::new(WorkerExecutorError::Interrupted {
                        kind: InterruptKind::Jump,
                    })
                    .context("durable scope restart"),
                )),
                None,
                false,
            ),
            Err(GuestCallSettlementError::Interrupted(_))
        ));
    }

    /// The agent whose guest calls the tests settle.
    fn own_agent_id() -> AgentId {
        AgentId::from_parts(7, 2)
    }

    /// A host call whose oplog read failed fails the agent, not the guest call: the read failure,
    /// raw or already a `WorkerExecutorError`, settles as an unreadable oplog of the agent.
    #[test]
    fn an_unreadable_oplog_settles_as_infrastructure() {
        let unreadable = UnreadableOplog {
            agent_id: own_agent_id(),
            error: crate::services::oplog::OplogReadError::Gap {
                start: OplogIndex::INITIAL,
                end: OplogIndex::from_u64(2),
            },
        };
        let expected = WorkerExecutorError::from(unreadable.clone());
        for error in [
            wasmtime::Error::from_anyhow(
                anyhow::Error::from(unreadable.clone()).context("host call"),
            ),
            wasmtime::Error::from_anyhow(
                anyhow::Error::from(expected.clone()).context("host call"),
            ),
        ] {
            match classify_guest_call_settlement::<()>(
                &own_agent_id(),
                OplogIndex::from_u64(1),
                Err(error),
                None,
                false,
            ) {
                Err(GuestCallSettlementError::Infrastructure(error)) => {
                    assert_eq!(error, expected)
                }
                Err(_) => panic!("an unreadable oplog did not settle as infrastructure"),
                Ok(()) => panic!("an unreadable oplog settled"),
            }
        }
    }

    /// The oplog of another agent that a host call could not read fails only the call, as a trap,
    /// when it failed for good, and recovers this agent when a later read can pass. Neither fails
    /// this agent with the cause of the other.
    #[test]
    fn an_unreadable_oplog_of_another_agent_fails_only_the_call() {
        let other = AgentId::from_parts(7, 3);
        let unreadable = |error| UnreadableOplog {
            agent_id: other,
            error,
        };
        let corrupt = unreadable(crate::services::oplog::OplogReadError::Gap {
            start: OplogIndex::INITIAL,
            end: OplogIndex::from_u64(2),
        });
        for error in [
            anyhow::Error::from(corrupt.clone()),
            anyhow::Error::from(WorkerExecutorError::from(corrupt)),
        ] {
            assert!(matches!(
                classify_guest_call_settlement::<()>(
                    &own_agent_id(),
                    OplogIndex::from_u64(1),
                    Err(wasmtime::Error::from_anyhow(error.context("host call"))),
                    None,
                    false,
                ),
                Err(GuestCallSettlementError::Trap(_))
            ));
        }
        let unavailable = unreadable(crate::services::oplog::OplogReadError::source_unavailable(
            crate::services::oplog::OplogReadSource::Archive(1),
            "outage",
        ));
        assert!(matches!(
            classify_guest_call_settlement::<()>(
                &own_agent_id(),
                OplogIndex::from_u64(1),
                Err(wasmtime::Error::from_anyhow(
                    anyhow::Error::from(unavailable).context("host call")
                )),
                None,
                false,
            ),
            Err(GuestCallSettlementError::Infrastructure(
                WorkerExecutorError::RecoveryRequired { .. }
            ))
        ));
    }

    #[test]
    async fn gc_exhaustion_without_backtrace_remains_a_guest_failure() -> anyhow::Result<()> {
        use wasmtime::{Engine, Instance, Module, Store, StoreLimitsBuilder};

        let engine =
            Engine::new(&golem_common::wasmtime_config::create_wasmtime_config_without_fs_cache())?;
        let module = Module::new(
            &engine,
            r#"(module
                (type $bytes (array (mut i8)))
                (func (export "allocate")
                    (drop (array.new_default $bytes (i32.const 1048576)))))"#,
        )?;
        let mut store = Store::new(
            &engine,
            StoreLimitsBuilder::new().memory_size(64 * 1024).build(),
        );
        store.limiter(|limits| limits);
        store.set_fuel(u64::MAX)?;
        store.set_epoch_deadline(u64::MAX);
        let instance = Instance::new_async(&mut store, &module, &[]).await?;
        let allocate = instance.get_typed_func::<(), ()>(&mut store, "allocate")?;
        for outer_failure in [true, false] {
            let error = allocate.call_async(&mut store, ()).await.unwrap_err();
            let error =
                wasmtime::Error::new(error.downcast::<wasmtime::GcHeapOutOfMemory<()>>().unwrap());
            assert!(error.downcast_ref::<wasmtime::WasmBacktrace>().is_none());
            let error = if outer_failure {
                match classify_guest_call_settlement::<()>(
                    &own_agent_id(),
                    OplogIndex::INITIAL,
                    Err(error),
                    None,
                    false,
                ) {
                    Err(GuestCallSettlementError::Trap(error)) => error,
                    other => panic!("GC exhaustion must remain a resource trap: {other:?}"),
                }
            } else {
                classify_guest_call_settlement(
                    &own_agent_id(),
                    OplogIndex::INITIAL,
                    Ok::<_, wasmtime::Error>(Err::<(), _>(error)),
                    None,
                    false,
                )
                .expect("the event loop settled")
                .expect_err("the nested guest call exhausted GC capacity")
            };
            assert!(matches!(
                TrapType::from_error::<crate::workerctx::default::Context>(
                    &error.into(),
                    OplogIndex::INITIAL,
                    golem_common::model::agent::AgentMode::Durable,
                ),
                TrapType::Error {
                    error: OplogAgentError::OutOfMemory,
                    ..
                }
            ));
        }
        Ok(())
    }

    #[test]
    fn guest_trap_and_settlement_failures_keep_distinct_provenance() {
        let guest_trap = classify_guest_call_settlement(
            &own_agent_id(),
            OplogIndex::from_u64(1),
            Ok::<_, wasmtime::Error>(Err::<(), _>(wasmtime::Error::msg("guest trapped"))),
            None,
            false,
        )
        .expect("the event loop itself settled");
        assert!(guest_trap.is_err());

        use crate::durable_host::stream_bus::StreamTerminalError;
        use golem_common::base_model::durable_stream::{
            StreamError, StreamTerminal, StreamTerminalAuthor,
        };
        use golem_common::model::agent::AgentMode;
        use golem_common::model::oplog::AgentError;

        let terminal = StreamTerminal::Failed(StreamError::Interrupted("source lost".into()));
        let result = classify_guest_call_settlement::<()>(
            &own_agent_id(),
            OplogIndex::from_u64(1),
            Err(wasmtime::Error::from_anyhow(
                StreamTerminalError {
                    authored_by: StreamTerminalAuthor::Protocol,
                    terminal: terminal.clone(),
                }
                .into_trap(),
            )),
            None,
            false,
        );
        let Err(GuestCallSettlementError::Trap(error)) = result else {
            panic!("a classified stream failure must remain a guest trap");
        };
        let error: anyhow::Error = error.into();
        assert_eq!(
            error
                .downcast_ref::<StreamTerminalError>()
                .unwrap()
                .terminal,
            terminal
        );
        assert!(matches!(
            TrapType::from_error::<crate::workerctx::default::Context>(
                &error,
                OplogIndex::INITIAL,
                AgentMode::Durable,
            ),
            TrapType::Error {
                error: AgentError::PermanentError(_),
                ..
            }
        ));

        assert!(matches!(
            classify_guest_call_settlement::<()>(
                &own_agent_id(),
                OplogIndex::from_u64(1),
                Err(wasmtime::Error::from_anyhow(
                    wasmtime::Trap::UnreachableCodeReached.into(),
                )),
                None,
                false,
            ),
            Err(GuestCallSettlementError::Trap(_))
        ));
        assert!(matches!(
            classify_guest_call_settlement::<()>(
                &own_agent_id(),
                OplogIndex::from_u64(1),
                Err(wasmtime::Error::from_anyhow(
                    GolemSpecificWasmTrap::WorkerOutOfMemory.into(),
                )),
                None,
                false,
            ),
            Err(GuestCallSettlementError::Trap(_))
        ));
        assert!(matches!(
            classify_guest_call_settlement::<()>(
                &own_agent_id(),
                OplogIndex::from_u64(1),
                Err(wasmtime::Error::from_anyhow(
                    wasmtime::Trap::AsyncDeadlock.into(),
                )),
                None,
                false,
            ),
            Err(GuestCallSettlementError::Infrastructure(_))
        ));
        assert!(matches!(
            classify_guest_call_settlement::<()>(
                &own_agent_id(),
                OplogIndex::from_u64(1),
                Err(wasmtime::Error::msg("host task failed")),
                None,
                false,
            ),
            Err(GuestCallSettlementError::Infrastructure(_))
        ));
        assert!(matches!(
            classify_guest_call_settlement::<()>(
                &own_agent_id(),
                OplogIndex::from_u64(1),
                Err(wasmtime::Error::msg("interrupted")),
                None,
                true,
            ),
            Err(GuestCallSettlementError::Interrupted(_))
        ));
        assert!(matches!(
            classify_guest_call_settlement::<()>(
                &own_agent_id(),
                OplogIndex::from_u64(1),
                Err(wasmtime::Error::from_anyhow(
                    InterruptKind::Suspend(golem_common::model::Timestamp::now_utc()).into(),
                )),
                None,
                false,
            ),
            Err(GuestCallSettlementError::Interrupted(_))
        ));
    }

    #[test]
    fn permanent_tool_guest_trap_accepts_only_fixed_instruction_traps() {
        use wasmtime::Trap;

        for trap in [
            Trap::StackOverflow,
            Trap::UnreachableCodeReached,
            Trap::MemoryOutOfBounds,
            Trap::TableOutOfBounds,
            Trap::IndirectCallToNull,
            Trap::BadSignature,
            Trap::IntegerOverflow,
            Trap::IntegerDivisionByZero,
            Trap::BadConversionToInteger,
            Trap::HeapMisaligned,
            Trap::NullReference,
            Trap::AtomicWaitNonSharedMemory,
            Trap::CastFailure,
            Trap::ArrayOutOfBounds,
            Trap::AllocationTooLarge,
        ] {
            let error = wasmtime::Error::from_anyhow(trap.into());
            assert_eq!(
                permanent_tool_guest_trap(&error),
                Some(format!("{error:#}"))
            );
        }
        for trap in [Trap::AsyncDeadlock, Trap::Interrupt, Trap::OutOfFuel] {
            assert_eq!(permanent_tool_guest_trap(&trap.into()), None);
        }

        let error = wasmtime::Error::from_anyhow(
            anyhow::Error::new(Trap::UnreachableCodeReached)
                .context("guest-requested trap: invalid tool input"),
        );
        assert_eq!(
            permanent_tool_guest_trap(&error),
            Some(format!(
                "guest-requested trap: invalid tool input: {}",
                Trap::UnreachableCodeReached
            ))
        );
    }

    #[test]
    fn permanent_tool_guest_trap_details_repeat_across_stores() {
        let engine = wasmtime::Engine::default();
        for instruction in ["unreachable", "i32.const 1 i32.const 0 i32.div_s drop"] {
            let module = wasmtime::Module::new(
                &engine,
                format!("(module (func (export \"run\") {instruction}))"),
            )
            .unwrap();
            let run = || {
                let mut store = wasmtime::Store::new(&engine, ());
                let instance = wasmtime::Instance::new(&mut store, &module, &[]).unwrap();
                let error = instance
                    .get_typed_func::<(), ()>(&mut store, "run")
                    .unwrap()
                    .call(&mut store, ())
                    .unwrap_err();
                assert!(error.downcast_ref::<wasmtime::WasmBacktrace>().is_some());
                permanent_tool_guest_trap(&error).expect("guest instruction trap")
            };
            assert_eq!(run(), run());
        }
    }

    #[test]
    fn permanent_tool_guest_trap_rejects_host_failures_with_backtraces() {
        use crate::durable_host::durability::{
            DurableCallTrapContext, HostFailureKind, SemanticTrapRetryOverride,
            SemanticTrapRetryVerdict, mark_durable_call_trap_context,
        };
        use golem_common::model::RetryPolicyState;

        let engine = wasmtime::Engine::default();
        let module = wasmtime::Module::new(
            &engine,
            r#"(module
                (import "host" "fail" (func $fail))
                (func (export "run") call $fail))"#,
        )
        .unwrap();
        let instruction_trap = || anyhow::Error::new(wasmtime::Trap::UnreachableCodeReached);
        for failure in [
            anyhow::anyhow!("unclassified host failure"),
            anyhow::Error::new(wasmtime::Trap::CastFailure)
                .context(GolemSpecificWasmTrap::WorkerExceededMemoryLimit),
            anyhow::Error::new(wasmtime::Trap::ArrayOutOfBounds)
                .context(GolemSpecificWasmTrap::WorkerOutOfMemory),
            mark_durable_call_trap_context(
                anyhow::Error::new(wasmtime::Trap::AllocationTooLarge),
                DurableCallTrapContext {
                    retry_from: OplogIndex::from_u64(17),
                },
            ),
            anyhow::Error::new(ClassifiedHostError {
                kind: HostFailureKind::Permanent,
                message: "permanent host failure".into(),
            }),
            mark_durable_call_trap_context(
                instruction_trap(),
                DurableCallTrapContext {
                    retry_from: OplogIndex::from_u64(17),
                },
            ),
            anyhow::Error::new(SemanticTrapRetryOverrideMarker {
                payload: SemanticTrapRetryOverride {
                    retry_from: OplogIndex::from_u64(17),
                    policy_name: "never".into(),
                    verdict: SemanticTrapRetryVerdict::GiveUp,
                    retry_policy_state: RetryPolicyState::Counter(1),
                },
                inner: instruction_trap(),
            }),
            anyhow::Error::new(DurableRecoveryFailure::new(
                DurableRecoveryFailureKind::Permanent,
                instruction_trap(),
            )),
            WorkerExecutorError::recovery_required("payload unavailable").into(),
            WorkerExecutorError::unexpected_oplog_entry(
                OplogIndex::from_u64(1),
                "expected",
                "actual",
            )
            .into(),
            WorkerExecutorError::runtime("persistence failed").into(),
            GolemSpecificWasmTrap::WorkerOutOfMemory.into(),
            GolemSpecificWasmTrap::WorkerExceededMemoryLimit.into(),
            GolemSpecificWasmTrap::WorkerExceededTableLimit.into(),
            GolemSpecificWasmTrap::WorkerMonthlyHttpCallBudgetExhausted.into(),
            GolemSpecificWasmTrap::WorkerMonthlyRpcCallBudgetExhausted.into(),
            InterruptKind::Restart.into(),
            wasmtime_wasi::I32Exit(0).into(),
        ] {
            let failure = std::sync::Mutex::new(Some(failure.context("host call context")));
            let mut linker = wasmtime::Linker::new(&engine);
            linker
                .func_wrap("host", "fail", move || -> wasmtime::Result<()> {
                    Err(wasmtime::Error::from_anyhow(
                        failure.lock().unwrap().take().unwrap(),
                    ))
                })
                .unwrap();
            let mut store = wasmtime::Store::new(&engine, ());
            let instance = linker.instantiate(&mut store, &module).unwrap();
            let error = instance
                .get_typed_func::<(), ()>(&mut store, "run")
                .unwrap()
                .call(&mut store, ())
                .unwrap_err();
            assert!(error.downcast_ref::<wasmtime::WasmBacktrace>().is_some());
            assert_eq!(permanent_tool_guest_trap(&error), None, "{error:#}");
        }
    }

    /// A guest instruction trap, with or without a Wasm backtrace, is a target failure; every
    /// host failure that a host call raises is `Host`, also with a Wasm backtrace; an interrupt
    /// or an exit is `Lifecycle`. `TrapType::from_error` and the failed invoke result carry the
    /// same provenance.
    #[test]
    fn failure_provenance_separates_target_traps_from_host_failures() {
        use crate::durable_host::durability::HostFailureKind;
        use crate::model::FailureProvenance as Provenance;
        use crate::workerctx::default::Context;

        let engine = wasmtime::Engine::default();
        let guest = wasmtime::Module::new(&engine, r#"(module (func (export "run") unreachable))"#)
            .unwrap();
        let mut store = wasmtime::Store::new(&engine, ());
        let instance = wasmtime::Instance::new(&mut store, &guest, &[]).unwrap();
        let guest_trap = instance
            .get_typed_func::<(), ()>(&mut store, "run")
            .unwrap()
            .call(&mut store, ())
            .unwrap_err();
        assert!(
            guest_trap
                .downcast_ref::<wasmtime::WasmBacktrace>()
                .is_some()
        );

        let host = wasmtime::Module::new(
            &engine,
            r#"(module
                (import "host" "fail" (func $fail))
                (func (export "run") call $fail))"#,
        )
        .unwrap();
        let host_call_failure = |failure: anyhow::Error| {
            let failure = std::sync::Mutex::new(Some(failure));
            let mut linker = wasmtime::Linker::new(&engine);
            linker
                .func_wrap("host", "fail", move || -> wasmtime::Result<()> {
                    Err(wasmtime::Error::from_anyhow(
                        failure.lock().unwrap().take().unwrap(),
                    ))
                })
                .unwrap();
            let mut store = wasmtime::Store::new(&engine, ());
            let instance = linker.instantiate(&mut store, &host).unwrap();
            let error = instance
                .get_typed_func::<(), ()>(&mut store, "run")
                .unwrap()
                .call(&mut store, ())
                .unwrap_err();
            assert!(error.downcast_ref::<wasmtime::WasmBacktrace>().is_some());
            error
        };

        let cases = [
            (guest_trap, Provenance::TargetInstruction),
            (
                wasmtime::Error::from_anyhow(
                    anyhow::Error::new(wasmtime::Trap::UnreachableCodeReached)
                        .context("guest-requested trap: panic"),
                ),
                Provenance::TargetInstruction,
            ),
            (
                host_call_failure(anyhow::anyhow!("unclassified host failure")),
                Provenance::Host,
            ),
            (
                host_call_failure(anyhow::Error::new(ClassifiedHostError {
                    kind: HostFailureKind::Transient,
                    message: "transient".into(),
                })),
                Provenance::Host,
            ),
            (
                host_call_failure(anyhow::Error::new(ClassifiedHostError {
                    kind: HostFailureKind::Permanent,
                    message: "permanent".into(),
                })),
                Provenance::Host,
            ),
            (
                host_call_failure(GolemSpecificWasmTrap::WorkerOutOfMemory.into()),
                Provenance::Host,
            ),
            (
                host_call_failure(GolemSpecificWasmTrap::WorkerExceededMemoryLimit.into()),
                Provenance::Host,
            ),
            (
                host_call_failure(InterruptKind::Restart.into()),
                Provenance::Lifecycle,
            ),
            (
                host_call_failure(
                    WorkerExecutorError::Interrupted {
                        kind: InterruptKind::Restart,
                    }
                    .into(),
                ),
                Provenance::Lifecycle,
            ),
            (
                host_call_failure(wasmtime_wasi::I32Exit(1).into()),
                Provenance::Lifecycle,
            ),
        ];
        for (error, expected) in cases {
            assert_eq!(failure_provenance(&error), expected, "{error:#}");
            let error: anyhow::Error = error.into();
            if let InvokeResult::Failed { provenance, .. } = InvokeResult::from_error::<Context>(
                0,
                &error,
                OplogIndex::INITIAL,
                AgentMode::Durable,
            ) {
                assert_eq!(provenance, expected, "{error:#}");
                let TrapType::Error {
                    provenance: from_trap,
                    ..
                } = TrapType::from_error::<Context>(
                    &error,
                    OplogIndex::INITIAL,
                    AgentMode::Durable,
                )
                else {
                    panic!("a failed invoke result comes from a trap error");
                };
                assert_eq!(from_trap, expected, "{error:#}");
                let trap = TrapType::Error {
                    error: OplogAgentError::Unknown("kept".into()),
                    retry_from: OplogIndex::INITIAL,
                    semantic_trap_retry_override: None,
                    provenance: expected,
                };
                assert!(matches!(
                    InvokeResult::from_trap_type(0, trap),
                    InvokeResult::Failed { provenance, .. } if provenance == expected
                ));
            } else {
                assert_eq!(expected, Provenance::Lifecycle, "{error:#}");
            }
        }
    }

    #[test]
    fn host_infrastructure_error_with_wasm_backtrace_stays_infrastructure() {
        let engine = wasmtime::Engine::default();
        let module = wasmtime::Module::new(
            &engine,
            r#"
                (module
                    (import "host" "fail" (func $fail))
                    (func (export "run")
                        call $fail))
            "#,
        )
        .unwrap();
        for failure in [
            WorkerExecutorError::runtime("registry unavailable"),
            crate::durable_host::tool_attachment_live_admission_rejected_error(),
        ] {
            let mut linker = wasmtime::Linker::new(&engine);
            let host_failure = failure.clone();
            linker
                .func_wrap("host", "fail", move || -> wasmtime::Result<()> {
                    Err(wasmtime::Error::from_anyhow(host_failure.clone().into()))
                })
                .unwrap();
            let mut store = wasmtime::Store::new(&engine, ());
            let instance = linker.instantiate(&mut store, &module).unwrap();
            let run = instance
                .get_typed_func::<(), ()>(&mut store, "run")
                .unwrap();
            let error = run.call(&mut store, ()).unwrap_err();

            assert!(
                error.downcast_ref::<wasmtime::WasmBacktrace>().is_some(),
                "the reproducer must exercise Wasmtime's attached backtrace context"
            );
            let Err(GuestCallSettlementError::Infrastructure(classified)) =
                classify_guest_call_settlement::<()>(
                    &own_agent_id(),
                    OplogIndex::from_u64(1),
                    Err(error),
                    None,
                    false,
                )
            else {
                panic!("host failure must remain infrastructure");
            };
            if crate::durable_host::is_tool_attachment_live_admission_rejection(&failure) {
                assert_eq!(classified, failure);
            }
        }
    }

    #[test]
    fn durable_recovery_failure_precedes_durable_call_semantic_marker() {
        use crate::durable_host::durability::{DurableCallTrapContext, DurableRecoveryFailure};

        let error = crate::durable_host::durability::mark_durable_call_trap_context(
            anyhow::Error::new(DurableRecoveryFailure::new(
                DurableRecoveryFailureKind::Retryable,
                std::io::Error::other("payload backend unavailable"),
            )),
            DurableCallTrapContext {
                retry_from: OplogIndex::from_u64(17),
            },
        );
        let result = classify_guest_call_settlement::<()>(
            &own_agent_id(),
            OplogIndex::from_u64(1),
            Err(wasmtime::Error::from_anyhow(error)),
            None,
            false,
        );

        let Err(GuestCallSettlementError::Infrastructure(WorkerExecutorError::RecoveryRequired {
            details,
            retry_from,
        })) = result
        else {
            panic!("retryable recovery failure must remain infrastructure recovery");
        };
        assert!(details.contains("payload backend unavailable"));
        assert_eq!(retry_from, Some(OplogIndex::from_u64(17)));
    }

    #[test]
    fn bare_recovery_required_preserves_its_retry_point() {
        let result = classify_guest_call_settlement::<()>(
            &own_agent_id(),
            OplogIndex::from_u64(1),
            Err(wasmtime::Error::from_anyhow(anyhow::Error::new(
                WorkerExecutorError::recovery_required_from(
                    "completed prefix payload unavailable",
                    OplogIndex::from_u64(23),
                ),
            ))),
            None,
            false,
        );

        let Err(GuestCallSettlementError::Infrastructure(WorkerExecutorError::RecoveryRequired {
            details,
            retry_from,
        })) = result
        else {
            panic!("bare recovery requirement must remain typed infrastructure recovery");
        };
        assert_eq!(details, "completed prefix payload unavailable");
        assert_eq!(retry_from, Some(OplogIndex::from_u64(23)));
    }

    #[test]
    fn permanent_durable_recovery_failure_is_not_retryable_recovery() {
        use crate::durable_host::durability::DurableRecoveryFailure;

        let inner = anyhow::anyhow!("payload 123 is missing")
            .context("failed to download incoming body chunk payload");
        let result = classify_guest_call_settlement::<()>(
            &own_agent_id(),
            OplogIndex::from_u64(1),
            Err(wasmtime::Error::from_anyhow(anyhow::Error::new(
                DurableRecoveryFailure::new(DurableRecoveryFailureKind::Permanent, inner),
            ))),
            None,
            false,
        );

        let Err(GuestCallSettlementError::Infrastructure(
            WorkerExecutorError::UnexpectedOplogEntry { divergence },
        )) = result
        else {
            panic!("permanent recovery failure must remain non-retryable infrastructure");
        };
        assert_eq!(divergence.oplog_index, OplogIndex::from_u64(1));
        assert_eq!(
            divergence.expected,
            ReplayStep::described("valid durable recovery data")
        );
        let ReplayStep::Other(got) = divergence.actual else {
            panic!("a recovery failure is described by its text");
        };
        assert!(
            got.description
                .contains("failed to download incoming body chunk payload")
        );
        assert!(got.description.contains("payload 123 is missing"));
    }

    #[test]
    fn settlement_timeout_is_infrastructure_even_after_root_return() {
        let result = classify_guest_call_settlement(
            &own_agent_id(),
            OplogIndex::from_u64(1),
            Ok(()),
            Some((std::time::Duration::from_secs(2), 1)),
            false,
        );
        let Err(GuestCallSettlementError::Infrastructure(error)) = result else {
            panic!("tail-work timeout must be an infrastructure failure");
        };
        assert!(error.to_string().contains("did not settle within 2s"));
    }
}
