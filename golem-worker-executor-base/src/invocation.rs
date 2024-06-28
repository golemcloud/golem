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

use golem_common::model::oplog::WorkerError;
use golem_common::model::{CallingConvention, WorkerId, WorkerStatus};
use golem_wasm_rpc::wasmtime::{decode_param, encode_output, type_to_analysed_type};
use golem_wasm_rpc::Value;
use rib::{ParsedFunctionName, ParsedFunctionReference};
use tracing::{debug, error};
use wasmtime::component::{Func, Val};
use wasmtime::{AsContextMut, StoreContextMut};

use crate::error::GolemError;
use crate::metrics::wasm::{record_invocation, record_invocation_consumption};
use crate::model::{InterruptKind, TrapType};
use crate::workerctx::{FuelManagement, WorkerCtx};

/// Invokes a function on a worker.
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
// TODO: rename - this just adds outcome metrics recording?
pub async fn invoke_worker<Ctx: WorkerCtx>(
    full_function_name: String,
    function_input: Vec<Value>,
    store: &mut impl AsContextMut<Data = Ctx>,
    instance: &wasmtime::component::Instance,
    calling_convention: CallingConvention,
    was_live_before: bool,
) -> Result<InvokeResult, GolemError> {
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
            interrupt_kind: InterruptKind::Interrupt,
            ..
        }) => {
            record_invocation(was_live_before, "interrupted");
            result
        }
        Ok(InvokeResult::Interrupted {
            interrupt_kind: InterruptKind::Suspend,
            ..
        }) => {
            record_invocation(was_live_before, "suspended");
            result
        }
        Ok(InvokeResult::Interrupted { .. }) => {
            record_invocation(was_live_before, "restarted"); // TODO: do we want to record this?
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
    store: &mut StoreContextMut<'a, Ctx>,
    instance: &'a wasmtime::component::Instance,
    parsed_function_name: &ParsedFunctionName,
) -> Result<Option<Func>, GolemError> {
    match &parsed_function_name.site().interface_name() {
        Some(interface_name) => {
            let mut exports = instance.exports(store);
            let mut exported_instance =
                exports
                    .instance(interface_name)
                    .ok_or(GolemError::runtime(format!(
                        "could not load exports for interface {}",
                        interface_name
                    )))?;
            match exported_instance.func(&parsed_function_name.function().function_name()) {
                Some(func) => Ok(Some(func)),
                None => {
                    if matches!(
                        parsed_function_name.function(),
                        ParsedFunctionReference::RawResourceDrop { .. }
                            | ParsedFunctionReference::IndexedResourceDrop { .. }
                    ) {
                        Ok(None)
                    } else {
                        match parsed_function_name.method_as_static() {
                            None => Err(GolemError::runtime(format!(
                                "could not load function {} for interface {}",
                                &parsed_function_name.function().function_name(),
                                interface_name
                            ))),
                            Some(parsed_static) => exported_instance
                                .func(&parsed_static.function().function_name())
                                .ok_or(GolemError::runtime(format!(
                                    "could not load function {} or {} for interface {}",
                                    &parsed_function_name.function().function_name(),
                                    &parsed_static.function().function_name(),
                                    interface_name
                                )))
                                .map(Some),
                        }
                    }
                }
            }
        }
        None => instance
            .get_func(store, &parsed_function_name.function().function_name())
            .ok_or(GolemError::runtime(format!(
                "could not load function {}",
                &parsed_function_name.function().function_name()
            )))
            .map(Some),
    }
}

// TODO: rename
async fn invoke_or_fail<Ctx: WorkerCtx>(
    worker_id: &WorkerId,
    full_function_name: String,
    mut function_input: Vec<Value>,
    store: &mut impl AsContextMut<Data = Ctx>,
    instance: &wasmtime::component::Instance,
    calling_convention: CallingConvention,
    was_live_before: bool,
) -> Result<InvokeResult, GolemError> {
    let mut store = store.as_context_mut();

    let parsed = ParsedFunctionName::parse(&full_function_name)
        .map_err(|err| GolemError::invalid_request(format!("Invalid function name: {}", err)))?;

    let function = find_function(&mut store, instance, &parsed)?;

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

    let context = format!("{worker_id}/{full_function_name}");
    let mut extra_fuel = 0;

    if parsed.function().is_indexed_resource() {
        let resource_handle =
            get_or_create_indexed_resource(&mut store, instance, &parsed, &context).await?;

        match resource_handle {
            InvokeResult::Succeeded {
                consumed_fuel,
                output,
            } => {
                function_input = [output, function_input].concat();
                extra_fuel = consumed_fuel;
            }
            other => {
                // Early return because of a failed invocation of the resource constructor
                return Ok(other);
            }
        }
    }

    let mut call_result = match function {
        Some(function) => {
            invoke(
                &mut store,
                function,
                &function_input,
                calling_convention,
                &context,
            )
            .await
        }
        None => {
            // Special function: drop
            drop_resource(&mut store, &parsed, &function_input, &context).await
        }
    };
    if let Ok(r) = call_result.as_mut() {
        r.add_fuel(extra_fuel);
    }

    store.data().set_suspended();

    call_result
}

