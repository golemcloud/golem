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

use crate::metrics::wasm::{record_invocation, record_invocation_consumption};
use crate::model::TrapType;
use crate::preview2::exports::golem::agent::guest as guest_exports;
use crate::preview2::exports::golem::api1_5_0::load_snapshot as load_snapshot_exports;
use crate::preview2::exports::golem::api1_5_0::save_snapshot as save_snapshot_exports;
use crate::preview2::oplog_processor_plugin::exports::golem::api1_5_0::oplog_processor as oplog_processor_exports;
use crate::preview2::{golem_agent, golem_api_1_x};
use crate::workerctx::{PublicWorkerIo, WorkerCtx};
use futures::FutureExt;
use golem_common::model::agent::{
    AgentMode, AgentType, DataSchema, LegacyParsedAgentId, UntypedDataValue,
};
use golem_common::model::component_metadata::ComponentMetadata;
use golem_common::model::oplog::AgentError as OplogAgentError;
use golem_common::model::{AgentInvocation, AgentInvocationResult, OplogIndex};
use golem_common::schema::adapters::untyped::{
    typed_output_value_to_untyped_data_value, untyped_data_value_to_input_value,
};
use golem_common::schema::agent::wit::decode_agent_error;
use golem_common::schema::wit::wire as core_wire;
use golem_common::schema::wit::{decode_value, encode_value};
use golem_service_base::error::worker_executor::{InterruptKind, WorkerExecutorError};
use tracing::{Instrument, Level, debug, span};
use wasmtime::{AsContextMut, StoreContextMut};

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

    if let InvocationMode::Live(invocation) = mode {
        async {
            store
                .data_mut()
                .on_agent_invocation_started(invocation)
                .await
        }
        .instrument(span!(Level::INFO, "on_agent_invocation_started"))
        .await?;
    }

    store.data_mut().set_running();

    // If the invocation targets a read-only AgentMethod, enable the read-only invocation
    // strictness for the duration of the call. We restore the mode on every exit path:
    // normal `Ok` / `Err` returns from the wasmtime call site as well as panics that
    // unwind through the call. This is the only place where strictness is enabled.
    if let Some(method_name) = &read_only_method {
        store.data_mut().enter_read_only_mode(method_name.clone());
    }

    let call_future = dispatch_call(&mut store, instance, call, &display_name);

    let call_outcome = std::panic::AssertUnwindSafe(call_future)
        .catch_unwind()
        .await;

    if read_only_method.is_some() {
        store.data_mut().exit_read_only_mode();
    }

    let call_result = match call_outcome {
        Ok(result) => result,
        Err(payload) => std::panic::resume_unwind(payload),
    };

    store.data().set_suspended();

    call_result
}

