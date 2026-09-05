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

use crate::durable_host::durability::{
    ClassifiedHostError, DurableCallTrapContextMarker, SemanticTrapRetryOverrideMarker,
};
use crate::durable_host::schema_value_stream::{StoreValueResolver, contains_stream};
use crate::durable_host::tool::operation::OwnerFailureWinner;
use crate::metrics::wasm::{record_invocation, record_invocation_consumption};
use crate::model::TrapType;
use crate::preview2::exports::golem::agent::guest as guest_exports;
use crate::preview2::exports::golem::api1_5_0::load_snapshot as load_snapshot_exports;
use crate::preview2::exports::golem::api1_5_0::save_snapshot as save_snapshot_exports;
use crate::preview2::oplog_processor_plugin::exports::golem::api1_5_0::oplog_processor as oplog_processor_exports;
use crate::preview2::{golem_agent, golem_api_1_x};
use crate::workerctx::{PublicWorkerIo, WorkerCtx};
use futures::FutureExt;
use golem_common::model::agent::{AgentMode, ParsedAgentId};
use golem_common::model::component_metadata::ComponentMetadata;
use golem_common::model::oplog::AgentError as OplogAgentError;
use golem_common::model::{AgentInvocation, AgentInvocationResult, OplogIndex};
use golem_common::schema::SchemaValue;
#[cfg(test)]
use golem_common::schema::agent::InputSchema;
use golem_common::schema::agent::wit::decode_agent_error_rejecting_quota_with;
use golem_common::schema::agent::{AgentMethodSchema, AgentTypeSchema, contains_stream_in_graph};
use golem_common::schema::graph::SchemaGraph;
use golem_common::schema::schema_type::SchemaType;
use golem_common::schema::validation::value::validate_value;
use golem_schema::schema::wit::wire as core_wire;
use golem_schema::schema::wit::{decode_value_with, encode_value_with, encode_value_with_streams};
use golem_service_base::error::worker_executor::{
    GolemSpecificWasmTrap, InterruptKind, WorkerExecutorError,
};
use tracing::{Instrument, Level, debug, span};
use wasmtime::component::Accessor;
use wasmtime::{AsContextMut, StoreContextMut};

/// Polls an invocation task outside Wasmtime's fiber and accessor TLS scopes, with enough native
/// stack for the nested host futures. Each poll returns before the temporary stack is released.
pub(crate) async fn with_invocation_stack<F: std::future::Future>(future: F) -> F::Output {
    const STACK_SIZE: usize = 16 * 1024 * 1024;
    let mut future = Box::pin(future);
    std::future::poll_fn(|cx| {
        stacker::maybe_grow(STACK_SIZE, STACK_SIZE, || future.as_mut().poll(cx))
    })
    .await
}

/// Describes how an invocation is being executed with respect to the oplog.
#[allow(clippy::large_enum_variant)]
pub enum InvocationMode {
    /// The invocation is happening live and should write oplog markers.
    Live(AgentInvocation),
    /// The invocation is being replayed from the oplog; no markers need to be written.
    Replay,
}

