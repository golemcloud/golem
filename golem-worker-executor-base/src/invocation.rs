// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use anyhow::anyhow;
use golem_common::model::{parse_function_name, CallingConvention, WorkerId, WorkerStatus};
use golem_wasm_rpc::wasmtime::{decode_param, encode_output};
use golem_wasm_rpc::Value;
use tracing::{debug, error, warn};
use wasmtime::component::{Func, Val};
use wasmtime::AsContextMut;

use crate::error::GolemError;
use crate::metrics::wasm::{record_invocation, record_invocation_consumption};
use crate::model::{InterruptKind, TrapType};
use crate::workerctx::{FuelManagement, WorkerCtx};

/// Invokes a function on a worker.
/// Returns true if the function invocation was finished, false if it was interrupted or scheduled for retry.
///
/// The context is held until the invocation finishes
///
/// Arguments:
/// - `full_function_name`: the name of the function to invoke, including the interface name if applicable
/// - `function_input`: the input parameters for the function
/// - `store`: reference to the wasmtime instance's store
/// - `instance`: reference to the wasmtime instance
/// - `calling_convention`: the calling convention to use
/// - `was_live_before`: whether the worker was live before the invocation, or this invocation is part of a recovery
pub async fn invoke_worker<Ctx: WorkerCtx>(
    full_function_name: String,
    function_input: Vec<Value>,
    store: &mut impl AsContextMut<Data = Ctx>,
    instance: &wasmtime::component::Instance,
    calling_convention: CallingConvention,
    was_live_before: bool,
) -> Option<Result<Vec<Value>, anyhow::Error>> {
    let mut store = store.as_context_mut();

    let worker_id = store.data().worker_id().clone();

    let result = invoke_or_fail(
        &worker_id,
        full_function_name.clone(),
        function_input,
        &mut store,
        instance,
        calling_convention,
        was_live_before,
    )
    .await;

    debug!("Invocation resulted in {:?}", result);

    match result {
        Err(err) => {
            let trap_type = TrapType::from_error::<Ctx>(&err);
            match trap_type {
                TrapType::Interrupt(InterruptKind::Interrupt) => {
                    record_invocation(was_live_before, "interrupted");
                    None
                }
                TrapType::Interrupt(InterruptKind::Suspend) => {
                    // this invocation was suspended and expected to be resumed by an external call or schedule
                    record_invocation(was_live_before, "suspended");
                    None
                }
                TrapType::Exit => {
                    record_invocation(was_live_before, "exited");
                    Some(Err(err))
                }
                _ => {
                    record_invocation(was_live_before, "failed");
                    Some(Err(err))
                }
            }
        }
        Ok(None) => {
            // this invocation did not produce any result, but we may get one in the future
            None
        }
        Ok(Some(result)) => {
            // this invocation finished and produced a result
            record_invocation(was_live_before, "success");
            Some(Ok(result))
        }
    }
}