/// Dispatches a single lowered invocation to the matching typed guest export
/// accessor (`golem:agent/guest@2.0.0`, `golem:api/save-snapshot`,
/// `golem:api/load-snapshot`, or `golem:api/oplog-processor`) and maps its
/// typed result into an [`InvokeResult`].
async fn dispatch_call<Ctx: WorkerCtx>(
    store: &mut StoreContextMut<'_, Ctx>,
    instance: &wasmtime::component::Instance,
    call: LoweredCall,
    display_name: &str,
) -> Result<InvokeResult, WorkerExecutorError> {
    match call {
        LoweredCall::Initialize {
            agent_type,
            input,
            principal,
        } => {
            let guest = load_agent_guest(store, instance)?;
            prepare_guest_call(store, display_name).await;
            let result = guest
                .call_initialize(&mut *store, &agent_type, &input, &principal)
                .await;
            let consumed_fuel =
                finish_invocation_and_get_fuel_consumption(store, display_name).await?;
            match result {
                Ok(Ok(())) => Ok(InvokeResult::Succeeded {
                    consumed_fuel,
                    result: AgentInvocationResult::AgentInitialization,
                }),
                Ok(Err(wire_err)) => invoke_result_from_agent_error(consumed_fuel, wire_err),
                Err(err) => Ok(invoke_result_from_trap::<Ctx>(store, consumed_fuel, err).await),
            }
        }
        LoweredCall::Invoke {
            method_name,
            input,
            principal,
            output_schema,
        } => {
            let guest = load_agent_guest(store, instance)?;
            prepare_guest_call(store, display_name).await;
            let result = guest
                .call_invoke(&mut *store, &method_name, &input, &principal)
                .await;
            let consumed_fuel =
                finish_invocation_and_get_fuel_consumption(store, display_name).await?;
            match result {
                Ok(Ok(maybe_output)) => {
                    let output = decode_invoke_output(maybe_output, &output_schema)?;
                    Ok(InvokeResult::Succeeded {
                        consumed_fuel,
                        result: AgentInvocationResult::AgentMethod { output },
                    })
                }
                Ok(Err(wire_err)) => invoke_result_from_agent_error(consumed_fuel, wire_err),
                Err(err) => Ok(invoke_result_from_trap::<Ctx>(store, consumed_fuel, err).await),
            }
        }
        LoweredCall::SaveSnapshot => {
            let guest = load_save_snapshot_guest(store, instance)?;
            prepare_guest_call(store, display_name).await;
            let result = guest.call_save(&mut *store).await;
            let consumed_fuel =
                finish_invocation_and_get_fuel_consumption(store, display_name).await?;
            match result {
                Ok(snapshot) => Ok(InvokeResult::Succeeded {
                    consumed_fuel,
                    result: AgentInvocationResult::SaveSnapshot {
                        snapshot: snapshot.into(),
                    },
                }),
                Err(err) => Ok(invoke_result_from_trap::<Ctx>(store, consumed_fuel, err).await),
            }
        }
        LoweredCall::LoadSnapshot { snapshot } => {
            let guest = load_load_snapshot_guest(store, instance)?;
            prepare_guest_call(store, display_name).await;
            let result = guest.call_load(&mut *store, &snapshot).await;
            let consumed_fuel =
                finish_invocation_and_get_fuel_consumption(store, display_name).await?;
            match result {
                Ok(inner) => Ok(InvokeResult::Succeeded {
                    consumed_fuel,
                    result: AgentInvocationResult::LoadSnapshot { error: inner.err() },
                }),
                Err(err) => Ok(invoke_result_from_trap::<Ctx>(store, consumed_fuel, err).await),
            }
        }
        LoweredCall::ProcessOplogEntries {
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
            let result = guest
                .call_process(
                    &mut *store,
                    account_info,
                    &config,
                    component_id,
                    &agent_id,
                    &metadata,
                    first_entry_index,
                    &entries,
                )
                .await;
            let consumed_fuel =
                finish_invocation_and_get_fuel_consumption(store, display_name).await?;
            match result {
                Ok(inner) => Ok(InvokeResult::Succeeded {
                    consumed_fuel,
                    result: AgentInvocationResult::ProcessOplogEntries { error: inner.err() },
                }),
                Err(err) => Ok(invoke_result_from_trap::<Ctx>(store, consumed_fuel, err).await),
            }
        }
    }
}

