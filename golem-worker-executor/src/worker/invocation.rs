// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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
use crate::virtual_export_compat;
use crate::workerctx::{PublicWorkerIo, WorkerCtx};
use anyhow::anyhow;
use golem_common::model::agent::AgentId;
use golem_common::model::agent::UntypedDataValue;
use golem_common::model::component_metadata::{ComponentMetadata, InvokableFunction};
use golem_common::model::oplog::RawSnapshotData;
use golem_common::model::oplog::WorkerError;
use golem_common::model::parsed_function_name::{ParsedFunctionName, ParsedFunctionReference};
use golem_common::model::{AgentInvocation, AgentInvocationResult, OplogIndex};
use golem_common::virtual_exports;
use golem_service_base::error::worker_executor::{InterruptKind, WorkerExecutorError};
use golem_wasm::wasmtime::{DecodeParamResult, decode_param, encode_output};
use golem_wasm::{FromValue, IntoValue, Value};
use tracing::{Instrument, debug, error};
use wasmtime::component::{Func, Val};
use wasmtime::{AsContext, AsContextMut, StoreContextMut};
use wasmtime_wasi_http::WasiHttpView;
use wasmtime_wasi_http::bindings::Proxy;

/// Invokes a function on a worker.
///
/// The context is held until the invocation finishes
///
/// Arguments:
/// - `invocation`: the agent invocation describing what to invoke
/// - `store`: reference to the wasmtime instance's store
/// - `instance`: reference to the wasmtime instance
pub async fn invoke_observed_and_traced<Ctx: WorkerCtx>(
    invocation: &AgentInvocation,
    store: &mut impl AsContextMut<Data = Ctx>,
    instance: &wasmtime::component::Instance,
    component_metadata: &ComponentMetadata,
    is_live: bool,
) -> Result<InvokeResult, WorkerExecutorError> {
    let mut store = store.as_context_mut();
    let was_live_before = store.data().is_live();

    debug!("Beginning invocation {invocation:?}");

    let result = invoke_observed(
        invocation,
        &mut store,
        instance,
        component_metadata,
        is_live,
    )
    .await;

    debug!("Invocation resulted in {:?}", result);

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

fn find_function<'a, Ctx: WorkerCtx>(
    mut store: &mut StoreContextMut<'a, Ctx>,
    instance: &'a wasmtime::component::Instance,
    parsed_function_name: &ParsedFunctionName,
) -> Result<FindFunctionResult, WorkerExecutorError> {
    if *parsed_function_name == *virtual_exports::http_incoming_handler::PARSED_FUNCTION_NAME {
        return Ok(FindFunctionResult::IncomingHttpHandlerBridge);
    };

    let parsed_function_ref = parsed_function_name.function();

    if matches!(
        parsed_function_ref,
        ParsedFunctionReference::RawResourceDrop { .. }
    ) {
        return Ok(FindFunctionResult::ResourceDrop);
    }

    match &parsed_function_name.site().interface_name() {
        Some(interface_name) => {
            let (_, exported_instance_idx) = instance
                .get_export(&mut store, None, interface_name)
                .ok_or(WorkerExecutorError::invalid_request(format!(
                    "could not load exports for interface {interface_name}"
                )))?;

            let func = instance
                .get_export(
                    &mut store,
                    Some(&exported_instance_idx),
                    &parsed_function_name.function().function_name(),
                )
                .and_then(|(_, idx)| instance.get_func(&mut store, idx));

            match func {
                Some(func) => Ok(FindFunctionResult::ExportedFunction(func)),
                None => match parsed_function_name.method_as_static() {
                    None => Err(WorkerExecutorError::invalid_request(format!(
                        "could not load function {} for interface {}",
                        &parsed_function_name.function().function_name(),
                        interface_name
                    ))),
                    Some(parsed_static) => instance
                        .get_export(
                            &mut store,
                            Some(&exported_instance_idx),
                            &parsed_static.function().function_name(),
                        )
                        .and_then(|(_, idx)| instance.get_func(&mut store, idx))
                        .ok_or(WorkerExecutorError::invalid_request(format!(
                            "could not load function {} or {} for interface {}",
                            &parsed_function_name.function().function_name(),
                            &parsed_static.function().function_name(),
                            interface_name
                        )))
                        .map(FindFunctionResult::ExportedFunction),
                },
            }
        }
        None => instance
            .get_func(store, parsed_function_name.function().function_name())
            .ok_or(WorkerExecutorError::invalid_request(format!(
                "could not load function {}",
                &parsed_function_name.function().function_name()
            )))
            .map(FindFunctionResult::ExportedFunction),
    }
}