/// Invokes a function on a worker.
///
/// The context is held until the invocation finishes
///
/// Arguments:
/// - `lowered`: the lowered invocation describing what to invoke
/// - `store`: reference to the wasmtime instance's store
/// - `instance`: reference to the wasmtime instance
/// - `mode`: whether this is a live invocation or a replay
pub async fn invoke_observed_and_traced<Ctx: WorkerCtx>(
    lowered: LoweredInvocation,
    store: &mut impl AsContextMut<Data = Ctx>,
    instance: &wasmtime::component::Instance,
    mode: InvocationMode,
) -> Result<InvokeResult, WorkerExecutorError> {
    let mut store = store.as_context_mut();
    let was_live_before = store.data().is_live();

    let result = invoke_observed(lowered, &mut store, instance, mode).await;

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
        Ok(InvokeResult::Failed { .. }) => {
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
    mode: InvocationMode,
) -> Result<InvokeResult, WorkerExecutorError> {
    let mut store = store.as_context_mut();

    let LoweredInvocation {
        display_name,
        read_only_method,
        call,
    } = lowered;
    let operator_authorized_oplog_processor =
        matches!(&call, LoweredCall::ProcessOplogEntries { .. });

    if let InvocationMode::Live(invocation) = mode {
        let started = async {
            store
                .data_mut()
                .on_agent_invocation_started(invocation)
                .await
        }
        .instrument(span!(Level::INFO, "on_agent_invocation_started"))
        .await;
        if let Err(error) = started {
            store.data_mut().on_agent_invocation_finished().await;
            return Err(error);
        }
    }

    // The invocation start records the authority that admits any secret handles
    // carried by the input. Materialize those handles only after that admission.
    let call = match materialize_call(&mut store, call) {
        Ok(call) => call,
        Err(error) => {
            store.data_mut().on_agent_invocation_finished().await;
            return Err(error);
        }
    };

    let primary_body = match store
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
    let deadline = store.data().durable_ctx().arm_invocation_deadline();

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

    let invocation_principal = call.principal();
    store
        .data_mut()
        .durable_ctx_mut()
        .set_invocation_principal(invocation_principal);
    let call_future = dispatch_call(&mut store, instance, call, &display_name);

    let call_outcome = std::panic::AssertUnwindSafe(call_future)
        .catch_unwind()
        .await;
    store
        .data_mut()
        .durable_ctx_mut()
        .set_invocation_principal(None);

    if read_only_method.is_some() {
        store.data_mut().exit_read_only_mode();
    }
    drop(operator_authorization);

    let mut call_result = match call_outcome {
        Ok(result) => result,
        Err(payload) => std::panic::resume_unwind(payload),
    };

    if let Some(parent) = primary_body
        .as_ref()
        .and_then(|primary_body| primary_body.invocation().cloned())
    {
        if let Err(error) =
            crate::durable_host::tool::prepare_tool_parent_end(&mut store, parent.clone()).await
        {
            call_result = Err(error);
        }
        if let Some(primary_body) = primary_body {
            primary_body.complete();
        }
        if let Err(error) =
            crate::durable_host::tool::settle_tool_children(&mut store, parent).await
        {
            call_result = Err(error);
        }
        if let Some(owner_failure) = store.data().durable_ctx().selected_tool_owner_failure() {
            let consumed_fuel = call_result
                .as_ref()
                .map(InvokeResult::consumed_fuel)
                .unwrap_or_default();
            call_result = match owner_failure {
                OwnerFailureWinner::Trap(trap) => {
                    Ok(InvokeResult::from_trap_type(consumed_fuel, trap))
                }
                OwnerFailureWinner::Lifecycle(interrupt_kind) => Ok(InvokeResult::Interrupted {
                    consumed_fuel,
                    interrupt_kind,
                }),
                OwnerFailureWinner::Infrastructure(error) => Err(error),
            };
        }
    }

    store.data_mut().on_agent_invocation_finished().await;

    let call_result = apply_invocation_deadline(&mut store, deadline, call_result).await;

    if manages_owner_execution_status {
        store.data().set_suspended();
    }

    call_result
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
    deadline: crate::durable_host::InvocationDeadline,
    call_result: Result<InvokeResult, WorkerExecutorError>,
) -> Result<InvokeResult, WorkerExecutorError> {
    if !deadline.exceeded() || store.data().durable_ctx().is_interrupting() {
        return call_result;
    }
    match call_result {
        Ok(InvokeResult::Interrupted {
            consumed_fuel,
            interrupt_kind: InterruptKind::Interrupt(_),
        }) => {
            let retry_from = store.data().get_current_retry_point().await;
            let in_atomic_region = store.data().current_in_atomic_region();
            let atomic_region_had_side_effects =
                store.data().current_atomic_region_had_side_effects();
            Ok(InvokeResult::Failed {
                consumed_fuel,
                error: OplogAgentError::InternalError(format!(
                    "invocation exceeded the configured maximum invocation duration of {:?}",
                    deadline
                        .duration()
                        .expect("an exceeded deadline always has a configured duration")
                )),
                retry_from,
                in_atomic_region,
                atomic_region_had_side_effects,
                semantic_trap_retry_override: None,
            })
        }
        other => other,
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

fn is_guest_semantic_trap(error: &wasmtime::Error) -> bool {
    let chain_contains = |predicate: &dyn Fn(&(dyn std::error::Error + 'static)) -> bool| {
        error.chain().any(predicate)
    };

    if let Some(trap) = error.root_cause().downcast_ref::<wasmtime::Trap>() {
        return !matches!(trap, wasmtime::Trap::AsyncDeadlock);
    }
    if chain_contains(&|cause| {
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
    }) {
        return true;
    }
    if chain_contains(&|cause| cause.is::<WorkerExecutorError>()) {
        return false;
    }

    error.downcast_ref::<wasmtime::WasmBacktrace>().is_some()
}

fn classify_guest_call_settlement<R>(
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
        if interrupted || error.root_cause().downcast_ref::<InterruptKind>().is_some() {
            GuestCallSettlementError::Interrupted(error)
        } else if is_guest_semantic_trap(&error) {
            GuestCallSettlementError::Trap(error)
        } else {
            GuestCallSettlementError::Infrastructure(WorkerExecutorError::runtime(format!(
                "guest call settlement task failed: {error}"
            )))
        }
    })
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
    let tracker = store.data().durable_ctx().tail_work_tracker();
    let tracker_for_error = tracker.clone();
    let drain_started = std::sync::Arc::new(tokio::sync::Notify::new());
    let fun = {
        let drain_started = drain_started.clone();
        async move |accessor: &Accessor<Ctx>| {
            let result = fun(accessor).await;
            // The root future has completed: everything from here on is the (bounded) drain
            // phase. Arm the timeout here rather than in the settlement predicate — the
            // predicate is only consulted at idle observation points, which are never reached
            // if e.g. a guest task lingers, and the drain must stay bounded even then.
            drain_started.notify_one();
            result
        }
    };
    let mut settled = move |_store: StoreContextMut<'_, Ctx>| {
        let active = tracker.active_count();
        tracing::debug!("invocation tail-work settlement check: {active} spawned task(s) active");
        tail_work_settled(active)
    };
    let tail_work_deadline = store
        .data()
        .durable_ctx()
        .arm_tail_work_deadline(drain_started);
    let result = store
        .as_context_mut()
        .run_concurrent_and_settle(fun, &mut settled)
        .await;
    let interrupted = store.data().durable_ctx().is_interrupting()
        || store.data().durable_ctx().invocation_deadline_exceeded();
    let tail_timeout = if tail_work_deadline.exceeded() && !interrupted {
        Some((
            tail_work_deadline.duration(),
            tracker_for_error.active_count(),
        ))
    } else {
        None
    };
    classify_guest_call_settlement(result, tail_timeout, interrupted)
}

/// Dispatches a single lowered invocation to the matching typed guest export
/// accessor (`golem:agent/guest@2.0.0`, `golem:api/save-snapshot`,
/// `golem:api/load-snapshot`, or `golem:api/oplog-processor`) and maps its
/// typed result into an [`InvokeResult`].
async fn dispatch_call<Ctx: WorkerCtx>(
    store: &mut StoreContextMut<'_, Ctx>,
    instance: &wasmtime::component::Instance,
    call: PreparedCall,
    display_name: &str,
) -> Result<InvokeResult, WorkerExecutorError> {
    match call {
        PreparedCall::Initialize {
            agent_type,
            input,
            principal,
        } => {
            let guest = load_agent_guest(store, instance)?;
            prepare_guest_call(store, display_name).await;
            let result = run_guest_call_settled(store, async |accessor| {
                guest
                    .call_initialize(accessor, agent_type, input, principal)
                    .await
            })
            .await;
            let consumed_fuel =
                finish_invocation_and_get_fuel_consumption(store, display_name).await?;
            match result {
                Ok(Ok(Ok(()))) => Ok(InvokeResult::Succeeded {
                    consumed_fuel,
                    result: AgentInvocationResult::AgentInitialization,
                }),
                Ok(Ok(Err(wire_err))) => {
                    invoke_result_from_agent_error(store, consumed_fuel, wire_err)
                }
                Ok(Err(err))
                | Err(GuestCallSettlementError::Interrupted(err))
                | Err(GuestCallSettlementError::Trap(err)) => {
                    Ok(invoke_result_from_trap::<Ctx>(store, consumed_fuel, err).await)
                }
                Err(GuestCallSettlementError::Infrastructure(error)) => Err(error),
            }
        }
        PreparedCall::Invoke {
            method_name,
            input,
            principal,
            expected_output,
        } => {
            let guest = load_agent_guest(store, instance)?;
            prepare_guest_call(store, display_name).await;
            let result = if expected_output.uses_streams() {
                let result = store
                    .as_context_mut()
                    .run_concurrent(async |accessor| {
                        guest
                            .call_invoke(accessor, method_name, input, principal)
                            .await
                    })
                    .await;
                let interrupted = store.data().durable_ctx().is_interrupting()
                    || store.data().durable_ctx().invocation_deadline_exceeded();
                classify_guest_call_settlement(result, None, interrupted)
            } else {
                run_guest_call_settled(store, async |accessor| {
                    guest
                        .call_invoke(accessor, method_name, input, principal)
                        .await
                })
                .await
            };
            let consumed_fuel =
                finish_invocation_and_get_fuel_consumption(store, display_name).await?;
            match result {
                Ok(Ok(Ok(invoke_output))) => {
                    let output = decode_invoke_output(store, invoke_output)?;
                    validate_invoke_output(display_name, &expected_output, &output)?;
                    Ok(InvokeResult::Succeeded {
                        consumed_fuel,
                        result: AgentInvocationResult::AgentMethod { output },
                    })
                }
                Ok(Ok(Err(wire_err))) => {
                    invoke_result_from_agent_error(store, consumed_fuel, wire_err)
                }
                Ok(Err(err))
                | Err(GuestCallSettlementError::Interrupted(err))
                | Err(GuestCallSettlementError::Trap(err)) => {
                    Ok(invoke_result_from_trap::<Ctx>(store, consumed_fuel, err).await)
                }
                Err(GuestCallSettlementError::Infrastructure(error)) => Err(error),
            }
        }
        PreparedCall::SaveSnapshot => {
            let guest = load_save_snapshot_guest(store, instance)?;
            prepare_guest_call(store, display_name).await;
            let result =
                run_guest_call_settled(store, async |accessor| guest.call_save(accessor).await)
                    .await;
            let consumed_fuel =
                finish_invocation_and_get_fuel_consumption(store, display_name).await?;
            match result {
                Ok(Ok(snapshot)) => Ok(InvokeResult::Succeeded {
                    consumed_fuel,
                    result: AgentInvocationResult::SaveSnapshot {
                        snapshot: snapshot.into(),
                    },
                }),
                Ok(Err(err))
                | Err(GuestCallSettlementError::Interrupted(err))
                | Err(GuestCallSettlementError::Trap(err)) => {
                    Ok(invoke_result_from_trap::<Ctx>(store, consumed_fuel, err).await)
                }
                Err(GuestCallSettlementError::Infrastructure(error)) => Err(error),
            }
        }
        PreparedCall::LoadSnapshot { snapshot } => {
            let guest = load_load_snapshot_guest(store, instance)?;
            prepare_guest_call(store, display_name).await;
            let result = run_guest_call_settled(store, async |accessor| {
                guest.call_load(accessor, snapshot).await
            })
            .await;
            let consumed_fuel =
                finish_invocation_and_get_fuel_consumption(store, display_name).await?;
            match result {
                Ok(Ok(inner)) => Ok(InvokeResult::Succeeded {
                    consumed_fuel,
                    result: AgentInvocationResult::LoadSnapshot { error: inner.err() },
                }),
                Ok(Err(err))
                | Err(GuestCallSettlementError::Interrupted(err))
                | Err(GuestCallSettlementError::Trap(err)) => {
                    Ok(invoke_result_from_trap::<Ctx>(store, consumed_fuel, err).await)
                }
                Err(GuestCallSettlementError::Infrastructure(error)) => Err(error),
            }
        }
        PreparedCall::ProcessOplogEntries {
            account_info,
            config,
            component_id,
            agent_id,
            metadata,
            first_entry_index,
            entries,
        } => {
            let guest = load_oplog_processor_guest(store, instance)?;
            prepare_guest_call(store, display_name).await;
            let result = run_guest_call_settled(store, async |accessor| {
                guest
                    .call_process(
                        accessor,
                        account_info,
                        config,
                        component_id,
                        agent_id,
                        metadata,
                        first_entry_index,
                        entries,
                    )
                    .await
            })
            .await;
            let consumed_fuel =
                finish_invocation_and_get_fuel_consumption(store, display_name).await?;
            match result {
                Ok(Ok(inner)) => Ok(InvokeResult::Succeeded {
                    consumed_fuel,
                    result: AgentInvocationResult::ProcessOplogEntries { error: inner.err() },
                }),
                Ok(Err(err))
                | Err(GuestCallSettlementError::Interrupted(err))
                | Err(GuestCallSettlementError::Trap(err)) => {
                    Ok(invoke_result_from_trap::<Ctx>(store, consumed_fuel, err).await)
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

pub(crate) fn rearm_fuel_check<T>(store: &mut StoreContextMut<'_, T>) {
    store.set_epoch_deadline(0);
}

/// Builds an [`InvokeResult`] from a wasmtime trap (guest panic, interrupt,
/// exit, or runtime error) raised by a typed export call.
async fn invoke_result_from_trap<Ctx: WorkerCtx>(
    store: &mut StoreContextMut<'_, Ctx>,
    consumed_fuel: u64,
    err: wasmtime::Error,
) -> InvokeResult {
    let retry_from = store.data().get_current_retry_point().await;
    let in_atomic_region = store.data().current_in_atomic_region();
    let atomic_region_had_side_effects = store.data().current_atomic_region_had_side_effects();
    let agent_mode = store.data().agent_mode();
    let err: anyhow::Error = err.into();
    InvokeResult::from_error::<Ctx>(
        consumed_fuel,
        &err,
        retry_from,
        in_atomic_region,
        atomic_region_had_side_effects,
        agent_mode,
    )
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
    Ok(InvokeResult::Failed {
        consumed_fuel,
        error: OplogAgentError::InternalError(agent_error.to_string()),
        retry_from: OplogIndex::INITIAL,
        in_atomic_region: false,
        atomic_region_had_side_effects: false,
        semantic_trap_retry_override: None,
    })
}

/// Decodes the optional value returned by `invoke` into the schema-native
/// [`SchemaValue`] carried across the gRPC / oplog boundary.
///
/// A `none` result (the declared `unit` output) is represented by the
/// canonical empty tuple, matching the `unit` projection used on the caller
/// side ([`schema_value_to_wire_output`](crate::durable_host::wasm_rpc)).
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
}

impl ExpectedInvokeOutput {
    fn uses_streams(&self) -> bool {
        contains_stream_in_graph(&self.graph, &self.root)
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
    ($fn_name:ident, $exports:ident, $field:ident, $missing_msg:literal, $load_msg:literal) => {
        fn $fn_name<Ctx: WorkerCtx>(
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
    load_load_snapshot_guest,
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
    /// The invoked function has failed
    Failed {
        consumed_fuel: u64,
        error: OplogAgentError,
        retry_from: OplogIndex,
        /// Whether the trapping call was inside an atomic region (membership). Round-tripped via
        /// `as_trap_type` into `TrapType::Error` so the post-trap recovery decision uses the call's
        /// own region rather than "any region currently active".
        in_atomic_region: bool,
        /// Whether the trapping call's atomic region had recorded side effects. Round-tripped into
        /// the persisted `OplogEntry::Error.inside_atomic_region`.
        atomic_region_had_side_effects: bool,
        /// Ephemeral semantic-retry override extracted from the failing
        /// `anyhow::Error` chain. Round-tripped via `as_trap_type` so the
        /// post-trap recovery path can honour it.
        semantic_trap_retry_override:
            Option<crate::durable_host::durability::SemanticTrapRetryOverride>,
    },
    /// The invoked function succeeded and produced a result
    Succeeded {
        consumed_fuel: u64,
        result: AgentInvocationResult,
    },
    /// The function was running but got interrupted
    Interrupted {
        consumed_fuel: u64,
        interrupt_kind: InterruptKind,
    },
}

impl InvokeResult {
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
                in_atomic_region,
                atomic_region_had_side_effects,
                semantic_trap_retry_override,
            } => Self::Failed {
                consumed_fuel,
                error,
                retry_from,
                in_atomic_region,
                atomic_region_had_side_effects,
                semantic_trap_retry_override,
            },
        }
    }

    pub fn from_error<Ctx: WorkerCtx>(
        consumed_fuel: u64,
        error: &anyhow::Error,
        fallback_retry_from: OplogIndex,
        fallback_in_atomic_region: bool,
        fallback_atomic_region_had_side_effects: bool,
        agent_mode: AgentMode,
    ) -> Self {
        match TrapType::from_error::<Ctx>(
            error,
            fallback_retry_from,
            fallback_in_atomic_region,
            fallback_atomic_region_had_side_effects,
            agent_mode,
        ) {
            TrapType::Interrupt(kind) => InvokeResult::Interrupted {
                consumed_fuel,
                interrupt_kind: kind,
            },
            TrapType::Exit => InvokeResult::Exited { consumed_fuel },
            TrapType::Error {
                error,
                retry_from,
                in_atomic_region,
                atomic_region_had_side_effects,
                semantic_trap_retry_override,
            } => InvokeResult::Failed {
                consumed_fuel,
                error,
                retry_from,
                in_atomic_region,
                atomic_region_had_side_effects,
                semantic_trap_retry_override,
            },
        }
    }

    pub fn consumed_fuel(&self) -> u64 {
        match self {
            InvokeResult::Exited { consumed_fuel, .. }
            | InvokeResult::Failed { consumed_fuel, .. }
            | InvokeResult::Succeeded { consumed_fuel, .. }
            | InvokeResult::Interrupted { consumed_fuel, .. } => *consumed_fuel,
        }
    }

    pub fn as_trap_type<Ctx: WorkerCtx>(&self) -> Option<TrapType> {
        match self {
            InvokeResult::Failed {
                error,
                retry_from,
                in_atomic_region,
                atomic_region_had_side_effects,
                semantic_trap_retry_override,
                ..
            } => Some(TrapType::Error {
                error: error.clone(),
                retry_from: *retry_from,
                in_atomic_region: *in_atomic_region,
                atomic_region_had_side_effects: *atomic_region_had_side_effects,
                semantic_trap_retry_override: semantic_trap_retry_override.clone(),
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
    /// `read_only` metadata is set. The worker-executor uses this to enable the
    /// read-only invocation strictness mode for the duration of the call, trapping
    /// outgoing HTTP / RPC host calls with `AgentError::ReadOnlyViolation`.
    pub read_only_method: Option<String>,
    /// The typed export call to perform.
    call: LoweredCall,
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
        principal: golem_agent::common::Principal,
    },
    Invoke {
        method_name: String,
        input: SchemaValue,
        principal: golem_agent::common::Principal,
        expected_output: Box<ExpectedInvokeOutput>,
    },
    SaveSnapshot,
    LoadSnapshot {
        snapshot: golem_api_1_x::host::Snapshot,
    },
    ProcessOplogEntries {
        account_info: oplog_processor_exports::AccountInfo,
        config: Vec<(String, String)>,
        component_id: core_wire::ComponentId,
        agent_id: core_wire::AgentId,
        metadata: golem_api_1_x::host::AgentMetadata,
        first_entry_index: u64,
        entries: Vec<golem_api_1_x::oplog::OplogEntry>,
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
        principal: golem_agent::common::Principal,
    },
    Invoke {
        method_name: String,
        input: core_wire::SchemaValueTree,
        principal: golem_agent::common::Principal,
        expected_output: Box<ExpectedInvokeOutput>,
    },
    SaveSnapshot,
    LoadSnapshot {
        snapshot: golem_api_1_x::host::Snapshot,
    },
    ProcessOplogEntries {
        account_info: oplog_processor_exports::AccountInfo,
        config: Vec<(String, String)>,
        component_id: core_wire::ComponentId,
        agent_id: core_wire::AgentId,
        metadata: golem_api_1_x::host::AgentMetadata,
        first_entry_index: u64,
        entries: Vec<golem_api_1_x::oplog::OplogEntry>,
    },
}

impl PreparedCall {
    fn principal(&self) -> Option<golem_common::model::agent::Principal> {
        match self {
            Self::Initialize { principal, .. } | Self::Invoke { principal, .. } => {
                Some(principal.clone().into())
            }
            Self::SaveSnapshot | Self::LoadSnapshot { .. } | Self::ProcessOplogEntries { .. } => {
                None
            }
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
            principal,
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
                principal,
            }
        }
        LoweredCall::Invoke {
            method_name,
            input,
            principal,
            expected_output,
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
                principal,
                expected_output,
            }
        }
        LoweredCall::SaveSnapshot => PreparedCall::SaveSnapshot,
        LoweredCall::LoadSnapshot { snapshot } => PreparedCall::LoadSnapshot { snapshot },
        LoweredCall::ProcessOplogEntries {
            account_info,
            config,
            component_id,
            agent_id,
            metadata,
            first_entry_index,
            entries,
        } => PreparedCall::ProcessOplogEntries {
            account_info,
            config,
            component_id,
            agent_id,
            metadata,
            first_entry_index,
            entries,
        },
    })
}

pub fn lower_invocation(
    invocation: AgentInvocation,
    component_metadata: &ComponentMetadata,
    agent_id: Option<&ParsedAgentId>,
) -> Result<LoweredInvocation, WorkerExecutorError> {
    match invocation {
        AgentInvocation::AgentInitialization {
            input, principal, ..
        } => {
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
                    principal: principal.into(),
                },
            })
        }
        AgentInvocation::AgentMethod {
            method_name,
            input,
            principal,
            ..
        } => {
            let agent_type = resolve_agent_type(component_metadata, agent_id)?;
            // The method is resolved only to classify read-only methods; the
            // input carrier is already the schema-native parameter record.
            // Encoding to the wire tree (which may mint owned capability
            // handles) is deferred to `materialize_call`.
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

            let read_only_method = method.read_only.is_some().then(|| method_name.clone());
            validate_method_invocation(agent_type, method, &input, &method_name)?;

            let expected_output = Box::new(ExpectedInvokeOutput {
                graph: agent_type.schema.clone(),
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
                    principal: principal.into(),
                    expected_output,
                },
            })
        }
        AgentInvocation::ManualUpdate { .. } => Err(WorkerExecutorError::invalid_request(
            "ManualUpdate should not be invoked as a wasm function directly".to_string(),
        )),
        AgentInvocation::SaveSnapshot { .. } => Ok(LoweredInvocation {
            display_name: "save-snapshot".to_string(),
            read_only_method: None,
            call: LoweredCall::SaveSnapshot,
        }),
        AgentInvocation::LoadSnapshot { snapshot, .. } => Ok(LoweredInvocation {
            display_name: "load-snapshot".to_string(),
            read_only_method: None,
            call: LoweredCall::LoadSnapshot {
                snapshot: snapshot.into(),
            },
        }),
        AgentInvocation::ProcessOplogEntries {
            account_id,
            config,
            metadata,
            first_entry_index,
            entries,
            ..
        } => {
            let component_id: core_wire::ComponentId = metadata.agent_id.component_id.into();
            let agent_id: core_wire::AgentId = metadata.agent_id.clone().into();
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
                    component_id,
                    agent_id,
                    metadata,
                    first_entry_index: u64::from(first_entry_index),
                    entries,
                },
            })
        }
    }
}

