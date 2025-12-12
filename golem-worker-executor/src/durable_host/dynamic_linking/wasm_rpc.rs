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

use crate::durable_host::wasm_rpc::{create_rpc_connection_span, WasmRpcEntryPayload};
use crate::model::WorkerConfig;
use crate::services::rpc::{RpcDemand, RpcError};
use crate::workerctx::WorkerCtx;
use anyhow::{anyhow, Context};
use golem_common::model::component::ComponentId;
use golem_common::model::component_metadata::{DynamicLinkedWasmRpc, InvokableFunction};
use golem_common::model::invocation_context::SpanId;
use golem_common::model::{OwnedWorkerId, WorkerId};
use golem_wasm::analysis::analysed_type::str;
use golem_wasm::analysis::{AnalysedResourceId, AnalysedResourceMode, AnalysedType, TypeHandle};
use golem_wasm::golem_rpc_0_2_x::types::{FutureInvokeResult, HostFutureInvokeResult};
use golem_wasm::wasmtime::{decode_param, encode_output, ResourceStore, ResourceTypeId};
use golem_wasm::{
    CancellationTokenEntry, HostWasmRpc, IntoValue, Uri, Value, WasmRpcEntry, WitValue,
};
use itertools::Itertools;
use rib::{ParsedFunctionName, ParsedFunctionReference};
use std::collections::HashMap;
use tracing::Instrument;
use uuid::Uuid;
use wasmtime::component::types::{ComponentInstance, ComponentItem};
use wasmtime::component::{LinkerInstance, Resource, ResourceType, Type, Val};
use wasmtime::{AsContextMut, Engine, StoreContextMut};
use wasmtime_wasi::IoView;

pub fn dynamic_wasm_rpc_link<
    Ctx: WorkerCtx
        + HostWasmRpc
        + HostFutureInvokeResult
        + wasmtime_wasi::p2::bindings::cli::environment::Host,
>(
    name: &str,
    rpc_metadata: &DynamicLinkedWasmRpc,
    engine: &Engine,
    root: &mut LinkerInstance<Ctx>,
    inst: &ComponentInstance,
) -> anyhow::Result<()> {
    let mut instance = root.instance(name)?;
    let mut resources: HashMap<(String, String), Vec<MethodInfo>> = HashMap::new();
    let mut functions = Vec::new();

    for (inner_name, inner_item) in inst.exports(engine) {
        let name = name.to_owned();
        let inner_name = inner_name.to_owned();

        match inner_item {
            ComponentItem::ComponentFunc(fun) => {
                let param_types: Vec<Type> = fun.params().map(|(_, t)| t).collect();
                let result_types: Vec<Type> = fun.results().collect();

                let function_name = ParsedFunctionName::parse(format!(
                    "{name}.{{{inner_name}}}"
                ))
                    .map_err(|err| anyhow!(format!("Unexpected linking error: {name}.{{{inner_name}}} is not a valid function name: {err}")))?;

                if let Some(resource_name) = function_name.function.resource_name() {
                    let methods = resources
                        .entry((name.clone(), resource_name.clone()))
                        .or_default();
                    methods.push(MethodInfo {
                        method_name: inner_name.clone(),
                        params: param_types.clone(),
                        results: result_types.clone(),
                    });
                }

                functions.push(FunctionInfo {
                    name: function_name,
                    params: param_types,
                    results: result_types,
                });
            }
            ComponentItem::CoreFunc(_) => {}
            ComponentItem::Module(_) => {}
            ComponentItem::Component(_) => {}
            ComponentItem::ComponentInstance(_) => {}
            ComponentItem::Type(_) => {}
            ComponentItem::Resource(_resource) => {
                resources.entry((name, inner_name)).or_default();
            }
        }
    }

    let mut resource_types = HashMap::new();
    for ((interface_name, resource_name), methods) in resources {
        let resource_type = DynamicRpcResource::analyse(&resource_name, &methods, rpc_metadata)?;

        if let Some(resource_type) = &resource_type {
            resource_types.insert(
                (interface_name.clone(), resource_name.clone()),
                resource_type.clone(),
            );
        }

        match resource_type {
            Some(DynamicRpcResource::InvokeResult) => {
                instance.resource(
                    &resource_name,
                    ResourceType::host::<FutureInvokeResult>(),
                    |_store, _rep| Ok(()),
                )?;
            }
            Some(DynamicRpcResource::Stub) | Some(DynamicRpcResource::ResourceStub) => {
                let target = rpc_metadata
                    .target(&resource_name)
                    .map_err(|err| anyhow!(err.clone()))?;
                let resource_name_clone = resource_name.clone();

                instance.resource_async(
                    &resource_name,
                    ResourceType::host::<WasmRpcEntry>(),
                    move |store, rep| {
                        let interface_name = target.interface_name.clone();
                        let resource_name = resource_name_clone.clone();

                        Box::new(
                            async move {
                                drop_linked_resource(store, rep, &interface_name, &resource_name)
                                    .await
                            }
                            .in_current_span(),
                        )
                    },
                )?;
            }
            None => {
                // Unsupported resource
            }
        }
    }

    for function in functions {
        let call_type = DynamicRpcCall::analyse(
            &function.name,
            &function.params,
            &function.results,
            rpc_metadata,
            &resource_types,
        )?;

        if let Some(call_type) = call_type {
            instance.func_new_async(
                &function.name.function.function_name(),
                move |store, params, results| {
                    let param_types = function.params.clone();
                    let result_types = function.results.clone();
                    let call_type = call_type.clone();
                    let function_name = function.name.clone();
                    Box::new(
                        async move {
                            dynamic_function_call(
                                store,
                                &function_name,
                                params,
                                &param_types,
                                results,
                                &result_types,
                                &call_type,
                            )
                            .await?;
                            Ok(())
                        }
                        .in_current_span(),
                    )
                },
            )?;
        } else {
            // Unsupported function
        }
    }

    Ok(())
}