/// Invokes a worker and calls the appropriate hooks to observe the invocation
async fn invoke_observed<Ctx: WorkerCtx>(
    invocation: &AgentInvocation,
    store: &mut impl AsContextMut<Data = Ctx>,
    instance: &wasmtime::component::Instance,
    component_metadata: &ComponentMetadata,
    is_live: bool,
) -> Result<InvokeResult, WorkerExecutorError> {
    let mut store = store.as_context_mut();

    let (full_function_name, function_input) =
        extract_function_name_and_input(invocation, component_metadata)?;

    let parsed = ParsedFunctionName::parse(&full_function_name).map_err(|err| {
        WorkerExecutorError::invalid_request(format!(
            "Invalid function name {full_function_name}: {err}"
        ))
    })?;

    let function = find_function(&mut store, instance, &parsed)?;

    let decoded_params =
        validate_function_parameters(&mut store, &function, &full_function_name, &function_input)
            .await?;

    if is_live {
        store
            .data_mut()
            .on_agent_invocation_started(&full_function_name, &function_input)
            .await?;
    }

    store.data_mut().set_running();

    let metadata = component_metadata
        .find_parsed_function(&parsed)
        .map_err(WorkerExecutorError::runtime)?
        .ok_or_else(|| {
            WorkerExecutorError::invalid_request(format!(
                "Could not find exported function: {parsed}"
            ))
        })?;

    verify_agent_invocation(store.data().agent_id(), &metadata)?;

    let call_result = match function {
        FindFunctionResult::ExportedFunction(function) => {
            invoke(
                &mut store,
                function,
                decoded_params,
                &full_function_name,
                &metadata,
                invocation,
            )
            .await
        }
        FindFunctionResult::ResourceDrop => {
            // Special function: drop
            drop_resource(&mut store, &function_input, &full_function_name, invocation).await
        }
        FindFunctionResult::IncomingHttpHandlerBridge => {
            invoke_http_handler(
                &mut store,
                instance,
                &function_input,
                &full_function_name,
                invocation,
            )
            .await
        }
    };

    store.data().set_suspended();

    call_result
}

fn verify_agent_invocation(
    agent_id: Option<AgentId>,
    invocation: &InvokableFunction,
) -> Result<(), WorkerExecutorError> {
    if let Some(agent_id) = agent_id {
        if invocation.agent_method_or_constructor.is_some() {
            if let Some(interface_name) = invocation.name.site.interface_name() {
                // interface_name is the kebab-cased agent type name from the static wrapper
                let agent_type = agent_id.wrapper_agent_type();
                if interface_name != agent_type {
                    Err(WorkerExecutorError::invalid_request(format!(
                        "Attempt to call a different agent type's method on an agent; targeted agent has type {agent_type}, the invocation is targeting {interface_name}"
                    )))
                } else {
                    // matching names
                    Ok(())
                }
            } else {
                // Unexpected state - should never reach this
                Ok(())
            }
        } else {
            // Not an agent invocation (deprecated)
            Ok(())
        }
    } else {
        // Not an agent (deprecated)
        Ok(())
    }
}

