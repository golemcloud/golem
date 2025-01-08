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

use crate::durable_host::wasm_rpc::{UrnExtensions, WasmRpcEntryPayload};
use crate::services::rpc::{RpcDemand, RpcError};
use crate::workerctx::WorkerCtx;
use anyhow::{anyhow, Context};
use golem_common::model::component_metadata::DynamicLinkedWasmRpc;
use golem_common::model::OwnedWorkerId;
use golem_wasm_rpc::golem::rpc::types::{FutureInvokeResult, HostFutureInvokeResult};
use golem_wasm_rpc::wasmtime::{decode_param, encode_output, ResourceStore};
use golem_wasm_rpc::{HostWasmRpc, Uri, Value, WasmRpcEntry, WitValue};
use itertools::Itertools;
use rib::{ParsedFunctionName, ParsedFunctionReference};
use std::collections::HashMap;
use tracing::Instrument;
use wasmtime::component::types::{ComponentInstance, ComponentItem, Field};
use wasmtime::component::{LinkerInstance, Resource, ResourceType, Type, Val};
use wasmtime::{AsContextMut, Engine, StoreContextMut};
use wasmtime_wasi::WasiView;

pub fn dynamic_wasm_rpc_link<Ctx: WorkerCtx + HostWasmRpc + HostFutureInvokeResult>(
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
                let param_types: Vec<Type> = fun.params().collect();
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
                let interface_name = rpc_metadata
                    .target_interface_name
                    .get(&resource_name)
                    .cloned()
                    .ok_or(anyhow!(
                        "Failed to get target interface name for resource {resource_name}"
                    ))?;
                let resource_name_clone = resource_name.clone();

                instance.resource_async(
                    &resource_name,
                    ResourceType::host::<WasmRpcEntry>(),
                    move |store, rep| {
                        let interface_name = interface_name.clone();
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

fn register_wasm_rpc_entry<Ctx: WorkerCtx>(
    store: &mut StoreContextMut<'_, Ctx>,
    remote_worker_id: OwnedWorkerId,
    demand: Box<dyn RpcDemand>,
) -> anyhow::Result<Resource<WasmRpcEntry>> {
    let mut wasi = store.data_mut().as_wasi_view();
    let table = wasi.table();
    Ok(table.push(WasmRpcEntry {
        payload: Box::new(WasmRpcEntryPayload::Interface {
            demand,
            remote_worker_id,
        }),
    })?)
}

async fn dynamic_function_call<Ctx: WorkerCtx + HostWasmRpc + HostFutureInvokeResult>(
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
        DynamicRpcCall::GlobalStubConstructor => {
            // Simple stub interface constructor

            let target_worker_urn = params[0].clone();
            let (remote_worker_id, demand) =
                create_rpc_target(&mut store, target_worker_urn).await?;

            let handle = register_wasm_rpc_entry(&mut store, remote_worker_id, demand)?;
            results[0] = Val::Resource(handle.try_into_resource_any(store)?);
        }
        DynamicRpcCall::ResourceStubConstructor {
            target_constructor_name,
            ..
        } => {
            // Resource stub constructor

            // First parameter is the target uri
            // Rest of the parameters must be sent to the remote constructor

            let target_worker_urn = params[0].clone();
            let (remote_worker_id, demand) =
                create_rpc_target(&mut store, target_worker_urn.clone()).await?;

            // First creating a resource for invoking the constructor (to avoid having to make a special case)
            let handle = register_wasm_rpc_entry(&mut store, remote_worker_id, demand)?;
            let temp_handle = handle.rep();

            let constructor_result = remote_invoke_and_wait(
                target_constructor_name,
                params,
                param_types,
                &mut store,
                handle,
            )
            .await?;

            let (resource_uri, resource_id) = unwrap_constructor_result(constructor_result)
                .context(format!("Unwrapping constructor result of {function_name}"))?;

            let (remote_worker_id, demand) =
                create_rpc_target(&mut store, target_worker_urn).await?;

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

            let result = remote_invoke_and_wait(
                target_function_name,
                params,
                param_types,
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

            remote_invoke(
                target_function_name,
                params,
                param_types,
                &mut store,
                handle,
            )
            .await?;
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

            let result = remote_async_invoke_and_await(
                target_function_name,
                params,
                param_types,
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
            let resource_id = store.data_mut().add(pollable_any).await;

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
    let must_drop = {
        let mut wasi = store.data_mut().as_wasi_view();
        let table = wasi.table();
        if let Some(entry) = table.get_any_mut(rep)?.downcast_ref::<WasmRpcEntry>() {
            let payload = entry.payload.downcast_ref::<WasmRpcEntryPayload>().unwrap();

            matches!(payload, WasmRpcEntryPayload::Resource { .. })
        } else {
            false
        }
    };
    if must_drop {
        let resource: Resource<WasmRpcEntry> = Resource::new_own(rep);

        let function_name = format!("{interface_name}.{{{resource_name}.drop}}");
        let _ = store
            .data_mut()
            .invoke_and_await(resource, function_name, vec![])
            .await?;
    }
    Ok(())
}

async fn encode_parameters<Ctx: ResourceStore + Send>(
    params: &[Val],
    param_types: &[Type],
    store: &mut StoreContextMut<'_, Ctx>,
) -> anyhow::Result<Vec<WitValue>> {
    let mut wit_value_params = Vec::new();
    for (idx, (param, typ)) in params.iter().zip(param_types).enumerate().skip(1) {
        let value: Value = encode_output(param, typ, store.data_mut())
            .await
            .map_err(|err| anyhow!(format!("Failed to encode parameter {idx}: {err}")))?;
        let wit_value: WitValue = value.into();
        wit_value_params.push(wit_value);
    }
    Ok(wit_value_params)
}

async fn remote_invoke_and_wait<Ctx: WorkerCtx + HostWasmRpc + HostFutureInvokeResult>(
    target_function_name: &ParsedFunctionName,
    params: &[Val],
    param_types: &[Type],
    store: &mut StoreContextMut<'_, Ctx>,
    handle: Resource<WasmRpcEntry>,
) -> anyhow::Result<Value> {
    let wit_value_params = encode_parameters(params, param_types, store)
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
    store: &mut StoreContextMut<'_, Ctx>,
    handle: Resource<WasmRpcEntry>,
) -> anyhow::Result<Value> {
    let wit_value_params = encode_parameters(params, param_types, store)
        .await
        .context(format!("Encoding parameters of {target_function_name}"))?;

    let invoke_result_resource = store
        .data_mut()
        .async_invoke_and_await(handle, target_function_name.to_string(), wit_value_params)
        .await?;

    let invoke_result_resource_any = invoke_result_resource.try_into_resource_any(&mut *store)?;
    let resource_id = store.data_mut().add(invoke_result_resource_any).await;

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
    store: &mut StoreContextMut<'_, Ctx>,
    handle: Resource<WasmRpcEntry>,
) -> anyhow::Result<()> {
    let wit_value_params = encode_parameters(params, param_types, store)
        .await
        .context(format!("Encoding parameters of {target_function_name}"))?;

    store
        .data_mut()
        .invoke(handle, target_function_name.to_string(), wit_value_params)
        .await??;

    Ok(())
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

async fn create_rpc_target<Ctx: WorkerCtx>(
    store: &mut StoreContextMut<'_, Ctx>,
    target_worker_urn: Val,
) -> anyhow::Result<(OwnedWorkerId, Box<dyn RpcDemand>)> {
    let worker_urn = match target_worker_urn {
        Val::Record(ref record) => {
            let mut target = None;
            for (key, val) in record.iter() {
                if key == "value" {
                    if let Val::String(s) = val {
                        target = Some(s.clone());
                    }
                }
            }
            target
        }
        _ => None,
    };

    let (remote_worker_id, demand) = if let Some(location) = worker_urn {
        let uri = Uri {
            value: location.clone(),
        };
        match uri.parse_as_golem_urn() {
            Some((remote_worker_id, None)) => {
                let remote_worker_id = store
                    .data_mut()
                    .generate_unique_local_worker_id(remote_worker_id)
                    .await?;

                let remote_worker_id = OwnedWorkerId::new(
                    &store.data().owned_worker_id().account_id,
                    &remote_worker_id,
                );
                let demand = store.data().rpc().create_demand(&remote_worker_id).await;
                (remote_worker_id, demand)
            }
            _ => {
                return Err(anyhow!(
                    "Invalid URI: {}. Must be urn:worker:component-id/worker-name",
                    location
                ))
            }
        }
    } else {
        return Err(anyhow!("Missing or invalid worker URN parameter. Expected to use golem:rpc/types@0.1.0.{{uri}} as constructor parameter, got {target_worker_urn:?}"));
    };
    Ok((remote_worker_id, demand))
}

#[derive(Clone)]
enum DynamicRpcCall {
    GlobalStubConstructor,
    ResourceStubConstructor {
        target_constructor_name: ParsedFunctionName,
    },
    BlockingFunctionCall {
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
        if let Some(resource_name) = stub_name.is_constructor() {
            match resource_types.get(&(
                stub_name.site.interface_name().unwrap_or_default(),
                resource_name.to_string(),
            )) {
                Some(DynamicRpcResource::Stub) => Ok(Some(DynamicRpcCall::GlobalStubConstructor)),
                Some(DynamicRpcResource::ResourceStub) => {
                    let target_constructor_name = ParsedFunctionName {
                        site: rpc_metadata.target_site(resource_name).map_err(|err| anyhow!("Failed to get mapped target site ({}) from dynamic linking metadata: {}",
                            rpc_metadata.target_interface_name.iter().map(|(k, v)| format!("{k}=>{v}")).collect::<Vec<_>>().join(", "),
                            err
                        ))?,
                        function: ParsedFunctionReference::RawResourceConstructor {
                            resource: resource_name.to_string(),
                        },
                    };

                    Ok(Some(DynamicRpcCall::ResourceStubConstructor {
                        target_constructor_name,
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
                    let blocking = method_name.starts_with("blocking-");
                    let target_method_name = if blocking {
                        method_name
                            .strip_prefix("blocking-")
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

                    let target_function_name = ParsedFunctionName {
                        site: rpc_metadata.target_site(resource_name).map_err(|err| anyhow!("Failed to get mapped target site ({}) from dynamic linking metadata: {}",
                            rpc_metadata.target_interface_name.iter().map(|(k, v)| format!("{k}=>{v}")).collect::<Vec<_>>().join(", "),
                            err
                        ))?,
                        function: target_function,
                    };

                    if blocking {
                        Ok(Some(DynamicRpcCall::BlockingFunctionCall {
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
                None => Ok(None),
            }
        } else {
            // Unsupported item
            Ok(None)
        }
    }
}

#[derive(Clone)]
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
        } else if let Some(constructor) = methods
            .iter()
            .find_or_first(|m| m.method_name.contains("[constructor]"))
        {
            if Self::first_parameter_is_uri(&constructor.params) {
                if constructor.params.len() > 1 {
                    Ok(Some(DynamicRpcResource::ResourceStub))
                } else if let Some(target_interface_name) =
                    rpc_metadata.target_interface_name.get(resource_name)
                {
                    if target_interface_name.ends_with(&format!("/{resource_name}")) {
                        Ok(Some(DynamicRpcResource::Stub))
                    } else {
                        Ok(Some(DynamicRpcResource::ResourceStub))
                    }
                } else {
                    Ok(Some(DynamicRpcResource::Stub))
                }
            } else {
                // First constructor parameter is not a Uri => not a stub
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
                .filter_map(|m| m.method_name.split('.').last().map(|s| s.to_string()))
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

    fn first_parameter_is_uri(param_types: &[Type]) -> bool {
        if let Some(Type::Record(record)) = param_types.first() {
            let fields: Vec<Field> = record.fields().collect();
            fields.len() == 1 && matches!(fields[0].ty, Type::String) && fields[0].name == "value"
        } else {
            false
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