async fn dynamic_function_call<
    Ctx: WorkerCtx
        + HostWasmRpc
        + HostFutureInvokeResult
        + wasmtime_wasi::p2::bindings::cli::environment::Host,
>(
    mut store: impl AsContextMut<Data = Ctx> + Send,
    function_name: &ParsedFunctionName,
    params: &[Val],
    param_types: &[Type],
    results: &mut [Val],
    result_types: &[Type],
    call_type: &DynamicRpcCall,
) -> anyhow::Result<()> {
    let mut store = store.as_context_mut();
    match call_type {
        DynamicRpcCall::GlobalStubConstructor { component_name, .. } => {
            // Simple stub interface constructor
            let target_worker_name = params[0].clone();
            let target_worker_id =
                resolve_default_worker_id(&mut store, component_name, target_worker_name).await?;
            let handle =
                HostWasmRpc::new(store.data_mut(), target_worker_id.worker_id().into()).await?;

            results[0] = Val::Resource(handle.try_into_resource_any(store)?);
        }
        DynamicRpcCall::GlobalCustomConstructor { .. } => {
            // Simple stub interface constructor that takes an agent-id as a parameter

            let worker_id = params[0].clone();
            let remote_worker_id = resolve_worker_id(&mut store, worker_id)?;
            let handle =
                HostWasmRpc::new(store.data_mut(), remote_worker_id.worker_id().into()).await?;

            results[0] = Val::Resource(handle.try_into_resource_any(store)?);
        }
        DynamicRpcCall::ResourceStubConstructor {
            target_constructor_name,
            component_name,
            ..
        } => {
            // Resource stub constructor

            // The first parameter is the target uri
            // Rest of the parameters must be sent to the remote constructor
            let target_worker_name = params[0].clone();
            let remote_worker_id =
                resolve_default_worker_id(&mut store, component_name, target_worker_name).await?;
            let handle =
                HostWasmRpc::new(store.data_mut(), remote_worker_id.worker_id().into()).await?;

            let remote_component_metadata = store
                .data()
                .component_service()
                .get_metadata(&remote_worker_id.worker_id.component_id, None)
                .await?
                .metadata;
            let constructor = remote_component_metadata.find_parsed_function(target_constructor_name)
                .map_err(|e| anyhow!("Failed to get target constructor metadata: {e}"))?
                .ok_or_else(|| anyhow!("Target constructor {target_constructor_name} not found in component metadata"))?;

            // First creating a resource for invoking the constructor (to avoid having to make a special case)
            let temp_handle = handle.rep();

            let mut analysed_param_types = constructor
                .analysed_export
                .parameters
                .iter()
                .map(|p| &p.typ)
                .collect::<Vec<_>>();
            analysed_param_types.insert(0, &str());

            let constructor_result = remote_invoke_and_await(
                target_constructor_name,
                params,
                param_types,
                &constructor
                    .analysed_export
                    .parameters
                    .iter()
                    .map(|p| &p.typ)
                    .collect::<Vec<_>>(),
                &mut store,
                handle,
            )
            .await?;

            let (resource_uri, resource_id) = unwrap_constructor_result(constructor_result)
                .context(format!("Unwrapping constructor result of {function_name}"))?;

            let span =
                create_rpc_connection_span(store.data_mut(), &remote_worker_id.worker_id).await?;

            let demand = create_demand(&mut store, &remote_worker_id, span.span_id()).await?;

            let handle = {
                let mut wasi = store.data_mut().as_wasi_view();
                let table = wasi.table();

                let temp_handle: Resource<WasmRpcEntry> = Resource::new_own(temp_handle);
                table.delete(temp_handle)?; // Removing the temporary handle

                table.push(WasmRpcEntry {
                    payload: Box::new(WasmRpcEntryPayload::Resource {
                        demand,
                        remote_worker_id,
                        resource_uri,
                        resource_id,
                        span_id: span.span_id().clone(),
                    }),
                })?
            };

            results[0] = Val::Resource(handle.try_into_resource_any(store)?);
        }
        DynamicRpcCall::ResourceCustomConstructor {
            target_constructor_name,
            ..
        } => {
            // Resource stub custom constructor

            // First parameter is the agent-id
            // Rest of the parameters must be sent to the remote constructor

            let worker_id = params[0].clone();
            let remote_worker_id = resolve_worker_id(&mut store, worker_id)?;
            let handle =
                HostWasmRpc::new(store.data_mut(), remote_worker_id.worker_id().into()).await?;

            let remote_component_metadata = store
                .data()
                .component_service()
                .get_metadata(&remote_worker_id.worker_id.component_id, None)
                .await?
                .metadata;
            let constructor = remote_component_metadata.find_parsed_function(target_constructor_name)
                .map_err(|e| anyhow!("Failed to get target constructor metadata: {e}"))?
                .ok_or_else(|| anyhow!("Target constructor {target_constructor_name} not found in component metadata"))?;

            // First creating a resource for invoking the constructor (to avoid having to make a special case)
            let temp_handle = handle.rep();

            let mut analysed_param_types = constructor
                .analysed_export
                .parameters
                .iter()
                .map(|p| &p.typ)
                .collect::<Vec<_>>();

            let worker_id_type = WorkerId::get_type();
            analysed_param_types.insert(0, &worker_id_type);

            let constructor_result = remote_invoke_and_await(
                target_constructor_name,
                params,
                param_types,
                &analysed_param_types,
                &mut store,
                handle,
            )
            .await?;

            let (resource_uri, resource_id) = unwrap_constructor_result(constructor_result)
                .context(format!("Unwrapping constructor result of {function_name}"))?;

            let span =
                create_rpc_connection_span(store.data_mut(), &remote_worker_id.worker_id).await?;

            let demand = create_demand(&mut store, &remote_worker_id, span.span_id()).await?;

            let handle = {
                let mut wasi = store.data_mut().as_wasi_view();
                let table = wasi.table();

                let temp_handle: Resource<WasmRpcEntry> = Resource::new_own(temp_handle);
                table.delete(temp_handle)?; // Removing the temporary handle

                table.push(WasmRpcEntry {
                    payload: Box::new(WasmRpcEntryPayload::Resource {
                        demand,
                        remote_worker_id,
                        resource_uri,
                        resource_id,
                        span_id: span.span_id().clone(),
                    }),
                })?
            };

            results[0] = Val::Resource(handle.try_into_resource_any(store)?);
        }
        DynamicRpcCall::BlockingFunctionCall {
            target_function_name,
            ..
        } => {
            // Simple stub interface method
            let handle = match params[0] {
                Val::Resource(handle) => handle,
                _ => return Err(anyhow!("Invalid parameter for {function_name} - it must be a resource handle but it is {:?}", params[0])),
            };
            let handle: Resource<WasmRpcEntry> = handle.try_into_resource(&mut store)?;

            let target_component_metadata = {
                let remote_worker_id = {
                    let mut wasi = store.data_mut().as_wasi_view();
                    let entry = wasi.table().get(&handle)?;
                    let payload = entry.payload.downcast_ref::<WasmRpcEntryPayload>().unwrap();
                    payload.remote_worker_id().clone()
                };
                store
                    .data()
                    .component_service()
                    .get_metadata(&remote_worker_id.worker_id.component_id, None)
                    .await?
                    .metadata
            };
            let target_function_metadata = target_component_metadata
                .find_parsed_function(target_function_name)
                .map_err(|e| anyhow!("Failed to get target function metadata: {e}"))?
                .ok_or_else(|| {
                    anyhow!(
                        "Target function {target_function_name} not found in component metadata"
                    )
                })?;

            let analysed_param_types =
                get_analysed_param_types_on_rpc_resource(&target_function_metadata);

            let result = remote_invoke_and_await(
                target_function_name,
                params,
                param_types,
                &analysed_param_types,
                &mut store,
                handle,
            )
            .await?;
            value_result_to_wasmtime_vals(result, results, result_types, &mut store)
                .await
                .context(format!("In {function_name}, decoding result value of remote {target_function_name} call"))?;
        }
        DynamicRpcCall::FireAndForgetFunctionCall {
            target_function_name,
            ..
        } => {
            // Fire-and-forget stub interface method
            let handle = match params[0] {
                Val::Resource(handle) => handle,
                _ => return Err(anyhow!("Invalid parameter for {function_name} - it must be a resource handle but it is {:?}", params[0])),
            };
            let handle: Resource<WasmRpcEntry> = handle.try_into_resource(&mut store)?;

            let target_component_metadata = {
                let remote_worker_id = {
                    let mut wasi = store.data_mut().as_wasi_view();
                    let entry = wasi.table().get(&handle)?;
                    let payload = entry.payload.downcast_ref::<WasmRpcEntryPayload>().unwrap();
                    payload.remote_worker_id().clone()
                };
                store
                    .data()
                    .component_service()
                    .get_metadata(&remote_worker_id.worker_id.component_id, None)
                    .await?
                    .metadata
            };
            let target_function_metadata = target_component_metadata
                .find_parsed_function(target_function_name)
                .map_err(|e| anyhow!("Failed to get target function metadata: {e}"))?
                .ok_or_else(|| {
                    anyhow!(
                        "Target function {target_function_name} not found in component metadata"
                    )
                })?;

            let analysed_param_types =
                get_analysed_param_types_on_rpc_resource(&target_function_metadata);

            remote_invoke(
                target_function_name,
                params,
                param_types,
                &analysed_param_types,
                &mut store,
                handle,
            )
            .await?;
        }
        DynamicRpcCall::ScheduledFunctionCall {
            target_function_name,
            ..
        } => {
            // scheduled function call
            let handle = match params[0] {
                Val::Resource(handle) => handle,
                _ => return Err(anyhow!("Invalid parameter for {function_name} - it must be a resource handle but it is {:?}", params[0])),
            };
            let handle: Resource<WasmRpcEntry> = handle.try_into_resource(&mut store)?;

            let target_component_metadata = {
                let remote_worker_id = {
                    let mut wasi = store.data_mut().as_wasi_view();
                    let entry = wasi.table().get(&handle)?;
                    let payload = entry.payload.downcast_ref::<WasmRpcEntryPayload>().unwrap();
                    payload.remote_worker_id().clone()
                };
                store
                    .data()
                    .component_service()
                    .get_metadata(&remote_worker_id.worker_id.component_id, None)
                    .await?
                    .metadata
            };
            let target_function_metadata = target_component_metadata
                .find_parsed_function(target_function_name)
                .map_err(|e| anyhow!("Failed to get target function metadata: {e}"))?
                .ok_or_else(|| {
                    anyhow!(
                        "Target function {target_function_name} not found in component metadata"
                    )
                })?;
            let analysed_param_types =
                get_analysed_param_types_on_rpc_resource(&target_function_metadata);

            // function should have at least one parameter for the scheduled_for datetime.
            if !(!params.is_empty() && !param_types.is_empty()) {
                Err(anyhow!(
                    "Function did not have any parameters. Expected at least scheduled_for"
                ))?
            };

            let scheduled_for = val_to_datetime(params.last().unwrap().clone())?;

            let cancellation_token = schedule_remote_invocation(
                scheduled_for,
                target_function_name,
                &params[..params.len() - 1],
                &param_types[..param_types.len() - 1],
                &analysed_param_types[..param_types.len() - 1],
                &mut store,
                handle,
            )
            .await?;

            results[0] = Val::Resource(cancellation_token.try_into_resource_any(store)?);
        }
        DynamicRpcCall::AsyncFunctionCall {
            target_function_name,
            ..
        } => {
            // Async stub interface method
            let handle = match params[0] {
                Val::Resource(handle) => handle,
                _ => return Err(anyhow!("Invalid parameter for {function_name} - it must be a resource handle but it is {:?}", params[0])),
            };
            let handle: Resource<WasmRpcEntry> = handle.try_into_resource(&mut store)?;

            let target_component_metadata = {
                let remote_worker_id = {
                    let mut wasi = store.data_mut().as_wasi_view();
                    let entry = wasi.table().get(&handle)?;
                    let payload = entry.payload.downcast_ref::<WasmRpcEntryPayload>().unwrap();
                    payload.remote_worker_id().clone()
                };
                store
                    .data()
                    .component_service()
                    .get_metadata(&remote_worker_id.worker_id.component_id, None)
                    .await?
                    .metadata
            };
            let target_function_metadata = target_component_metadata
                .find_parsed_function(target_function_name)
                .map_err(|e| anyhow!("Failed to get target function metadata: {e}"))?
                .ok_or_else(|| {
                    anyhow!(
                        "Target function {target_function_name} not found in component metadata"
                    )
                })?;

            let analysed_param_types =
                get_analysed_param_types_on_rpc_resource(&target_function_metadata);

            let result = remote_async_invoke_and_await(
                target_function_name,
                params,
                param_types,
                &analysed_param_types,
                &mut store,
                handle,
            )
            .await?;

            value_result_to_wasmtime_vals(result, results, result_types, &mut store)
                .await
                .context(format!("In {function_name}, decoding result value of remote {target_function_name} call"))?;
        }
        DynamicRpcCall::FutureInvokeResultSubscribe => {
            let handle = match params[0] {
                Val::Resource(handle) => handle,
                _ => return Err(anyhow!("Invalid parameter for {function_name} - it must be a resource handle but it is {:?}", params[0])),
            };
            let handle: Resource<FutureInvokeResult> = handle.try_into_resource(&mut store)?;
            let pollable = store.data_mut().subscribe(handle).await?;
            let pollable_any = pollable.try_into_resource_any(&mut store)?;
            let resource_id = store
                .data_mut()
                .add(
                    pollable_any,
                    ResourceTypeId {
                        owner: "wasi:io@0.2.3/poll".to_string(), // TODO: move this to some constant so it's easier to get updated
                        name: "pollable".to_string(),
                    },
                )
                .await;

            let value_result = Value::Tuple(vec![Value::Handle {
                uri: store.data().self_uri().value,
                resource_id,
            }]);
            value_result_to_wasmtime_vals(value_result, results, result_types, &mut store)
                .await
                .context(format!("In {function_name}, decoding the result value"))?;
        }
        DynamicRpcCall::FutureInvokeResultGet => {
            let handle = match params[0] {
                Val::Resource(handle) => handle,
                _ => return Err(anyhow!("Invalid parameter for {function_name} - it must be a resource handle but it is {:?}", params[0])),
            };
            let handle: Resource<FutureInvokeResult> = handle.try_into_resource(&mut store)?;
            let result = HostFutureInvokeResult::get(store.data_mut(), handle).await?;

            // NOTE: we are currently failing on RpcError instead of passing it to the caller, as the generated stub interface requires
            let value_result = Value::Tuple(vec![match result {
                None => Value::Option(None),
                Some(Ok(value)) => {
                    let value: Value = value.into();
                    match value {
                        Value::Tuple(items) if items.len() == 1 => {
                            Value::Option(Some(Box::new(items.into_iter().next().unwrap())))
                        }
                        _ => Err(anyhow!("Invalid future invoke result value in {function_name} - expected a tuple with a single item, got {value:?}"))?,
                    }
                }
                Some(Err(err)) => {
                    let rpc_error: RpcError = err.into();
                    Err(anyhow!(
                        "RPC invocation of {function_name} failed with: {rpc_error}"
                    ))?
                }
            }]);

            value_result_to_wasmtime_vals(value_result, results, result_types, &mut store)
                .await
                .context(format!("In {function_name}, decoding the result value"))?;
        }
    }

    Ok(())
}