async fn get_or_create_indexed_resource<'a, Ctx: WorkerCtx>(
    store: &mut StoreContextMut<'a, Ctx>,
    instance: &'a wasmtime::component::Instance,
    parsed_function_name: &ParsedFunctionName,
    context: &str,
) -> Result<InvokeResult, GolemError> {
    let resource_name =
        parsed_function_name
            .function()
            .resource_name()
            .ok_or(GolemError::invalid_request(
                "Cannot extract resource name from function name",
            ))?;

    let resource_constructor_name = ParsedFunctionName::new(
        parsed_function_name.site().clone(),
        ParsedFunctionReference::RawResourceConstructor {
            resource: resource_name.clone(),
        },
    );
    let resource_constructor = find_function(store, instance, &resource_constructor_name)?.ok_or(
        GolemError::runtime(format!(
            "could not find resource constructor for resource {}",
            resource_name
        )),
    )?;

    let constructor_param_types = resource_constructor.params(&store).iter().map(type_to_analysed_type).collect::<Result<Vec<_>, _>>()
        .map_err(|err| GolemError::invalid_request(format!("Indexed resource invocation cannot be used with owned or borrowed resource handles in constructor parameter position! ({err})")))?;

    let raw_constructor_params = parsed_function_name
        .function()
        .raw_resource_params()
        .ok_or(GolemError::invalid_request(
            "Could not extract raw resource constructor parameters from function name",
        ))?;

    match store
        .data()
        .get_indexed_resource(resource_name, raw_constructor_params)
    {
        Some(resource_id) => {
            debug!("Using existing indexed resource with id {resource_id}");
            Ok(InvokeResult::from_success(
                0,
                vec![Value::Handle {
                    uri: store.data().self_uri(),
                    resource_id,
                }],
            ))
        }
        None => {
            let constructor_params = parsed_function_name
                .function()
                .resource_params(&constructor_param_types)
                .map_err(|err| {
                    GolemError::invalid_request(format!(
                        "Failed to parse resource constructor parameters: {err}"
                    ))
                })?
                .ok_or(GolemError::invalid_request(
                    "Could not extract resource constructor parameters from function name",
                ))?;

            debug!("Creating new indexed resource with parameters {constructor_params:?}");

            let constructor_result = invoke(
                store,
                resource_constructor,
                &constructor_params,
                CallingConvention::Component,
                context,
            )
            .await?;

            if let InvokeResult::Succeeded { output, .. } = &constructor_result {
                if let Some(Value::Handle { resource_id, .. }) = output.first() {
                    debug!("Storing indexed resource with id {resource_id}");
                    store.data_mut().store_indexed_resource(
                        resource_name,
                        raw_constructor_params,
                        *resource_id,
                    );
                } else {
                    return Err(GolemError::invalid_request(
                        "Resource constructor did not return a resource handle",
                    ));
                }
            }

            Ok(constructor_result)
        }
    }
}