async fn validate_function_parameters(
    store: &mut impl AsContextMut<Data = impl WorkerCtx>,
    function: &FindFunctionResult,
    raw_function_name: &str,
    function_input: &[Value],
) -> Result<Vec<DecodeParamResult>, WorkerExecutorError> {
    match function {
        FindFunctionResult::ExportedFunction(func) => {
            let mut store = store.as_context_mut();
            let param_types: Vec<_> = {
                let params = func.params(&store);
                params.to_vec()
            };

            if function_input.len() != param_types.len() {
                return Err(WorkerExecutorError::ParamTypeMismatch {
                    details: format!(
                        "expected {}, got {} parameters",
                        param_types.len(),
                        function_input.len()
                    ),
                });
            }

            let mut results = Vec::new();
            for (param, (_, param_type)) in function_input.iter().zip(param_types.iter()) {
                let decoded = decode_param(param, param_type, store.data_mut())
                    .await
                    .map_err(WorkerExecutorError::from)?;
                results.push(decoded);
            }
            Ok(results)
        }
        FindFunctionResult::ResourceDrop => {
            if function_input.len() != 1 {
                return Err(WorkerExecutorError::ValueMismatch {
                    details: "unexpected parameter count for drop".to_string(),
                });
            }

            let store = store.as_context_mut();
            let self_uri = store.data().self_uri();

            match function_input.first() {
                Some(Value::Handle { uri, resource_id }) => {
                    if uri == &self_uri.value {
                        Ok(*resource_id)
                    } else {
                        Err(WorkerExecutorError::ValueMismatch {
                            details: format!(
                                "trying to drop handle for on wrong worker ({} vs {}) {}",
                                uri, self_uri.value, raw_function_name
                            ),
                        })
                    }
                }
                _ => Err(WorkerExecutorError::ValueMismatch {
                    details: format!("unexpected function input for drop for {raw_function_name}"),
                }),
            }?;

            Ok(vec![])
        }
        FindFunctionResult::IncomingHttpHandlerBridge => Ok(vec![]),
    }
}

async fn invoke<Ctx: WorkerCtx>(
    store: &mut impl AsContextMut<Data = Ctx>,
    function: Func,
    decoded_function_input: Vec<DecodeParamResult>,
    raw_function_name: &str,
    metadata: &InvokableFunction,
    invocation: &AgentInvocation,
) -> Result<InvokeResult, WorkerExecutorError> {
    let mut store = store.as_context_mut();

    let mut params = Vec::new();
    let mut resources_to_drop = Vec::new();
    for result in decoded_function_input {
        params.push(result.val);
        resources_to_drop.extend(result.resources_to_drop);
    }

    let (results, consumed_fuel) =
        call_exported_function(&mut store, function, params, raw_function_name).await?;

    for resource in resources_to_drop {
        debug!("Dropping passed owned resources {:?}", resource);
        resource.resource_drop_async(&mut store).await?;
    }

    match results {
        Ok(results) => {
            let types = function.results(&store);

            if results.len() > 1 {
                Err(WorkerExecutorError::runtime(
                    "Function returned with more than one values, which is not supported",
                ))
            } else {
                let output = match results
                    .iter()
                    .zip(types.iter())
                    .zip(metadata.analysed_export.result.as_ref().map(|r| &r.typ))
                    .next()
                {
                    Some(((val, typ), analysed_type)) => Some(
                        encode_output(val, typ, analysed_type, store.data_mut())
                            .await
                            .map_err(WorkerExecutorError::from)?,
                    ),
                    None => None,
                };
                let result = wrap_output_as_agent_result(invocation, output)?;
                Ok(InvokeResult::Succeeded {
                    consumed_fuel,
                    result,
                })
            }
        }
        Err(err) => {
            let retry_from = store.data().get_current_retry_point().await;
            Ok(InvokeResult::from_error::<Ctx>(
                consumed_fuel,
                &err,
                retry_from,
            ))
        }
    }
}