fn get_analysed_param_types_on_rpc_resource(
    target_function_metadata: &InvokableFunction,
) -> Vec<&AnalysedType> {
    let mut analysed_param_types = target_function_metadata
        .analysed_export
        .parameters
        .iter()
        .map(|p| &p.typ)
        .collect::<Vec<_>>();

    analysed_param_types.insert(
        0,
        &AnalysedType::Handle(TypeHandle {
            name: None,
            owner: None,
            resource_id: AnalysedResourceId(u64::MAX), // NOTE: this is a fake value but currently it does not cause any issues
            mode: AnalysedResourceMode::Borrowed,
        }),
    );

    analysed_param_types
}

fn unwrap_constructor_result(constructor_result: Value) -> anyhow::Result<(Uri, u64)> {
    if let Value::Tuple(values) = constructor_result {
        if values.len() == 1 {
            if let Value::Handle { uri, resource_id } = values.into_iter().next().unwrap() {
                Ok((Uri { value: uri }, resource_id))
            } else {
                Err(anyhow!(
                    "Invalid constructor result: single handle expected"
                ))
            }
        } else {
            Err(anyhow!(
                "Invalid constructor result: single result value expected, but got {}",
                values.len()
            ))
        }
    } else {
        Err(anyhow!(
                "Invalid constructor result: a tuple with a single field expected, but got {constructor_result:?}"
            ))
    }
}