async fn invoke_or_fail<Ctx: WorkerCtx>(
    worker_id: &WorkerId,
    full_function_name: String,
    function_input: Vec<Value>,
    store: &mut impl AsContextMut<Data = Ctx>,
    instance: &wasmtime::component::Instance,
    calling_convention: CallingConvention,
    was_live_before: bool,
) -> anyhow::Result<Option<Vec<Value>>> {
    let mut store = store.as_context_mut();

    let parsed = parse_function_name(&full_function_name);

    let function = match &parsed.interface {
        Some(interface_name) => {
            let mut exports = instance.exports(&mut store);
            let mut exported_instance = exports.instance(interface_name).ok_or(anyhow!(
                "could not load exports for interface {}",
                interface_name
            ))?;
            match exported_instance.func(&parsed.function) {
                Some(func) => Ok(Some(func)),
                None => {
                    if parsed.function.starts_with("[drop]") {
                        Ok(None)
                    } else {
                        match parsed.method_as_static() {
                            None => Err(anyhow!(
                                "could not load function {} for interface {}",
                                &parsed.function,
                                interface_name
                            )),
                            Some(parsed_static) => exported_instance
                                .func(&parsed_static.function)
                                .ok_or(anyhow!(
                                    "could not load function {} or {} for interface {}",
                                    &parsed.function,
                                    &parsed_static.function,
                                    interface_name
                                ))
                                .map(Some),
                        }
                    }
                }
            }
        }
        None => instance
            .get_func(&mut store, &parsed.function)
            .ok_or(anyhow!("could not load function {}", &parsed.function))
            .map(Some),
    }?;

    if was_live_before {
        store
            .data_mut()
            .on_exported_function_invoked(
                &full_function_name,
                &function_input,
                Some(calling_convention),
            )
            .await?;
    }

    store.data_mut().set_running();
    store
        .data_mut()
        .store_worker_status(WorkerStatus::Running)
        .await;

    let call_result = match function {
        Some(function) => {
            invoke(
                &mut store,
                function,
                &function_input,
                calling_convention,
                &format!("{worker_id}/{full_function_name}"),
            )
            .await
        }
        None => {
            // Special function: drop
            drop_resource(
                &mut store,
                &function_input,
                &format!("{worker_id}/{full_function_name}"),
            )
            .await
        }
    };

    store.data().set_suspended();

    match call_result {
        Err(err) => {
            let trap_type = TrapType::from_error::<Ctx>(&err);
            let failure_payload = store.data_mut().on_invocation_failure(&trap_type).await?;
            store.data_mut().deactivate().await;
            let result_status = store
                .data_mut()
                .on_invocation_failure_deactivated(&failure_payload, &trap_type)
                .await?;
            store
                .data_mut()
                .store_worker_status(result_status.clone())
                .await;

            if result_status == WorkerStatus::Retrying || result_status == WorkerStatus::Running {
                Ok(None)
            } else {
                store
                    .data_mut()
                    .on_invocation_failure_final(&failure_payload, &trap_type)
                    .await?;
                Err(err)
            }
        }
        Ok(InvokeResult {
            exited,
            consumed_fuel,
            output,
        }) => {
            let result = store
                .data_mut()
                .on_invocation_success(&full_function_name, &function_input, consumed_fuel, output)
                .await?;

            if exited {
                store.data_mut().deactivate().await;
                store
                    .data_mut()
                    .store_worker_status(WorkerStatus::Exited)
                    .await;
            } else {
                store
                    .data_mut()
                    .store_worker_status(WorkerStatus::Idle)
                    .await;
            }
            Ok(result)
        }
    }
}

async fn invoke<Ctx: WorkerCtx>(
    store: &mut impl AsContextMut<Data = Ctx>,
    function: Func,
    function_input: &[Value],
    calling_convention: CallingConvention,
    context: &str,
) -> Result<InvokeResult, anyhow::Error> {
    let mut store = store.as_context_mut();
    match calling_convention {
        CallingConvention::Component => {
            let param_types = function.params(&store);

            if function_input.len() != param_types.len() {
                return Err(GolemError::ParamTypeMismatch.into());
            }

            let mut params = Vec::new();
            let mut resources_to_drop = Vec::new();
            for (param, param_type) in function_input.iter().zip(param_types.iter()) {
                let result =
                    decode_param(param, param_type, store.data_mut()).map_err(GolemError::from)?;
                params.push(result.val);
                resources_to_drop.extend(result.resources_to_drop);
            }

            let (results, consumed_fuel) =
                call_exported_function(&mut store, function, params, context).await?;

            for resource in resources_to_drop {
                debug!("Dropping passed owned resources {:?}", resource);
                resource.resource_drop_async(&mut store).await?;
            }

            let results = results?;

            let mut output: Vec<Value> = Vec::new();
            for result in results.iter() {
                let result_value =
                    encode_output(result, store.data_mut()).map_err(GolemError::from)?;
                output.push(result_value);
            }

            Ok(InvokeResult {
                exited: false,
                consumed_fuel,
                output,
            })
        }
        CallingConvention::Stdio => {
            if function_input.len() != 1 {
                panic!("unexpected parameter count for stdio calling convention for {context}")
            }
            let stdin = match function_input.first().unwrap() {
                Value::String(value) => value.clone(),
                _ => panic!("unexpected function input for stdio calling convention for {context}"),
            };

            store.data_mut().start_capturing_stdout(stdin).await;

            let (call_result, consumed_fuel) =
                call_exported_function(&mut store, function, vec![], context).await?;

            let exited = match call_result {
                Err(err) => {
                    if Ctx::is_exit(&err) == Some(0) {
                        Ok(true)
                    } else {
                        Err(err)
                    }
                }
                Ok(_) => Ok(false),
            }?;

            let stdout = store.data_mut().finish_capturing_stdout().await.ok();
            let output: Vec<Value> = vec![Value::String(stdout.unwrap_or("".to_string()))];

            Ok(InvokeResult {
                exited,
                consumed_fuel,
                output,
            })
        }
    }
}