async fn invoke_http_handler<Ctx: WorkerCtx>(
    store: &mut impl AsContextMut<Data = Ctx>,
    instance: &wasmtime::component::Instance,
    function_input: &[Value],
    raw_function_name: &str,
    invocation: &AgentInvocation,
) -> Result<InvokeResult, WorkerExecutorError> {
    let (sender, receiver) = tokio::sync::oneshot::channel();

    let proxy = Proxy::new(&mut *store, instance)?;
    let mut store_context = store.as_context_mut();

    let idempotency_key = store_context.data().get_current_idempotency_key().await;
    if let Some(idempotency_key) = &idempotency_key {
        store_context
            .data()
            .get_public_state()
            .event_service()
            .emit_invocation_start(
                raw_function_name,
                idempotency_key,
                store_context.data().is_live(),
            );
    }

    debug!("Invoking wasi:http/incoming-http-handler handle");

    let (_, mut task_exits) = {
        let (scheme, hyper_request) =
            virtual_export_compat::http_incoming_handler::input_to_hyper_request(function_input)?;
        let incoming = store_context
            .data_mut()
            .as_wasi_http_view()
            .new_incoming_request(scheme, hyper_request)?;
        let outgoing = store_context
            .data_mut()
            .as_wasi_http_view()
            .new_response_outparam(sender)?;

        // unsafety comes from scope_and_collect:
        //
        // This function is not completely safe:
        // please see cancellation_soundness in [tests.rs](https://github.com/rmanoka/async-scoped/blob/master/src/tests.rs) for a test-case that suggests how this can lead to invalid memory access if not dealt with care.
        // The caller must ensure that the lifetime ’a is valid until the returned future is fully driven. Dropping the future is okay, but blocks the current thread until all spawned futures complete.
        unsafe {
            async_scoped::TokioScope::scope_and_collect(|s| {
                s.spawn(
                    proxy
                        .wasi_http_incoming_handler()
                        .call_handle(store_context, incoming, outgoing)
                        .in_current_span(),
                );
            })
            .await
        }
    };

    let out = receiver.await;

    let res_or_error = match out {
        Ok(Ok(resp)) => {
            Ok(virtual_export_compat::http_incoming_handler::http_response_to_output(resp).await?)
        }
        Ok(Err(e)) => Err(anyhow::Error::from(e)),
        Err(_) => {
            // An error in the receiver (`RecvError`) only indicates that the
            // task exited before a response was sent (i.e., the sender was
            // dropped); it does not describe the underlying cause of failure.
            // Instead, we retrieve and propagate the error from inside the task
            // which should more clearly tell the user what went wrong. Note
            // that we assume the task has already exited at this point, so the
            // `await` should be resolved immediately.
            let task_exit = task_exits.remove(0);
            let e = match task_exit {
                Ok(r) => r.expect_err("if the receiver has an error, the task must have failed"),
                Err(_e) => anyhow!("failed joining wasm task"),
            };
            Err(e)?
        }
    };

    let consumed_fuel =
        finish_invocation_and_get_fuel_consumption(&mut store.as_context_mut(), raw_function_name)
            .await?;

    match res_or_error {
        Ok(resp) => {
            let result = wrap_output_as_agent_result(invocation, Some(resp))?;
            Ok(InvokeResult::Succeeded {
                consumed_fuel,
                result,
            })
        }
        Err(e) => {
            let retry_from = store.as_context().data().get_current_retry_point().await;
            Ok(InvokeResult::from_error::<Ctx>(
                consumed_fuel,
                &e,
                retry_from,
            ))
        }
    }
}

async fn drop_resource<Ctx: WorkerCtx>(
    store: &mut impl AsContextMut<Data = Ctx>,
    function_input: &[Value],
    raw_function_name: &str,
    invocation: &AgentInvocation,
) -> Result<InvokeResult, WorkerExecutorError> {
    let mut store = store.as_context_mut();

    let resource_id = match function_input.first() {
        Some(Value::Handle { resource_id, .. }) => *resource_id,
        _ => unreachable!(), // previously validated by `validate_function_parameters`
    };

    if let Some((_, resource)) = store.data_mut().get(resource_id).await {
        debug!("Dropping resource {resource:?} in {raw_function_name}");

        let result = resource.resource_drop_async(&mut store).await;

        let current_fuel_level = store.get_fuel().unwrap_or(0);
        let consumed_fuel = store.data_mut().return_fuel(current_fuel_level);

        match result {
            Ok(_) => {
                let result = wrap_output_as_agent_result(invocation, None)?;
                Ok(InvokeResult::Succeeded {
                    consumed_fuel,
                    result,
                })
            }
            Err(err) => {
                let retry_from = store.data().get_current_retry_point().await;
                Ok(InvokeResult::from_error::<Ctx>(
                    consumed_fuel,
                    &err,
                    retry_from,
                ))
            }
        }
    } else {
        let result = wrap_output_as_agent_result(invocation, None)?;
        Ok(InvokeResult::Succeeded {
            consumed_fuel: 0,
            result,
        })
    }
}