async fn drop_linked_resource<Ctx: WorkerCtx + HostWasmRpc + HostFutureInvokeResult>(
    mut store: StoreContextMut<'_, Ctx>,
    rep: u32,
    interface_name: &str,
    resource_name: &str,
) -> anyhow::Result<()> {
    let (must_invoke_remote_drop, span_id) = {
        let mut wasi = store.data_mut().as_wasi_view();
        let table = wasi.table();
        if let Some(entry) = table.get_any_mut(rep)?.downcast_ref::<WasmRpcEntry>() {
            let payload = entry.payload.downcast_ref::<WasmRpcEntryPayload>().unwrap();
            let span_id = payload.span_id();

            (
                matches!(payload, WasmRpcEntryPayload::Resource { .. }),
                Some(span_id.clone()),
            )
        } else {
            (false, None)
        }
    };

    if must_invoke_remote_drop {
        let resource: Resource<WasmRpcEntry> = Resource::new_own(rep);

        let function_name = format!("{interface_name}.{{{resource_name}.drop}}");
        let _ = store
            .data_mut()
            .invoke_and_await(resource, function_name, vec![])
            .await?;
    }

    if let Some(span_id) = span_id {
        store.data_mut().finish_span(&span_id).await?;
    }

    Ok(())
}

async fn encode_parameters<Ctx: ResourceStore + Send>(
    params: &[Val],
    param_types: &[Type],
    analysed_parameter_types: &[&AnalysedType],
    store: &mut StoreContextMut<'_, Ctx>,
) -> anyhow::Result<Vec<WitValue>> {
    let mut wit_value_params = Vec::new();
    for (idx, ((param, typ), analysed_type)) in params
        .iter()
        .zip(param_types)
        .zip(analysed_parameter_types)
        .enumerate()
        .skip(1)
    {
        let value: Value = encode_output(param, typ, analysed_type, store.data_mut())
            .await
            .map_err(|err| anyhow!(format!("Failed to encode parameter {idx}: {err}")))?;
        let wit_value: WitValue = value.into();
        wit_value_params.push(wit_value);
    }
    Ok(wit_value_params)
}