// TODO: rename
async fn invoke<Ctx: WorkerCtx>(
    store: &mut impl AsContextMut<Data = Ctx>,
    function: Func,
    function_input: &[Value],
    calling_convention: CallingConvention,
    context: &str,
) -> Result<InvokeResult, GolemError> {
    let mut store = store.as_context_mut();
    match calling_convention {
        CallingConvention::Component => {
            let param_types = function.params(&store);

            if function_input.len() != param_types.len() {
                return Err(GolemError::ParamTypeMismatch);
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

            match results {
                Ok(results) => {
                    let types = function.results(&store);
                    let mut output: Vec<Value> = Vec::new();
                    for (val, typ) in results.iter().zip(types.iter()) {
                        let result_value =
                            encode_output(val, typ, store.data_mut()).map_err(GolemError::from)?;
                        output.push(result_value);
                    }

                    Ok(InvokeResult::from_success(consumed_fuel, output))
                }
                Err(err) => Ok(InvokeResult::from_error::<Ctx>(consumed_fuel, &err)),
            }
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

            match call_result {
                Err(err) => Ok(InvokeResult::from_error::<Ctx>(consumed_fuel, &err)),
                Ok(_) => {
                    let stdout = store.data_mut().finish_capturing_stdout().await.ok();
                    let output: Vec<Value> = vec![Value::String(stdout.unwrap_or("".to_string()))];
                    Ok(InvokeResult::from_success(consumed_fuel, output))
                }
            }
        }
    }
}

async fn drop_resource<Ctx: WorkerCtx>(
    store: &mut impl AsContextMut<Data = Ctx>,
    parsed_function_name: &ParsedFunctionName,
    function_input: &[Value],
    context: &str,
) -> Result<InvokeResult, GolemError> {
    let mut store = store.as_context_mut();
    let self_uri = store.data().self_uri();
    if function_input.len() != 1 {
        return Err(GolemError::ValueMismatch {
            details: format!("unexpected parameter count for drop {context}"),
        });
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
                })
            }
        }
        _ => Err(GolemError::ValueMismatch {
            details: format!("unexpected function input for drop calling convention for {context}"),
        }),
    }?;

    if let ParsedFunctionReference::IndexedResourceDrop {
        resource,
        resource_params,
    } = parsed_function_name.function()
    {
        debug!(
            "Dropping indexed resource {resource:?} with params {resource_params:?} in {context}"
        );
        store
            .data_mut()
            .drop_indexed_resource(resource, resource_params);
    }

    if let Some(resource) = store.data_mut().get(resource) {
        debug!("Dropping resource {resource:?} in {context}");
        store.data_mut().borrow_fuel().await?;

        let result = resource.resource_drop_async(&mut store).await;

        let current_fuel_level = store.get_fuel().unwrap_or(0);
        let consumed_fuel = store
            .data_mut()
            .return_fuel(current_fuel_level as i64)
            .await?;

        match result {
            Ok(_) => Ok(InvokeResult::from_success(consumed_fuel, vec![])),
            Err(err) => Ok(InvokeResult::from_error::<Ctx>(consumed_fuel, &err)),
        }
    } else {
        Ok(InvokeResult::from_success(0, vec![]))
    }
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

#[derive(Clone, Debug)]
pub enum InvokeResult {
    /// The invoked function exited with exit code 0
    Exited { consumed_fuel: i64 },
    /// The invoked function has failed
    Failed {
        consumed_fuel: i64,
        error: WorkerError,
    },
    /// The invoked function succeeded and produced a result
    Succeeded {
        consumed_fuel: i64,
        output: Vec<Value>,
    },
    /// The function was running but got interrupted
    Interrupted {
        consumed_fuel: i64,
        interrupt_kind: InterruptKind,
    },
}

impl InvokeResult {
    pub fn from_success(consumed_fuel: i64, output: Vec<Value>) -> Self {
        InvokeResult::Succeeded {
            consumed_fuel,
            output,
        }
    }

    pub fn from_error<Ctx: WorkerCtx>(consumed_fuel: i64, error: &anyhow::Error) -> Self {
        match TrapType::from_error::<Ctx>(error) {
            TrapType::Interrupt(kind) => InvokeResult::Interrupted {
                consumed_fuel,
                interrupt_kind: kind,
            },
            TrapType::Exit => InvokeResult::Exited { consumed_fuel },
            TrapType::Error(error) => InvokeResult::Failed {
                consumed_fuel,
                error,
            },
        }
    }

    pub fn consumed_fuel(&self) -> i64 {
        match self {
            InvokeResult::Exited { consumed_fuel, .. }
            | InvokeResult::Failed { consumed_fuel, .. }
            | InvokeResult::Succeeded { consumed_fuel, .. }
            | InvokeResult::Interrupted { consumed_fuel, .. } => *consumed_fuel,
        }
    }

    pub fn add_fuel(&mut self, extra_fuel: i64) {
        match self {
            InvokeResult::Exited { consumed_fuel } => {
                *consumed_fuel += extra_fuel;
            }
            InvokeResult::Failed { consumed_fuel, .. } => {
                *consumed_fuel += extra_fuel;
            }
            InvokeResult::Succeeded { consumed_fuel, .. } => {
                *consumed_fuel += extra_fuel;
            }
            InvokeResult::Interrupted { consumed_fuel, .. } => {
                *consumed_fuel += extra_fuel;
            }
        }
    }

    pub fn as_trap_type<Ctx: WorkerCtx>(&self) -> Option<TrapType> {
        match self {
            InvokeResult::Failed { error, .. } => Some(TrapType::Error(error.clone())),
            InvokeResult::Interrupted { interrupt_kind, .. } => {
                Some(TrapType::Interrupt(interrupt_kind.clone()))
            }
            InvokeResult::Exited { .. } => Some(TrapType::Exit),
            _ => None,
        }
    }
}