async fn call_exported_function<Ctx: WorkerCtx>(
    store: &mut impl AsContextMut<Data = Ctx>,
    function: Func,
    params: Vec<Val>,
    raw_function_name: &str,
) -> Result<(anyhow::Result<Vec<Val>>, u64), WorkerExecutorError> {
    let mut store = store.as_context_mut();

    let idempotency_key = store.data().get_current_idempotency_key().await;
    if let Some(idempotency_key) = &idempotency_key {
        store
            .data()
            .get_public_state()
            .event_service()
            .emit_invocation_start(raw_function_name, idempotency_key, store.data().is_live());
    }

    let mut results: Vec<Val> = function
        .results(&store)
        .iter()
        .map(|_| Val::Bool(false))
        .collect();

    let result = function.call_async(&mut store, &params, &mut results).await;
    let result = if result.is_ok() {
        function.post_return_async(&mut store).await.map_err(|e| {
            error!("Error in post_return_async for {raw_function_name}: {}", e);
            e
        })
    } else {
        result
    };

    let consumed_fuel_for_call =
        finish_invocation_and_get_fuel_consumption(&mut store, raw_function_name).await?;

    Ok((result.map(|_| results), consumed_fuel_for_call))
}

async fn finish_invocation_and_get_fuel_consumption<Ctx: WorkerCtx>(
    store: &mut StoreContextMut<'_, Ctx>,
    raw_function_name: &str,
) -> Result<u64, WorkerExecutorError> {
    let current_fuel_level = store.get_fuel().unwrap_or(0);
    let consumed_fuel_for_call = store.data_mut().return_fuel(current_fuel_level);

    if consumed_fuel_for_call > 0 {
        debug!(
            "Fuel consumed for call {raw_function_name}: {}",
            consumed_fuel_for_call
        );
    }

    record_invocation_consumption(consumed_fuel_for_call);

    Ok(consumed_fuel_for_call)
}

#[derive(Clone, Debug)]
pub enum InvokeResult {
    /// The invoked function exited with exit code 0
    Exited { consumed_fuel: u64 },
    /// The invoked function has failed
    Failed {
        consumed_fuel: u64,
        error: WorkerError,
        retry_from: OplogIndex,
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
    ) -> Self {
        match TrapType::from_error::<Ctx>(error, retry_from) {
            TrapType::Interrupt(kind) => InvokeResult::Interrupted {
                consumed_fuel,
                interrupt_kind: kind,
            },
            TrapType::Exit => InvokeResult::Exited { consumed_fuel },
            TrapType::Error { error, retry_from } => InvokeResult::Failed {
                consumed_fuel,
                error,
                retry_from,
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
                error, retry_from, ..
            } => Some(TrapType::Error {
                error: error.clone(),
                retry_from: *retry_from,
            }),
            InvokeResult::Interrupted { interrupt_kind, .. } => {
                Some(TrapType::Interrupt(*interrupt_kind))
            }
            InvokeResult::Exited { .. } => Some(TrapType::Exit),
            _ => None,
        }
    }
}

/// Extracts the low-level wasm function name and input parameters from an `AgentInvocation`.
///
/// This is the single place that maps high-level agent invocations to the raw wasm function
/// names and parameter values needed by the wasmtime runtime.
fn extract_function_name_and_input(
    invocation: &AgentInvocation,
    component_metadata: &ComponentMetadata,
) -> Result<(String, Vec<Value>), WorkerExecutorError> {
    match invocation {
        AgentInvocation::AgentInitialization { input, .. } => {
            let initialize = component_metadata
                .agent_initialize()
                .map_err(WorkerExecutorError::runtime)?
                .ok_or_else(|| {
                    WorkerExecutorError::invalid_request(
                        "agent initialize function not found in component".to_string(),
                    )
                })?;
            Ok((
                initialize.name.to_string(),
                vec![input.clone().into_value()],
            ))
        }
        AgentInvocation::AgentMethod {
            method_name, input, ..
        } => Ok((method_name.clone(), vec![input.clone().into_value()])),
        AgentInvocation::ManualUpdate { .. } => Err(WorkerExecutorError::invalid_request(
            "ManualUpdate should not be invoked as a wasm function directly".to_string(),
        )),
        AgentInvocation::SaveSnapshot { .. } => {
            let save_snapshot = component_metadata
                .save_snapshot()
                .map_err(WorkerExecutorError::runtime)?
                .ok_or_else(|| {
                    WorkerExecutorError::invalid_request(
                        "save-snapshot function not found in component".to_string(),
                    )
                })?;
            Ok((save_snapshot.name.to_string(), vec![]))
        }
        AgentInvocation::LoadSnapshot { snapshot, .. } => {
            let load_snapshot = component_metadata
                .load_snapshot()
                .map_err(WorkerExecutorError::runtime)?
                .ok_or_else(|| {
                    WorkerExecutorError::invalid_request(
                        "load-snapshot function not found in component".to_string(),
                    )
                })?;
            Ok((
                load_snapshot.name.to_string(),
                vec![snapshot.clone().into_value()],
            ))
        }
        AgentInvocation::ProcessOplogEntries { .. } => {
            // ProcessOplogEntries stores Vec<OplogEntry> (internal format), but the WIT
            // oplog-processor.process function expects PublicOplogEntry values.
            // The conversion from OplogEntry to PublicOplogEntry is async and requires
            // services, so it must happen before constructing this AgentInvocation variant.
            // Currently, plugin.rs handles this conversion and invokes the worker directly.
            Err(WorkerExecutorError::invalid_request(
                "ProcessOplogEntries cannot be invoked through the normal invocation pipeline; \
                 entries must be converted to PublicOplogEntry first"
                    .to_string(),
            ))
        }
    }
}