async fn remote_invoke_and_await<Ctx: WorkerCtx + HostWasmRpc + HostFutureInvokeResult>(
    target_function_name: &ParsedFunctionName,
    params: &[Val],
    param_types: &[Type],
    analysed_param_types: &[&AnalysedType],
    store: &mut StoreContextMut<'_, Ctx>,
    handle: Resource<WasmRpcEntry>,
) -> anyhow::Result<Value> {
    let wit_value_params = encode_parameters(params, param_types, analysed_param_types, store)
        .await
        .context(format!("Encoding parameters of {target_function_name}"))?;

    let wit_value_result = store
        .data_mut()
        .invoke_and_await(handle, target_function_name.to_string(), wit_value_params)
        .await??;

    let value_result: Value = wit_value_result.into();
    Ok(value_result)
}

async fn remote_async_invoke_and_await<Ctx: WorkerCtx + HostWasmRpc + HostFutureInvokeResult>(
    target_function_name: &ParsedFunctionName,
    params: &[Val],
    param_types: &[Type],
    analysed_param_types: &[&AnalysedType],
    store: &mut StoreContextMut<'_, Ctx>,
    handle: Resource<WasmRpcEntry>,
) -> anyhow::Result<Value> {
    let wit_value_params = encode_parameters(params, param_types, analysed_param_types, store)
        .await
        .context(format!("Encoding parameters of {target_function_name}"))?;

    let invoke_result_resource = store
        .data_mut()
        .async_invoke_and_await(handle, target_function_name.to_string(), wit_value_params)
        .await?;

    let invoke_result_resource_any = invoke_result_resource.try_into_resource_any(&mut *store)?;
    let resource_id = store
        .data_mut()
        .add(
            invoke_result_resource_any,
            ResourceTypeId {
                owner: "golem:rpc@0.2.2/future-invoke-result".to_string(), // TODO: move this to some constant so it's easier to get updated
                name: "future-invoke-result".to_string(),
            },
        )
        .await;

    let value_result: Value = Value::Tuple(vec![Value::Handle {
        uri: store.data().self_uri().value,
        resource_id,
    }]);
    Ok(value_result)
}