pub fn validate_agent_method_invocation(
    component_metadata: &ComponentMetadata,
    agent_id: Option<&ParsedAgentId>,
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

    validate_method_invocation(agent_type, method, input, method_name)
}

pub fn method_uses_streams(
    agent_type: &AgentTypeSchema,
    method: &AgentMethodSchema,
    input: &SchemaValue,
) -> bool {
    contains_stream(input) || method.uses_streams(&agent_type.schema)
}

pub fn validate_method_invocation(
    agent_type: &AgentTypeSchema,
    method: &AgentMethodSchema,
    input: &SchemaValue,
    method_name: &str,
) -> Result<bool, WorkerExecutorError> {
    method
        .validate_input(&agent_type.schema, input)
        .map_err(|error| {
            WorkerExecutorError::invalid_request(format!(
                "Method '{method_name}': invalid input parameter value: {error}"
            ))
        })?;
    Ok(method_uses_streams(agent_type, method, input))
}

/// Resolves the [`AgentTypeSchema`] an invocation targets: by name when an agent id
/// is available, otherwise the single declared agent type (or an error when the
/// component declares zero or multiple types and no id was provided).
fn resolve_agent_type<'a>(
    component_metadata: &'a ComponentMetadata,
    agent_id: Option<&ParsedAgentId>,
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
    use golem_common::base_model::Empty;
    use golem_common::base_model::agent::Snapshotting;
    use golem_common::base_model::component_metadata::KnownExports;
    use golem_common::model::IdempotencyKey;
    use golem_common::model::agent::{AgentTypeName, Principal};
    use golem_common::model::invocation_context::InvocationContextStack;
    use golem_common::schema::TypedSchemaValue;
    use golem_common::schema::agent::{
        AgentConstructorSchema, AgentMethodSchema, NamedField, OutputSchema,
    };
    use golem_common::schema::graph::SchemaGraph;
    use golem_common::schema::schema_type::SchemaType;
    use std::collections::BTreeMap;
    use test_r::test;

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
    async fn live_streaming_response_is_published_exactly_once() {
        let value = SchemaValue::U64(42);
        assert_eq!(
            reject_stream_at_materializing_boundary(value.clone()).unwrap(),
            value
        );
    }

    /// Component metadata with one agent type whose `do-work` method takes two
    /// user-supplied parameters (`count: u32`, `label: string`) plus an
    /// auto-injected `principal` field.
    fn metadata() -> ComponentMetadata {
        let method = AgentMethodSchema {
            name: METHOD_NAME.to_string(),
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(vec![
                NamedField::user_supplied("count", SchemaType::u32()),
                NamedField::user_supplied("label", SchemaType::string()),
                NamedField::auto_injected(
                    "principal",
                    golem_common::schema::agent::AutoInjectedKind::Principal,
                    SchemaType::string(),
                ),
            ]),
            output_schema: OutputSchema::Unit,
            http_endpoint: Vec::new(),
            read_only: None,
        };
        metadata_with_method(method)
    }

    fn metadata_with_method(method: AgentMethodSchema) -> ComponentMetadata {
        let at = AgentTypeSchema {
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
            None,
            None,
            vec![at],
            BTreeMap::new(),
        )
    }

    fn agent_id() -> ParsedAgentId {
        let parameters = TypedSchemaValue::new(
            SchemaGraph::anonymous(SchemaType::record(Vec::new())),
            SchemaValue::Record { fields: Vec::new() },
        );
        ParsedAgentId::new(AgentTypeName(AGENT_TYPE.to_string()), parameters, None)
    }

    fn method_invocation(input: SchemaValue) -> AgentInvocation {
        AgentInvocation::AgentMethod {
            idempotency_key: IdempotencyKey::new("k".to_string()),
            method_name: METHOD_NAME.to_string(),
            input,
            invocation_context: InvocationContextStack::fresh(),
            principal: Principal::anonymous(),
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
        let lowered = lower_invocation(method_invocation(input), &metadata, Some(&agent_id))
            .expect("valid input should lower");
        assert_eq!(lowered.display_name, METHOD_NAME);
        assert!(lowered.read_only_method.is_none());
    }

    #[test]
    fn method_with_non_record_input_is_rejected() {
        let metadata = metadata();
        let agent_id = agent_id();
        let Err(err) = lower_invocation(
            method_invocation(SchemaValue::U32(1)),
            &metadata,
            Some(&agent_id),
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
        let Err(err) = lower_invocation(method_invocation(input), &metadata, Some(&agent_id))
        else {
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
        let Err(err) = lower_invocation(method_invocation(input), &metadata, Some(&agent_id))
        else {
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
            Some(&agent_id()),
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
        };
        validate_invoke_output(METHOD_NAME, &expected, &SchemaValue::U32(42))
            .expect("matching value must pass output validation");
    }

    #[test]
    fn declared_output_rejects_mismatched_value() {
        let expected = ExpectedInvokeOutput {
            graph: SchemaGraph::empty(),
            root: SchemaType::u32(),
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
    fn guest_trap_and_settlement_failures_keep_distinct_provenance() {
        let guest_trap = classify_guest_call_settlement(
            Ok::<_, wasmtime::Error>(Err::<(), _>(wasmtime::Error::msg("guest trapped"))),
            None,
            false,
        )
        .expect("the event loop itself settled");
        assert!(guest_trap.is_err());

        assert!(matches!(
            classify_guest_call_settlement::<()>(
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
                Err(wasmtime::Error::msg("host task failed")),
                None,
                false,
            ),
            Err(GuestCallSettlementError::Infrastructure(_))
        ));
        assert!(matches!(
            classify_guest_call_settlement::<()>(
                Err(wasmtime::Error::msg("interrupted")),
                None,
                true,
            ),
            Err(GuestCallSettlementError::Interrupted(_))
        ));
        assert!(matches!(
            classify_guest_call_settlement::<()>(
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
        let mut linker = wasmtime::Linker::new(&engine);
        linker
            .func_wrap("host", "fail", || -> wasmtime::Result<()> {
                Err(wasmtime::Error::from_anyhow(
                    WorkerExecutorError::runtime("registry unavailable").into(),
                ))
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
        assert!(matches!(
            classify_guest_call_settlement::<()>(Err(error), None, false),
            Err(GuestCallSettlementError::Infrastructure(_))
        ));
    }

    #[test]
    fn settlement_timeout_is_infrastructure_even_after_root_return() {
        let result = classify_guest_call_settlement(
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