/// Wraps raw wasm output into the appropriate `AgentInvocationResult` variant
/// based on the type of `AgentInvocation`.
fn wrap_output_as_agent_result(
    invocation: &AgentInvocation,
    output: Option<Value>,
) -> Result<AgentInvocationResult, WorkerExecutorError> {
    match invocation {
        AgentInvocation::AgentInitialization { .. } => {
            let data = match output {
                Some(v) => UntypedDataValue::from_value(v).map_err(|e| {
                    WorkerExecutorError::runtime(format!(
                        "Failed to convert agent initialization result: {e}"
                    ))
                })?,
                None => UntypedDataValue::Tuple(vec![]),
            };
            Ok(AgentInvocationResult::AgentInitialization { output: data })
        }
        AgentInvocation::AgentMethod { .. } => {
            let data = match output {
                Some(v) => UntypedDataValue::from_value(v).map_err(|e| {
                    WorkerExecutorError::runtime(format!(
                        "Failed to convert agent method result: {e}"
                    ))
                })?,
                None => UntypedDataValue::Tuple(vec![]),
            };
            Ok(AgentInvocationResult::AgentMethod { output: data })
        }
        AgentInvocation::ManualUpdate { .. } => Ok(AgentInvocationResult::ManualUpdate),
        AgentInvocation::SaveSnapshot { .. } => {
            let snapshot = match output {
                Some(v) => RawSnapshotData::from_value(v).map_err(|e| {
                    WorkerExecutorError::runtime(format!(
                        "Invalid result from save-snapshot function: {e}"
                    ))
                })?,
                None => {
                    return Err(WorkerExecutorError::runtime(
                        "Missing result from save-snapshot function",
                    ));
                }
            };
            Ok(AgentInvocationResult::SaveSnapshot { snapshot })
        }
        AgentInvocation::LoadSnapshot { .. } => {
            let error = decode_result_error(output, "load-snapshot")?;
            Ok(AgentInvocationResult::LoadSnapshot { error })
        }
        AgentInvocation::ProcessOplogEntries { .. } => {
            let error = decode_result_error(output, "process-oplog-entries")?;
            Ok(AgentInvocationResult::ProcessOplogEntries { error })
        }
    }
}

/// Decodes the output of a WIT function that returns `result<_, string>`.
/// Returns `Ok(None)` if the result was Ok, `Ok(Some(error))` if it was Err(string),
/// or `Err` if the output couldn't be decoded.
fn decode_result_error(
    output: Option<Value>,
    function_name: &str,
) -> Result<Option<String>, WorkerExecutorError> {
    match output {
        Some(value) => {
            let result: Result<(), String> = FromValue::from_value(value).map_err(|e| {
                WorkerExecutorError::runtime(format!(
                    "Failed to decode result from {function_name} function: {e}"
                ))
            })?;
            match result {
                Ok(()) => Ok(None),
                Err(error) => Ok(Some(error)),
            }
        }
        None => Err(WorkerExecutorError::runtime(format!(
            "Unexpected empty result from {function_name} function"
        ))),
    }
}

enum FindFunctionResult {
    ExportedFunction(Func),
    ResourceDrop,
    IncomingHttpHandlerBridge,
}