async fn remote_invoke<Ctx: WorkerCtx + HostWasmRpc + HostFutureInvokeResult>(
    target_function_name: &ParsedFunctionName,
    params: &[Val],
    param_types: &[Type],
    analysed_param_types: &[&AnalysedType],
    store: &mut StoreContextMut<'_, Ctx>,
    handle: Resource<WasmRpcEntry>,
) -> anyhow::Result<()> {
    let wit_value_params = encode_parameters(params, param_types, analysed_param_types, store)
        .await
        .context(format!("Encoding parameters of {target_function_name}"))?;

    store
        .data_mut()
        .invoke(handle, target_function_name.to_string(), wit_value_params)
        .await??;

    Ok(())
}

async fn schedule_remote_invocation<Ctx: WorkerCtx + HostWasmRpc + HostFutureInvokeResult>(
    scheduled_for: golem_wasm::wasi::clocks::wall_clock::Datetime,
    target_function_name: &ParsedFunctionName,
    params: &[Val],
    param_types: &[Type],
    analysed_param_types: &[&AnalysedType],
    store: &mut StoreContextMut<'_, Ctx>,
    handle: Resource<WasmRpcEntry>,
) -> anyhow::Result<Resource<CancellationTokenEntry>> {
    let wit_value_params = encode_parameters(params, param_types, analysed_param_types, store)
        .await
        .context(format!("Encoding parameters of {target_function_name}"))?;

    store
        .data_mut()
        .schedule_cancelable_invocation(
            handle,
            scheduled_for,
            target_function_name.to_string(),
            wit_value_params,
        )
        .await
}

async fn value_result_to_wasmtime_vals<Ctx: ResourceStore + Send>(
    value_result: Value,
    results: &mut [Val],
    result_types: &[Type],
    store: &mut StoreContextMut<'_, Ctx>,
) -> anyhow::Result<()> {
    match value_result {
        Value::Tuple(values) | Value::Record(values) => {
            for (idx, (value, typ)) in values.iter().zip(result_types).enumerate() {
                let result = decode_param(value, typ, store.data_mut())
                    .await
                    .map_err(|err| {
                        anyhow!(format!("Failed to decode result value {idx}: {err}"))
                    })?;
                results[idx] = result.val;
            }
        }
        _ => {
            return Err(anyhow!(
                "Unexpected result value {value_result:?}, expected tuple or record"
            ));
        }
    }

    Ok(())
}

fn val_to_datetime(val: Val) -> anyhow::Result<golem_wasm::wasi::clocks::wall_clock::Datetime> {
    let fields = match val {
        Val::Record(inner) => inner.into_iter().collect::<HashMap<String, _>>(),
        _ => Err(anyhow!("did not find a record value"))?,
    };

    let seconds = match fields
        .get("seconds")
        .ok_or(anyhow!("did not find seconds field"))?
    {
        Val::U64(value) => *value,
        _ => Err(anyhow!("seconds field has invalid type"))?,
    };

    let nanoseconds = match fields
        .get("nanoseconds")
        .ok_or(anyhow!("did not find nanoseconds field"))?
    {
        Val::U32(value) => *value,
        _ => Err(anyhow!("nanoseconds field has invalid type"))?,
    };

    Ok(golem_wasm::wasi::clocks::wall_clock::Datetime {
        seconds,
        nanoseconds,
    })
}

async fn resolve_default_worker_id<Ctx: WorkerCtx>(
    store: &mut StoreContextMut<'_, Ctx>,
    component_name: &str,
    worker_name: Val,
) -> anyhow::Result<OwnedWorkerId> {
    let worker_name = match worker_name {
        Val::String(name) => name,
        _ => return Err(anyhow!("Missing or invalid worker name parameter. Expected to get a string worker name as constructor parameter, got {worker_name:?}")),
    };

    let result = store
        .data()
        .component_service()
        .resolve_component(
            component_name.to_string(),
            store.data().component_metadata().environment_id,
            store.data().component_metadata().application_id,
            store.data().component_metadata().account_id,
        )
        .await?;

    if let Some(component_id) = result {
        let remote_worker_id = WorkerId {
            component_id,
            worker_name,
        };
        let remote_worker_id = OwnedWorkerId::new(
            &store.data().owned_worker_id().environment_id,
            &remote_worker_id,
        );
        Ok(remote_worker_id)
    } else {
        Err(anyhow!("Failed to resolve component {component_name}"))
    }
}

