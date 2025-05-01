// Copyright 2024-2025 Golem Cloud
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

use crate::error::GolemError;
use crate::metrics::wasm::{record_invocation, record_invocation_consumption};
use crate::model::{InterruptKind, TrapType};
use crate::virtual_export_compat;
use crate::workerctx::{PublicWorkerIo, WorkerCtx};
use anyhow::anyhow;
use golem_common::model::oplog::{WorkerError, WorkerResourceId};
use golem_common::model::{IdempotencyKey, WorkerStatus};
use golem_common::virtual_exports;
use golem_wasm_rpc::wasmtime::{decode_param, encode_output, type_to_analysed_type};
use golem_wasm_rpc::Value;
use rib::{ParsedFunctionName, ParsedFunctionReference};
use tracing::{debug, error, Instrument};
use wasmtime::component::{Func, Val};
use wasmtime::{AsContextMut, StoreContextMut};
use wasmtime_wasi_http::bindings::Proxy;
use wasmtime_wasi_http::WasiHttpView;

/// Invokes a function on a worker.
///
/// The context is held until the invocation finishes
///
/// Arguments:
/// - `full_function_name`: the name of the function to invoke, including the interface name if applicable
/// - `function_input`: the input parameters for the function
/// - `store`: reference to the wasmtime instance's store
/// - `instance`: reference to the wasmtime instance
pub async fn invoke_observed_and_traced<Ctx: WorkerCtx>(
    full_function_name: String,
    function_input: Vec<Value>,
    store: &mut impl AsContextMut<Data = Ctx>,
    instance: &wasmtime::component::Instance,
) -> Result<InvokeResult, GolemError> {
    let mut store = store.as_context_mut();
    let was_live_before = store.data().is_live();

    let result = invoke_observed(
        full_function_name.clone(),
        function_input,
        &mut store,
        instance,
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

/// Returns the first function from the given list that is available on the instance
///
/// This can be used to find an exported function when multiple versions of an interface
/// is supported, such as for the load-snapshot/save-snapshot interfaces.
///
/// This function should not be used on the hot path.
pub fn find_first_available_function<Ctx: WorkerCtx>(
    store: &mut impl AsContextMut<Data = Ctx>,
    instance: &wasmtime::component::Instance,
    names: Vec<String>,
) -> Option<String> {
    let mut store = store.as_context_mut();
    for name in names {
        let parsed = ParsedFunctionName::parse(&name).ok()?;

        if let Ok(FindFunctionResult::ExportedFunction(_)) =
            find_function(&mut store, instance, &parsed)
        {
            return Some(name);
        }
    }
    None
}

fn find_function<'a, Ctx: WorkerCtx>(
    mut store: &mut StoreContextMut<'a, Ctx>,
    instance: &'a wasmtime::component::Instance,
    parsed_function_name: &ParsedFunctionName,
) -> Result<FindFunctionResult, GolemError> {
    if *parsed_function_name == *virtual_exports::http_incoming_handler::PARSED_FUNCTION_NAME {
        return Ok(FindFunctionResult::IncomingHttpHandlerBridge);
    };

    let parsed_function_ref = parsed_function_name.function();

    if matches!(
        parsed_function_ref,
        ParsedFunctionReference::RawResourceDrop { .. }
            | ParsedFunctionReference::IndexedResourceDrop { .. }
    ) {
        return Ok(FindFunctionResult::ResourceDrop);
    }

    match &parsed_function_name.site().interface_name() {
        Some(interface_name) => {
            let exported_instance_idx = instance
                .get_export(&mut store, None, interface_name)
                .ok_or(GolemError::invalid_request(format!(
                    "could not load exports for interface {}",
                    interface_name
                )))?;

            let func = instance
                .get_export(
                    &mut store,
                    Some(&exported_instance_idx),
                    &parsed_function_name.function().function_name(),
                )
                .and_then(|idx| instance.get_func(&mut store, idx));

            match func {
                Some(func) => Ok(FindFunctionResult::ExportedFunction(func)),
                None => match parsed_function_name.method_as_static() {
                    None => Err(GolemError::invalid_request(format!(
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
                        .and_then(|idx| instance.get_func(&mut store, idx))
                        .ok_or(GolemError::invalid_request(format!(
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
            .ok_or(GolemError::invalid_request(format!(
                "could not load function {}",
                &parsed_function_name.function().function_name()
            )))
            .map(FindFunctionResult::ExportedFunction),
    }
}

/// Invokes a worker and calls the appropriate hooks to observe the invocation
async fn invoke_observed<Ctx: WorkerCtx>(
    full_function_name: String,
    mut function_input: Vec<Value>,
    store: &mut impl AsContextMut<Data = Ctx>,
    instance: &wasmtime::component::Instance,
) -> Result<InvokeResult, GolemError> {
    let mut store = store.as_context_mut();

    let parsed = ParsedFunctionName::parse(&full_function_name)
        .map_err(|err| GolemError::invalid_request(format!("Invalid function name: {}", err)))?;

    let function = find_function(&mut store, instance, &parsed)?;

    validate_function_parameters(
        &mut store,
        &function,
        &full_function_name,
        &function_input,
        parsed.function().is_indexed_resource(),
    )?;

    if store.data().is_live() {
        store
            .data_mut()
            .on_exported_function_invoked(&full_function_name, &function_input)
            .await?;
    }

    store.data_mut().set_running();
    store
        .data_mut()
        .store_worker_status(WorkerStatus::Running)
        .await;

    let mut extra_fuel = 0;

    if parsed.function().is_indexed_resource() {
        let resource_handle =
            get_or_create_indexed_resource(&mut store, instance, &parsed, &full_function_name)
                .await?;

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
        FindFunctionResult::ExportedFunction(function) => {
            invoke(&mut store, function, &function_input, &full_function_name).await
        }
        FindFunctionResult::ResourceDrop => {
            // Special function: drop
            drop_resource(&mut store, &parsed, &function_input, &full_function_name).await
        }
        FindFunctionResult::IncomingHttpHandlerBridge => {
            invoke_http_handler(&mut store, instance, &function_input, &full_function_name).await
        }
    };
    if let Ok(r) = call_result.as_mut() {
        r.add_fuel(extra_fuel);
    }

    store.data().set_suspended().await?;

    call_result
}

fn validate_function_parameters(
    store: &mut impl AsContextMut<Data = impl WorkerCtx>,
    function: &FindFunctionResult,
    raw_function_name: &str,
    function_input: &[Value],
    using_indexed_resource: bool,
) -> Result<(), GolemError> {
    match function {
        FindFunctionResult::ExportedFunction(func) => {
            let store = store.as_context_mut();
            let param_types: Vec<_> = if using_indexed_resource {
                // For indexed resources we are going to inject the resource handle as the first parameter
                // later so we only have to validate the remaining parameters
                let params = func.params(&store);
                params.iter().skip(1).cloned().collect()
            } else {
                let params = func.params(&store);
                params.to_vec()
            };

            if function_input.len() != param_types.len() {
                return Err(GolemError::ParamTypeMismatch {
                    details: format!(
                        "expected {}, got {} parameters",
                        param_types.len(),
                        function_input.len()
                    ),
                });
            }
        }
        FindFunctionResult::ResourceDrop => {
            let expected = if using_indexed_resource { 0 } else { 1 };
            if function_input.len() != expected {
                return Err(GolemError::ValueMismatch {
                    details: "unexpected parameter count for drop".to_string(),
                });
            }

            if !using_indexed_resource {
                let store = store.as_context_mut();
                let self_uri = store.data().self_uri();

                match function_input.first() {
                    Some(Value::Handle { uri, resource_id }) => {
                        if uri == &self_uri.value {
                            Ok(*resource_id)
                        } else {
                            Err(GolemError::ValueMismatch {
                                details: format!(
                                    "trying to drop handle for on wrong worker ({} vs {}) {}",
                                    uri, self_uri.value, raw_function_name
                                ),
                            })
                        }
                    }
                    _ => Err(GolemError::ValueMismatch {
                        details: format!(
                            "unexpected function input for drop for {raw_function_name}"
                        ),
                    }),
                }?;
            }
        }
        FindFunctionResult::IncomingHttpHandlerBridge => {}
    }
    Ok(())
}

async fn get_or_create_indexed_resource<'a, Ctx: WorkerCtx>(
    store: &mut StoreContextMut<'a, Ctx>,
    instance: &'a wasmtime::component::Instance,
    parsed_function_name: &ParsedFunctionName,
    raw_function_name: &str,
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

    let resource_constructor = if let FindFunctionResult::ExportedFunction(func) =
        find_function(store, instance, &resource_constructor_name)?
    {
        func
    } else {
        Err(GolemError::invalid_request(format!(
            "could not find resource constructor for resource {}",
            resource_name
        )))?
    };

    let constructor_param_types = resource_constructor.params(store as &StoreContextMut<'a, Ctx>).iter().map(type_to_analysed_type).collect::<Result<Vec<_>, _>>()
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
                    uri: store.data().self_uri().value,
                    resource_id: resource_id.0,
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

            let constructor_params: Vec<Value> = constructor_params
                .into_iter()
                .map(|vnt| vnt.value)
                .collect();

            debug!("Creating new indexed resource with parameters {constructor_params:?}");

            let constructor_result = invoke(
                store,
                resource_constructor,
                &constructor_params,
                raw_function_name,
            )
            .await?;

            if let InvokeResult::Succeeded { output, .. } = &constructor_result {
                if let Some(Value::Handle { resource_id, .. }) = output.first() {
                    debug!("Storing indexed resource with id {resource_id}");
                    store
                        .data_mut()
                        .store_indexed_resource(
                            resource_name,
                            raw_constructor_params,
                            WorkerResourceId(*resource_id),
                        )
                        .await;
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

async fn invoke<Ctx: WorkerCtx>(
    store: &mut impl AsContextMut<Data = Ctx>,
    function: Func,
    function_input: &[Value],
    raw_function_name: &str,
) -> Result<InvokeResult, GolemError> {
    let mut store = store.as_context_mut();
    let param_types = function.params(&store);

    let mut params = Vec::new();
    let mut resources_to_drop = Vec::new();
    for (param, param_type) in function_input.iter().zip(param_types.iter()) {
        let result = decode_param(param, param_type, store.data_mut())
            .await
            .map_err(GolemError::from)?;
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
            let mut output: Vec<Value> = Vec::new();
            for (val, typ) in results.iter().zip(types.iter()) {
                let result_value = encode_output(val, typ, store.data_mut())
                    .await
                    .map_err(GolemError::from)?;
                output.push(result_value);
            }

            Ok(InvokeResult::from_success(consumed_fuel, output))
        }
        Err(err) => Ok(InvokeResult::from_error::<Ctx>(consumed_fuel, &err)),
    }
}

async fn invoke_http_handler<Ctx: WorkerCtx>(
    store: &mut impl AsContextMut<Data = Ctx>,
    instance: &wasmtime::component::Instance,
    function_input: &[Value],
    raw_function_name: &str,
) -> Result<InvokeResult, GolemError> {
    let (sender, receiver) = tokio::sync::oneshot::channel();

    let proxy = Proxy::new(&mut *store, instance).unwrap();
    let mut store_context = store.as_context_mut();

    store_context.data_mut().borrow_fuel().await?;

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
            .new_incoming_request(scheme, hyper_request)
            .unwrap();
        let outgoing = store_context
            .data_mut()
            .as_wasi_http_view()
            .new_response_outparam(sender)
            .unwrap();

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
            // Instead we retrieve and propagate the error from inside the task
            // which should more clearly tell the user what went wrong. Note
            // that we assume the task has already exited at this point so the
            // `await` should resolve immediately.
            let task_exit = task_exits.remove(0);
            let e = match task_exit {
                Ok(r) => r.expect_err("if the receiver has an error, the task must have failed"),
                Err(_e) => anyhow!("failed joining wasm task"),
            };
            Err(e)?
        }
    };

    let consumed_fuel = finish_invocation_and_get_fuel_consumption(
        &mut store.as_context_mut(),
        raw_function_name,
        idempotency_key,
    )
    .await?;

    match res_or_error {
        Ok(resp) => Ok(InvokeResult::from_success(consumed_fuel, vec![resp])),
        Err(e) => Ok(InvokeResult::from_error::<Ctx>(consumed_fuel, &e)),
    }
}

async fn drop_resource<Ctx: WorkerCtx>(
    store: &mut impl AsContextMut<Data = Ctx>,
    parsed_function_name: &ParsedFunctionName,
    function_input: &[Value],
    raw_function_name: &str,
) -> Result<InvokeResult, GolemError> {
    let mut store = store.as_context_mut();

    let resource_id = match function_input.first() {
        Some(Value::Handle { resource_id, .. }) => *resource_id,
        _ => unreachable!(), // previously validated by `validate_function_parameters`
    };

    if let ParsedFunctionReference::IndexedResourceDrop {
        resource,
        resource_params,
    } = parsed_function_name.function()
    {
        debug!(
            "Dropping indexed resource {resource:?} with params {resource_params:?} in {raw_function_name}"
        );
        store
            .data_mut()
            .drop_indexed_resource(resource, resource_params);
    }

    if let Some(resource) = store.data_mut().get(resource_id).await {
        debug!("Dropping resource {resource:?} in {raw_function_name}");
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

async fn call_exported_function<Ctx: WorkerCtx>(
    store: &mut impl AsContextMut<Data = Ctx>,
    function: Func,
    params: Vec<Val>,
    raw_function_name: &str,
) -> Result<(anyhow::Result<Vec<Val>>, i64), GolemError> {
    let mut store = store.as_context_mut();

    store.data_mut().borrow_fuel().await?;

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
        finish_invocation_and_get_fuel_consumption(&mut store, raw_function_name, idempotency_key)
            .await?;

    Ok((result.map(|_| results), consumed_fuel_for_call))
}

async fn finish_invocation_and_get_fuel_consumption<Ctx: WorkerCtx>(
    store: &mut StoreContextMut<'_, Ctx>,
    raw_function_name: &str,
    idempotency_key: Option<IdempotencyKey>,
) -> Result<i64, GolemError> {
    let current_fuel_level = store.get_fuel().unwrap_or(0);
    let consumed_fuel_for_call = store
        .data_mut()
        .return_fuel(current_fuel_level as i64)
        .await?;

    if consumed_fuel_for_call > 0 {
        debug!(
            "Fuel consumed for call {raw_function_name}: {}",
            consumed_fuel_for_call
        );
    }

    if let Some(idempotency_key) = idempotency_key {
        store
            .data()
            .get_public_state()
            .event_service()
            .emit_invocation_finished(raw_function_name, &idempotency_key, store.data().is_live());
    }

    record_invocation_consumption(consumed_fuel_for_call);

    Ok(consumed_fuel_for_call)
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

enum FindFunctionResult {
    ExportedFunction(Func),
    ResourceDrop,
    IncomingHttpHandlerBridge,
}