/// Resets call counters and emits the invocation-start event before a guest
/// call. Mirrors the bookkeeping the legacy dynamic dispatch performed.
async fn prepare_guest_call<Ctx: WorkerCtx>(
    store: &mut StoreContextMut<'_, Ctx>,
    display_name: &str,
) {
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

/// Builds an [`InvokeResult`] from a wasmtime trap (guest panic, interrupt,
/// exit, or runtime error) raised by a typed export call.
async fn invoke_result_from_trap<Ctx: WorkerCtx>(
    store: &mut StoreContextMut<'_, Ctx>,
    consumed_fuel: u64,
    err: wasmtime::Error,
) -> InvokeResult {
    let retry_from = store.data().get_current_retry_point().await;
    let agent_mode = store.data().agent_mode();
    let err: anyhow::Error = err.into();
    InvokeResult::from_error::<Ctx>(consumed_fuel, &err, retry_from, agent_mode)
}

/// Maps a guest-returned `agent-error` (the `Err` arm of `initialize` /
/// `invoke`) into a failed [`InvokeResult`].
fn invoke_result_from_agent_error(
    consumed_fuel: u64,
    wire_err: golem_agent::common::AgentError,
) -> Result<InvokeResult, WorkerExecutorError> {
    let agent_error = decode_agent_error(wire_err).map_err(|e| {
        WorkerExecutorError::runtime(format!("Failed to decode agent-error from guest: {e}"))
    })?;
    Ok(InvokeResult::Failed {
        consumed_fuel,
        error: OplogAgentError::InternalError(agent_error.to_string()),
        retry_from: OplogIndex::INITIAL,
        semantic_trap_retry_override: None,
    })
}

/// Decodes the `option<schema-value-tree>` output of `invoke` into the legacy
/// [`UntypedDataValue`] consumed by the (later-wave) gRPC / oplog boundary.
fn decode_invoke_output(
    maybe_output: Option<core_wire::SchemaValueTree>,
    output_schema: &DataSchema,
) -> Result<UntypedDataValue, WorkerExecutorError> {
    match maybe_output {
        // `none` is the declared `unit` output.
        None => Ok(UntypedDataValue::Tuple(Vec::new())),
        Some(tree) => {
            let value = decode_value(&tree).map_err(|e| {
                WorkerExecutorError::runtime(format!("Failed to decode agent method output: {e}"))
            })?;
            typed_output_value_to_untyped_data_value(value, output_schema).map_err(|e| {
                WorkerExecutorError::runtime(format!("Failed to convert agent method output: {e}"))
            })
        }
    }
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

async fn finish_invocation_and_get_fuel_consumption<Ctx: WorkerCtx>(
    store: &mut StoreContextMut<'_, Ctx>,
    display_name: &str,
) -> Result<u64, WorkerExecutorError> {
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
    pub fn from_error<Ctx: WorkerCtx>(
        consumed_fuel: u64,
        error: &anyhow::Error,
        retry_from: OplogIndex,
        agent_mode: AgentMode,
    ) -> Self {
        match TrapType::from_error::<Ctx>(error, retry_from, agent_mode) {
            TrapType::Interrupt(kind) => InvokeResult::Interrupted {
                consumed_fuel,
                interrupt_kind: kind,
            },
            TrapType::Exit => InvokeResult::Exited { consumed_fuel },
            TrapType::Error {
                error,
                retry_from,
                semantic_trap_retry_override,
            } => InvokeResult::Failed {
                consumed_fuel,
                error,
                retry_from,
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
                semantic_trap_retry_override,
                ..
            } => Some(TrapType::Error {
                error: error.clone(),
                retry_from: *retry_from,
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

/// The typed guest-export call an [`AgentInvocation`] lowers to, carrying the
/// schema-native wire arguments needed by the `bindgen!`-generated accessors.
enum LoweredCall {
    Initialize {
        agent_type: String,
        input: core_wire::SchemaValueTree,
        principal: golem_agent::common::Principal,
    },
    Invoke {
        method_name: String,
        input: core_wire::SchemaValueTree,
        principal: golem_agent::common::Principal,
        output_schema: DataSchema,
    },
    SaveSnapshot,
    LoadSnapshot {
        snapshot: golem_api_1_x::host::Snapshot,
    },
    ProcessOplogEntries {
        account_info: oplog_processor_exports::AccountInfo,
        config: Vec<(String, String)>,
        component_id: golem_wasm::ComponentId,
        agent_id: golem_wasm::AgentId,
        metadata: golem_api_1_x::host::AgentMetadata,
        first_entry_index: u64,
        entries: Vec<golem_api_1_x::oplog::OplogEntry>,
    },
}

pub fn lower_invocation(
    invocation: AgentInvocation,
    component_metadata: &ComponentMetadata,
    agent_id: Option<&LegacyParsedAgentId>,
) -> Result<LoweredInvocation, WorkerExecutorError> {
    match invocation {
        AgentInvocation::AgentInitialization {
            input, principal, ..
        } => {
            let agent_type = resolve_agent_type(component_metadata, agent_id)?;
            let input_value =
                untyped_data_value_to_input_value(input, &agent_type.constructor.input_schema)
                    .map_err(|e| {
                        WorkerExecutorError::invalid_request(format!(
                            "Invalid initialize input for agent type '{}': {e}",
                            agent_type.type_name
                        ))
                    })?;
            Ok(LoweredInvocation {
                display_name: "initialize".to_string(),
                read_only_method: None,
                call: LoweredCall::Initialize {
                    agent_type: agent_type.type_name.to_string(),
                    input: encode_value(&input_value),
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
            // `agent_type` is owned, so consume `methods` and move the matched
            // method's `output_schema` into the lowered call rather than cloning
            // it on every invocation.
            let method = agent_type
                .methods
                .into_iter()
                .find(|m| m.name == method_name)
                .ok_or_else(|| {
                    WorkerExecutorError::invalid_request(format!(
                        "Agent method '{method_name}' not found in agent type '{}'",
                        agent_type.type_name
                    ))
                })?;

            let read_only_method = method.read_only.is_some().then(|| method_name.clone());

            let input_value =
                untyped_data_value_to_input_value(input, &method.input_schema).map_err(|e| {
                    WorkerExecutorError::invalid_request(format!(
                        "Invalid input for method '{method_name}': {e}"
                    ))
                })?;

            Ok(LoweredInvocation {
                display_name: method_name.clone(),
                read_only_method,
                call: LoweredCall::Invoke {
                    method_name,
                    input: encode_value(&input_value),
                    principal: principal.into(),
                    output_schema: method.output_schema,
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
            let component_id: golem_wasm::ComponentId =
                metadata.agent_id.component_id.clone().into();
            let agent_id: golem_wasm::AgentId = metadata.agent_id.clone().into();
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

/// Resolves the [`AgentType`] an invocation targets: by name when an agent id
/// is available, otherwise the single declared agent type (or an error when the
/// component declares zero or multiple types and no id was provided).
fn resolve_agent_type(
    component_metadata: &ComponentMetadata,
    agent_id: Option<&LegacyParsedAgentId>,
) -> Result<AgentType, WorkerExecutorError> {
    match agent_id {
        Some(id) => component_metadata
            .find_agent_type_by_name(&id.agent_type)
            .ok_or_else(|| {
                WorkerExecutorError::invalid_request(format!(
                    "Agent type '{}' not found in component",
                    id.agent_type
                ))
            }),
        None => match component_metadata.agent_types() {
            [single] => Ok(single.clone()),
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