fn decode_component_id(component_id: Val) -> Option<ComponentId> {
    match component_id {
        Val::Record(component_id_fields) if component_id_fields.len() == 1 => {
            match &component_id_fields[0].1 {
                Val::Record(uuid_fields) if uuid_fields.len() == 2 => {
                    match (&uuid_fields[0].1, &uuid_fields[1].1) {
                        (Val::U64(hibits), Val::U64(lobits)) => {
                            Some(ComponentId(Uuid::from_u64_pair(*hibits, *lobits)))
                        }
                        _ => None,
                    }
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn decode_worker_id(worker_id: Val) -> Option<WorkerId> {
    match worker_id {
        Val::Record(worker_id_fields) if worker_id_fields.len() == 2 => {
            let component_id = decode_component_id(worker_id_fields[0].1.clone())?;
            match &worker_id_fields[1].1 {
                Val::String(name) => Some(WorkerId {
                    component_id,
                    worker_name: name.clone(),
                }),
                _ => None,
            }
        }
        _ => None,
    }
}

fn resolve_worker_id<Ctx: WorkerCtx>(
    store: &mut StoreContextMut<'_, Ctx>,
    worker_id: Val,
) -> anyhow::Result<OwnedWorkerId> {
    let remote_worker_id = decode_worker_id(worker_id.clone()).ok_or_else(|| anyhow!("Missing or invalid worker id parameter. Expected to get an agent-id value as a custom constructor parameter, got {worker_id:?}"))?;

    let remote_worker_id = OwnedWorkerId::new(
        &store.data().owned_worker_id().environment_id,
        &remote_worker_id,
    );
    Ok(remote_worker_id)
}

async fn create_demand<Ctx: WorkerCtx + wasmtime_wasi::p2::bindings::cli::environment::Host>(
    store: &mut StoreContextMut<'_, Ctx>,
    remote_worker_id: &OwnedWorkerId,
    span_id: &SpanId,
) -> anyhow::Result<Box<dyn RpcDemand>> {
    let self_created_by = *store.data().created_by();
    let self_worker_id = store.data().owned_worker_id().worker_id();

    let mut env = store.data_mut().get_environment().await?;
    WorkerConfig::remove_dynamic_vars(&mut env);

    let config = store.data().wasi_config_vars();
    let stack = store.data().clone_as_inherited_stack(span_id);
    let demand = store
        .data()
        .rpc()
        .create_demand(
            remote_worker_id,
            &self_created_by,
            &self_worker_id,
            &env,
            config,
            stack,
        )
        .await?;

    Ok(demand)
}

#[derive(Debug, Clone)]
enum DynamicRpcCall {
    GlobalStubConstructor {
        component_name: String,
    },
    GlobalCustomConstructor {},
    ResourceStubConstructor {
        component_name: String,
        target_constructor_name: ParsedFunctionName,
    },
    ResourceCustomConstructor {
        target_constructor_name: ParsedFunctionName,
    },
    BlockingFunctionCall {
        target_function_name: ParsedFunctionName,
    },
    ScheduledFunctionCall {
        target_function_name: ParsedFunctionName,
    },
    FireAndForgetFunctionCall {
        target_function_name: ParsedFunctionName,
    },
    AsyncFunctionCall {
        target_function_name: ParsedFunctionName,
    },
    FutureInvokeResultSubscribe,
    FutureInvokeResultGet,
}

impl DynamicRpcCall {
    pub fn analyse(
        stub_name: &ParsedFunctionName,
        _param_types: &[Type],
        result_types: &[Type],
        rpc_metadata: &DynamicLinkedWasmRpc,
        resource_types: &HashMap<(String, String), DynamicRpcResource>,
    ) -> anyhow::Result<Option<DynamicRpcCall>> {
        fn context(rpc_metadata: &DynamicLinkedWasmRpc) -> String {
            format!(
                "Failed to get mapped target site ({}) from dynamic linking metadata",
                rpc_metadata
                    .targets
                    .iter()
                    .map(|(k, v)| format!("{k}=>{v}"))
                    .collect::<Vec<_>>()
                    .join(", "),
            )
        }

        if let Some(resource_name) = stub_name.is_constructor() {
            match resource_types.get(&(
                stub_name.site.interface_name().unwrap_or_default(),
                resource_name.to_string(),
            )) {
                Some(DynamicRpcResource::Stub) => {
                    let target = rpc_metadata
                        .target(resource_name)
                        .map_err(|err| anyhow!(err))
                        .context(context(rpc_metadata))?;

                    Ok(Some(DynamicRpcCall::GlobalStubConstructor {
                        component_name: target.component_name,
                    }))
                }
                Some(DynamicRpcResource::ResourceStub) => {
                    let target = rpc_metadata
                        .target(resource_name)
                        .map_err(|err| anyhow!(err))
                        .context(context(rpc_metadata))?;

                    Ok(Some(DynamicRpcCall::ResourceStubConstructor {
                        target_constructor_name: ParsedFunctionName {
                            site: target
                                .site()
                                .map_err(|err| anyhow!(err))
                                .context(context(rpc_metadata))?,
                            function: ParsedFunctionReference::RawResourceConstructor {
                                resource: resource_name.to_string(),
                            },
                        },
                        component_name: target.component_name,
                    }))
                }
                _ => Ok(None),
            }
        } else if let Some(resource_name) = stub_name.is_method() {
            match resource_types.get(&(
                stub_name.site.interface_name().unwrap_or_default(),
                resource_name.to_string(),
            )) {
                Some(DynamicRpcResource::InvokeResult) => {
                    if stub_name.function.resource_method_name() == Some("subscribe".to_string()) {
                        Ok(Some(DynamicRpcCall::FutureInvokeResultSubscribe))
                    } else if stub_name.function.resource_method_name() == Some("get".to_string()) {
                        Ok(Some(DynamicRpcCall::FutureInvokeResultGet))
                    } else {
                        Ok(None)
                    }
                }
                Some(stub) => {
                    let method_name = stub_name.function.resource_method_name().unwrap(); // safe because of stub_name.is_method()

                    if stub_name.is_static_method().is_some() && method_name == "custom" {
                        match stub {
                            DynamicRpcResource::Stub => {
                                Ok(Some(DynamicRpcCall::GlobalCustomConstructor {}))
                            }
                            DynamicRpcResource::ResourceStub => {
                                let target = rpc_metadata
                                    .target(resource_name)
                                    .map_err(|err| anyhow!(err))
                                    .context(context(rpc_metadata))?;

                                Ok(Some(DynamicRpcCall::ResourceCustomConstructor {
                                    target_constructor_name: ParsedFunctionName {
                                        site: target
                                            .site()
                                            .map_err(|err| anyhow!(err))
                                            .context(context(rpc_metadata))?,
                                        function: ParsedFunctionReference::RawResourceConstructor {
                                            resource: resource_name.to_string(),
                                        },
                                    },
                                }))
                            }
                            DynamicRpcResource::InvokeResult => {
                                unreachable!()
                            }
                        }
                    } else {
                        let blocking = method_name.starts_with("blocking-");
                        let scheduled = method_name.starts_with("schedule-");

                        let target_method_name = if blocking {
                            method_name
                                .strip_prefix("blocking-")
                                .unwrap_or(&method_name)
                        } else if scheduled {
                            method_name
                                .strip_prefix("schedule-")
                                .unwrap_or(&method_name)
                        } else {
                            &method_name
                        };

                        let target_function = match stub {
                            DynamicRpcResource::Stub => ParsedFunctionReference::Function {
                                function: target_method_name.to_string(),
                            },
                            _ => ParsedFunctionReference::RawResourceMethod {
                                resource: resource_name.to_string(),
                                method: target_method_name.to_string(),
                            },
                        };

                        let target = rpc_metadata
                            .target(resource_name)
                            .map_err(|err| anyhow!(err))
                            .context(context(rpc_metadata))?;

                        let target_function_name = ParsedFunctionName {
                            site: target
                                .site()
                                .map_err(|err| anyhow!(err))
                                .context(context(rpc_metadata))?,
                            function: target_function,
                        };

                        if blocking {
                            Ok(Some(DynamicRpcCall::BlockingFunctionCall {
                                target_function_name,
                            }))
                        } else if scheduled {
                            Ok(Some(DynamicRpcCall::ScheduledFunctionCall {
                                target_function_name,
                            }))
                        } else if !result_types.is_empty() {
                            Ok(Some(DynamicRpcCall::AsyncFunctionCall {
                                target_function_name,
                            }))
                        } else {
                            Ok(Some(DynamicRpcCall::FireAndForgetFunctionCall {
                                target_function_name,
                            }))
                        }
                    }
                }
                None => Ok(None),
            }
        } else {
            // Unsupported item
            Ok(None)
        }
    }
}

#[derive(Debug, Clone)]
enum DynamicRpcResource {
    Stub,
    ResourceStub,
    InvokeResult,
}

impl DynamicRpcResource {
    pub fn analyse(
        resource_name: &str,
        methods: &[MethodInfo],
        rpc_metadata: &DynamicLinkedWasmRpc,
    ) -> anyhow::Result<Option<DynamicRpcResource>> {
        if resource_name == "pollable" {
            Ok(None)
        } else if Self::is_invoke_result(resource_name, methods) {
            Ok(Some(DynamicRpcResource::InvokeResult))
        } else if let Some(_constructor) = methods
            .iter()
            .find_or_first(|m| m.method_name.contains("[constructor]"))
        {
            if let Some(target) = rpc_metadata.targets.get(resource_name) {
                if target
                    .interface_name
                    .ends_with(&format!("/{resource_name}"))
                {
                    Ok(Some(DynamicRpcResource::Stub))
                } else {
                    Ok(Some(DynamicRpcResource::ResourceStub))
                }
            } else {
                Ok(None)
            }
        } else {
            // No constructor => not a stub
            Ok(None)
        }
    }

    fn is_invoke_result(resource_name: &str, methods: &[MethodInfo]) -> bool {
        resource_name.starts_with("future-")
            && resource_name.ends_with("-result")
            && methods
                .iter()
                .filter_map(|m| m.method_name.split('.').next_back().map(|s| s.to_string()))
                .sorted()
                .collect::<Vec<_>>()
                == vec!["get".to_string(), "subscribe".to_string()]
            && {
                let subscribe = methods
                    .iter()
                    .find(|m| m.method_name.ends_with(".subscribe"))
                    .unwrap();
                subscribe.params.len() == 1
                    && matches!(subscribe.params[0], Type::Borrow(_))
                    && subscribe.results.len() == 1
                    && matches!(subscribe.results[0], Type::Own(_))
            }
    }
}

struct MethodInfo {
    method_name: String,
    params: Vec<Type>,
    results: Vec<Type>,
}

struct FunctionInfo {
    name: ParsedFunctionName,
    params: Vec<Type>,
    results: Vec<Type>,
}
