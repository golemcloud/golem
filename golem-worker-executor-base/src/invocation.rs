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
use golem_common::model::{CallingConvention, VersionedWorkerId, WorkerStatus};
use golem_wasm_rpc::wasmtime::{decode_param, encode_output};
use golem_wasm_rpc::Value;
use tracing::{debug, error, warn};
use wasmtime::component::{Func, Val};
use wasmtime::AsContextMut;

use crate::error::{is_interrupt, is_suspend, GolemError};
use crate::metrics::wasm::{record_invocation, record_invocation_consumption};
use crate::model::parse_function_name;
use crate::workerctx::{FuelManagement, WorkerCtx};

/// Invokes a function on a worker.
/// Returns true if the function invocation was finished, false if it was interrupted or scheduled for retry.
///
/// The WorkerDetails reference is hold until the invocation finishes
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
    function_input: Vec<golem_wasm_rpc::protobuf::Val>,
    store: &mut impl AsContextMut<Data = Ctx>,
    instance: &wasmtime::component::Instance,
    calling_convention: &CallingConvention,
    was_live_before: bool,
) -> bool {
    let mut store = store.as_context_mut();

    let worker_id = store.data().worker_id().clone();
    debug!("invoke_worker_impl: {worker_id}/{full_function_name}");

    if let Some(invocation_key) = &store.data().get_current_invocation_key().await {
        store.data_mut().resume_invocation_key(invocation_key).await
    }

    debug!("invoke_worker_impl_or_fail: {worker_id}/{full_function_name}");

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

    debug!(
        "invoke_worker_impl_or_fail: {worker_id}/{full_function_name} resulted in {:?}",
        result
    );

    let invocation_key = store.data().get_current_invocation_key().await;
    match result {
        Err(err) => {
            if is_interrupt(&err) {
                // this invocation was interrupted and has to be resumed manually later
                match invocation_key {
                    Some(invocation_key) => {
                        debug!(
                            "Storing interrupted status for invocation key {:?}",
                            &invocation_key
                        );
                        store
                            .data_mut()
                            .interrupt_invocation_key(&invocation_key)
                            .await;
                    }
                    None => {
                        warn!("Fire-and-forget invocation of {worker_id}/{full_function_name} got interrupted");
                    }
                }
                record_invocation(was_live_before, "interrupted");
                false
            } else if is_suspend(&err) {
                // this invocation was suspended and expected to be resumed by an external call or schedule
                record_invocation(was_live_before, "suspended");
                false
            } else {
                // this invocation failed it won't be retried later
                match invocation_key {
                    Some(invocation_key) => {
                        debug!(
                            "Storing failed result for invocation key {:?}",
                            &invocation_key
                        );
                        store
                            .data_mut()
                            .confirm_invocation_key(&invocation_key, Err(err.into()))
                            .await;
                    }
                    None => {
                        error!("Fire-and-forget invocation of {worker_id}/{full_function_name} failed: {}", err);
                    }
                }
                record_invocation(was_live_before, "failed");
                true
            }
        }
        Ok(None) => {
            // this invocation did not produce any result, but we may get one in the future
            false
        }
        Ok(Some(_)) => {
            // this invocation finished and produced a result
            record_invocation(was_live_before, "success");
            true
        }
    }
}