async fn drop_resource<Ctx: WorkerCtx>(
    store: &mut impl AsContextMut<Data = Ctx>,
    function_input: &[Value],
    context: &str,
) -> Result<InvokeResult, anyhow::Error> {
    let mut store = store.as_context_mut();
    let self_uri = store.data().self_uri();
    if function_input.len() != 1 {
        return Err(GolemError::ValueMismatch {
            details: format!("unexpected parameter count for drop {context}"),
        }
        .into());
    }
    let resource = match function_input.first().unwrap() {
        Value::Handle { uri, resource_id } => {
            if uri == &self_uri {
                Ok(*resource_id)
            } else {
                Err(GolemError::ValueMismatch {
                    details: format!(
                        "trying to drop handle for on wrong worker ({} vs {}) {}",
                        uri.value, self_uri.value, context
                    ),
                }
                .into())
            }
        }
        _ => Err(anyhow!(
            "unexpected function input for drop calling convention for {context}"
        )),
    }?;

    if let Some(resource) = store.data_mut().get(resource) {
        debug!("Dropping resource {resource:?} in {context}");
        resource.resource_drop_async(&mut store).await?;
    }

    let output = Vec::new();

    Ok(InvokeResult {
        exited: false,
        consumed_fuel: 0,
        output,
    })
}

async fn call_exported_function<Ctx: FuelManagement + Send>(
    store: &mut impl AsContextMut<Data = Ctx>,
    function: Func,
    params: Vec<Val>,
    context: &str,
) -> Result<(anyhow::Result<Vec<Val>>, i64), GolemError> {
    let mut store = store.as_context_mut();

    store.data_mut().borrow_fuel().await?;

    let mut results: Vec<Val> = function
        .results(&store)
        .iter()
        .map(|_| Val::Bool(false))
        .collect();

    // We always have to return the captured stdout in Stdio calling convention because
    // of the special error I32Exit(0) which is treated as a success, triggered through WASI
    let result = function.call_async(&mut store, &params, &mut results).await;
    let result = if result.is_ok() {
        function.post_return_async(&mut store).await.map_err(|e| {
            error!("Error in post_return_async for {context}: {}", e);
            e
        })
    } else {
        result
    };

    let current_fuel_level = store.get_fuel().unwrap_or(0);
    let consumed_fuel_for_call = store
        .data_mut()
        .return_fuel(current_fuel_level as i64)
        .await?;

    if consumed_fuel_for_call > 0 {
        debug!(
            "Fuel consumed for call {context}: {}",
            consumed_fuel_for_call
        );
    }
    record_invocation_consumption(consumed_fuel_for_call);

    Ok((result.map(|_| results), consumed_fuel_for_call))
}

struct InvokeResult {
    pub exited: bool,
    pub consumed_fuel: i64,
    pub output: Vec<Value>,
}