async fn invoke_or_fail<Ctx: WorkerCtx>(
    worker_id: &VersionedWorkerId,
    full_function_name: String,
    function_input: Vec<golem_wasm_rpc::protobuf::Val>,
    store: &mut impl AsContextMut<Data = Ctx>,
    instance: &wasmtime::component::Instance,
    calling_convention: &CallingConvention,
    was_live_before: bool,
) -> anyhow::Result<Option<Vec<golem_wasm_rpc::protobuf::Val>>> {
    let mut store = store.as_context_mut();

    let (interface_name, function_name) = parse_function_name(&full_function_name);

    let function = match interface_name {
        Some(interface_name) => instance
            .exports(&mut store)
            .instance(interface_name)
            .ok_or(anyhow!(
                "could not load exports for interface {}",
                interface_name
            ))?
            .func(function_name)
            .ok_or(anyhow!(
                "could not load function {} for interface {}",
                function_name,
                interface_name
            )),
        None => instance
            .get_func(&mut store, function_name)
            .ok_or(anyhow!("could not load function {}", function_name)),
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

    let call_result = invoke(
        &mut store,
        function,
        &function_input,
        calling_convention.try_into().unwrap(),
        &format!("{worker_id}/{function_name}"),
    )
    .await;

    store.data().set_suspended();

    match call_result {
        Err(err) => {
            store.data_mut().on_invocation_failure(&err).await?;
            store.data_mut().deactivate().await;
            let result_status = store
                .data_mut()
                .on_invocation_failure_deactivated(&err)
                .await?;
            store
                .data_mut()
                .store_worker_status(result_status.clone())
                .await;

            if result_status == WorkerStatus::Retrying {
                Ok(None)
            } else {
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
    function_input: &[golem_wasm_rpc::protobuf::Val],
    calling_convention: InternalCallingConvention,
    context: &str,
) -> Result<InvokeResult, anyhow::Error> {
    let mut store = store.as_context_mut();
    match calling_convention {
        InternalCallingConvention::Component => {
            let param_types = function.params(&store);
            if function_input.len() != param_types.len() {
                return Err(GolemError::ParamTypeMismatch.into());
            }

            let params = function_input
                .iter()
                .zip(param_types.iter())
                .map(|(param, param_type)| {
                    let param_value: Value = param
                        .clone()
                        .try_into()
                        .map_err(|err| GolemError::ValueMismatch { details: err })?;
                    decode_param(&param_value, param_type).map_err(|err| GolemError::from(err))
                })
                .collect::<Result<Vec<Val>, GolemError>>()?;

            let (results, consumed_fuel) =
                call_exported_function(&mut store, function, params, context).await?;
            let results = results?;

            let mut output: Vec<golem_wasm_rpc::protobuf::Val> = Vec::new();
            for result in results.iter() {
                let result_value = encode_output(result).map_err(|err| GolemError::from(err))?;
                let proto_value = result_value.into();
                output.push(proto_value);
            }

            if let Some(invocation_key) = store.data().get_current_invocation_key().await {
                debug!(
                    "Storing successful results for invocation key {:?}",
                    &invocation_key
                );

                store
                    .data_mut()
                    .confirm_invocation_key(&invocation_key, Ok(output.clone()))
                    .await;
            } else {
                debug!("No invocation key");
            }

            Ok(InvokeResult {
                exited: false,
                consumed_fuel,
                output,
            })
        }
        InternalCallingConvention::Stdio => {
            if function_input.len() != 1 {
                panic!("unexpected parameter count for stdio calling convention for {context}")
            }
            let stdin = match function_input.first().unwrap().val.as_ref().unwrap() {
                golem_wasm_rpc::protobuf::val::Val::String(value) => value.clone(),
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

            let output: Vec<golem_wasm_rpc::protobuf::Val> = vec![golem_wasm_rpc::protobuf::Val {
                val: Some(golem_wasm_rpc::protobuf::val::Val::String(
                    stdout.unwrap_or("".to_string()),
                )),
            }];

            if let Some(invocation_key) = store.data().get_current_invocation_key().await {
                debug!(
                    "Storing successful results for invocation key {:?}",
                    &invocation_key
                );

                store
                    .data_mut()
                    .confirm_invocation_key(&invocation_key, Ok(output.clone()))
                    .await;
            } else {
                debug!("No invocation key");
            }

            Ok(InvokeResult {
                exited,
                consumed_fuel,
                output,
            })
        }
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
    debug!("fuel consumed for call: {}", consumed_fuel_for_call);
    record_invocation_consumption(consumed_fuel_for_call);

    Ok((result.map(|_| results), consumed_fuel_for_call))
}

struct InvokeResult {
    pub exited: bool,
    pub consumed_fuel: i64,
    pub output: Vec<golem_wasm_rpc::protobuf::Val>,
}

#[derive(Clone, Debug)]
pub enum InternalCallingConvention {
    Component,
    Stdio,
}

impl TryFrom<&CallingConvention> for InternalCallingConvention {
    type Error = String;

    fn try_from(value: &CallingConvention) -> Result<Self, Self::Error> {
        match *value {
            CallingConvention::Component => Ok(InternalCallingConvention::Component),
            CallingConvention::Stdio => Ok(InternalCallingConvention::Stdio),
            CallingConvention::StdioEventloop => {
                Err("Invalid state: StdioEventLoop must be handled on higher level, can never reach invoke()".to_string())
            }
        }
    }
}
