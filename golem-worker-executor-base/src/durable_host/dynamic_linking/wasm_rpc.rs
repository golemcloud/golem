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

use super::grpc::{GrpcClient, GrpcConfiguration};
use super::openapi::OpenApiClient;
use crate::durable_host::dynamic_linking::grpc::GrpcStatus;
use crate::durable_host::wasm_rpc::{
    create_rpc_connection_span, try_get_analysed_function, WasmRpcEntryPayload,
};
use crate::services::rpc::{RpcDemand, RpcError};
use crate::workerctx::WorkerCtx;
use anyhow::{anyhow, Context};
use convert_case::{Case, Casing};
use golem_common::model::component_metadata::{
    DynamicLinkedWasmRpc, GrpcRemote, OpenApiRemote, RpcRemote,
};
use golem_common::model::invocation_context::SpanId;
use golem_common::model::{
    AccountId, ComponentId, ComponentType, OwnedWorkerId, TargetWorkerId, Timestamp, WorkerId,
};
use golem_wasm_ast::analysis::{AnalysedFunction, AnalysedType};
use golem_wasm_rpc::golem_rpc_0_2_x::types::{FutureInvokeResult, HostFutureInvokeResult};
use golem_wasm_rpc::json::TypeAnnotatedValueJsonExtensions;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::protobuf::Type as ProtoType;
use golem_wasm_rpc::wasmtime::{decode_param, encode_output, ResourceStore};
use golem_wasm_rpc::{
    create_from_type, CancellationTokenEntry, HostWasmRpc, Uri, Value, ValueAndType, WasmRpcEntry,
    WitValue,
};
use itertools::Itertools;
use prost_reflect::{DescriptorPool, DynamicMessage};
use rib::{ParsedFunctionName, ParsedFunctionReference};
use serde::Serialize;
use serde_json::{json, Deserializer, Value as JsonValue};
use std::collections::HashMap;
use std::str::FromStr;
use tonic::metadata::MetadataMap;
use tracing::{warn, Instrument};
use uuid::Uuid;
use wasmtime::component::types::{ComponentInstance, ComponentItem};
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
        warn!("Resource {interface_name}.{resource_name} has type {resource_type:?}");

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
        warn!("Function {} has call type {call_type:?}", function.name);

        if let Some(call_type) = call_type {
            let rpc_metadata = rpc_metadata.clone();
            instance.func_new_async(
                &function.name.function.function_name(),
                move |store, params, results| {
                    let param_types = function.params.clone();
                    let result_types = function.results.clone();
                    let call_type = call_type.clone();
                    let function_name = function.name.clone();
                    let rpc_metadata = rpc_metadata.clone();

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
                                &rpc_metadata,
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
    span_id: SpanId,
) -> anyhow::Result<Resource<WasmRpcEntry>> {
    let mut wasi = store.data_mut().as_wasi_view();
    let table = wasi.table();
    Ok(table.push(WasmRpcEntry {
        payload: Box::new(WasmRpcEntryPayload::Interface {
            demand,
            remote_worker_id,
            span_id,
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
    rpc_metadata: &DynamicLinkedWasmRpc,
) -> anyhow::Result<()> {
    let rpc_metadata_ = rpc_metadata.clone();
    let mut store = store.as_context_mut();
    match call_type {
        DynamicRpcCall::GlobalStubConstructor {
            component_name,
            component_type,
        } => {
            // Simple stub interface constructor

            let (remote_worker_id, demand) = match component_type {
                ComponentType::Durable => {
                    let target_worker_name = params[0].clone();
                    create_default_durable_rpc_target(
                        &mut store,
                        component_name,
                        target_worker_name,
                    )
                    .await?
                }
                ComponentType::Ephemeral => {
                    create_default_ephemeral_rpc_target(&mut store, component_name).await?
                }
            };

            let span =
                create_rpc_connection_span(store.data_mut(), &remote_worker_id.worker_id).await?;

            let handle = register_wasm_rpc_entry(
                &mut store,
                remote_worker_id,
                demand,
                span.span_id().clone(),
            )?;
            results[0] = Val::Resource(handle.try_into_resource_any(store)?);
        }
        DynamicRpcCall::GlobalCustomConstructor { component_type } => {
            // Simple stub interface constructor that takes a worker-id or component-id as a parameter

            let (remote_worker_id, demand) = match component_type {
                ComponentType::Durable => {
                    let worker_id = params[0].clone();
                    create_durable_rpc_target(&mut store, worker_id).await?
                }
                ComponentType::Ephemeral => {
                    let component_id = params[0].clone();
                    create_ephemeral_rpc_target(&mut store, component_id).await?
                }
            };

            let span =
                create_rpc_connection_span(store.data_mut(), &remote_worker_id.worker_id).await?;

            let handle = register_wasm_rpc_entry(
                &mut store,
                remote_worker_id,
                demand,
                span.span_id().clone(),
            )?;
            results[0] = Val::Resource(handle.try_into_resource_any(store)?);
        }
        DynamicRpcCall::ResourceStubConstructor {
            target_constructor_name,
            component_name,
            component_type,
            ..
        } => {
            match rpc_metadata_.remote {
                RpcRemote::OpenApi(_) | RpcRemote::Grpc(_) => {
                    let component_details =
                        get_component_details(&mut store, component_name).await?;
                    let analysed_function = try_get_analysed_function::<Ctx>(
                        store.data().component_service(),
                        &component_details.0,
                        &component_details.1,
                        &target_constructor_name.to_string(),
                    )
                    .await?;

                    // ignoring handle at index 0 of params
                    let (_, params_json_values) = get_req_types(
                        target_constructor_name,
                        &params[1..params.len()],
                        &param_types[1..param_types.len()],
                        analysed_function,
                        &mut store,
                    )
                    .await?;

                    let handle = register_wasm_rpc_entry_custom(
                        &mut store,
                        Timestamp::now_utc().to_millis(),
                        params_json_values,
                    )?;
                    results[0] = Val::Resource(handle.try_into_resource_any(store)?);
                }
                RpcRemote::GolemWorker(_) => {
                    // Resource stub constructor

                    // First parameter is the target uri
                    // Rest of the parameters must be sent to the remote constructor

                    let (remote_worker_id, demand) = match component_type {
                        ComponentType::Durable => {
                            let target_worker_name = params[0].clone();
                            create_default_durable_rpc_target(
                                &mut store,
                                component_name,
                                target_worker_name,
                            )
                            .await?
                        }
                        ComponentType::Ephemeral => {
                            create_default_ephemeral_rpc_target(&mut store, component_name).await?
                        }
                    };

                    let span =
                        create_rpc_connection_span(store.data_mut(), &remote_worker_id.worker_id)
                            .await?;

                    // First creating a resource for invoking the constructor (to avoid having to make a special case)
                    let handle = register_wasm_rpc_entry(
                        &mut store,
                        remote_worker_id,
                        demand,
                        span.span_id().clone(),
                    )?;
                    let temp_handle = handle.rep();

                    let constructor_result = remote_invoke_and_await(
                        target_constructor_name,
                        params,
                        param_types,
                        &mut store,
                        handle,
                    )
                    .await?;

                    let (resource_uri, resource_id) = unwrap_constructor_result(constructor_result)
                        .context(format!("Unwrapping constructor result of {function_name}"))?;

                    let (remote_worker_id, demand) = match component_type {
                        ComponentType::Durable => {
                            let target_worker_name = params[0].clone();
                            create_default_durable_rpc_target(
                                &mut store,
                                component_name,
                                target_worker_name,
                            )
                            .await?
                        }
                        ComponentType::Ephemeral => {
                            create_default_ephemeral_rpc_target(&mut store, component_name).await?
                        }
                    };

                    let span =
                        create_rpc_connection_span(store.data_mut(), &remote_worker_id.worker_id)
                            .await?;

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
            };
        }
        DynamicRpcCall::ResourceCustomConstructor {
            target_constructor_name,
            component_type,
        } => {
            // Resource stub custom constructor

            // First parameter is the worker-id or component-id (for ephemeral)
            // Rest of the parameters must be sent to the remote constructor

            let (remote_worker_id, demand) = match component_type {
                ComponentType::Durable => {
                    let worker_id = params[0].clone();
                    create_durable_rpc_target(&mut store, worker_id).await?
                }
                ComponentType::Ephemeral => {
                    let component_id = params[0].clone();
                    create_ephemeral_rpc_target(&mut store, component_id).await?
                }
            };

            let span =
                create_rpc_connection_span(store.data_mut(), &remote_worker_id.worker_id).await?;

            // First creating a resource for invoking the constructor (to avoid having to make a special case)
            let handle = register_wasm_rpc_entry(
                &mut store,
                remote_worker_id,
                demand,
                span.span_id().clone(),
            )?;
            let temp_handle = handle.rep();

            let constructor_result = remote_invoke_and_await(
                target_constructor_name,
                params,
                param_types,
                &mut store,
                handle,
            )
            .await?;

            let (resource_uri, resource_id) = unwrap_constructor_result(constructor_result)
                .context(format!("Unwrapping constructor result of {function_name}"))?;

            let (remote_worker_id, demand) = match component_type {
                ComponentType::Durable => {
                    let worker_id = params[0].clone();
                    create_durable_rpc_target(&mut store, worker_id).await?
                }
                ComponentType::Ephemeral => {
                    let component_id = params[0].clone();
                    create_ephemeral_rpc_target(&mut store, component_id).await?
                }
            };

            let span =
                create_rpc_connection_span(store.data_mut(), &remote_worker_id.worker_id).await?;

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
            component_name,
        } => {
            match rpc_metadata_.remote {
                RpcRemote::OpenApi(open_api_remote) => {
                    openapi_call(
                        target_function_name,
                        component_name,
                        params,
                        param_types,
                        results,
                        result_types,
                        open_api_remote,
                        &mut store,
                        "blocking".to_string(),
                    )
                    .await?;
                }
                RpcRemote::Grpc(grpc_remote) => {
                    grpc_call(
                        target_function_name,
                        component_name,
                        params,
                        param_types,
                        results,
                        result_types,
                        grpc_remote,
                        &mut store,
                        "blocking".to_string(),
                    )
                    .await?;
                }
                RpcRemote::GolemWorker(_) => {
                    // Simple stub interface method
                    let handle = match params[0] {
                    Val::Resource(handle) => handle,
                    _ => return Err(anyhow!("Invalid parameter for {function_name} - it must be a resource handle but it is {:?}", params[0])),
                };
                    let handle: Resource<WasmRpcEntry> = handle.try_into_resource(&mut store)?;

                    let result = remote_invoke_and_await(
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
            };
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
                &mut store,
                handle,
            )
            .await?;

            results[0] = Val::Resource(cancellation_token.try_into_resource_any(store)?);
        }
        DynamicRpcCall::AsyncFunctionCall {
            target_function_name,
            component_name,
        } => {
            match rpc_metadata_.remote {
                RpcRemote::OpenApi(open_api_remote) => {
                    openapi_call(
                        target_function_name,
                        component_name,
                        params,
                        param_types,
                        results,
                        result_types,
                        open_api_remote,
                        &mut store,
                        "async".to_string(),
                    )
                    .await?;
                }
                RpcRemote::Grpc(grpc_remote) => {
                    grpc_call(
                        target_function_name,
                        component_name,
                        params,
                        param_types,
                        results,
                        result_types,
                        grpc_remote,
                        &mut store,
                        "async".to_string(),
                    )
                    .await?;
                }
                RpcRemote::GolemWorker(_) => {
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
            };
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

async fn grpc_call<Ctx: WorkerCtx + HostWasmRpc + ResourceStore + Send>(
    function_name: &ParsedFunctionName,
    component_name: &String,
    params: &[Val],
    param_types: &[Type],
    results: &mut [Val],
    result_types: &[Type],
    grpc_remote: GrpcRemote,
    store: &mut StoreContextMut<'_, Ctx>,
    _call_type: String,
) -> Result<(), anyhow::Error> {
    let component_details = get_component_details(store, component_name).await?;
    let analysed_function = try_get_analysed_function::<Ctx>(
        store.data().component_service(),
        &component_details.0,
        &component_details.1,
        &function_name.to_string(),
    )
    .await?;

    let (_, params_json_values) = get_req_types(
        function_name,
        params,
        param_types,
        analysed_function.clone(),
        store,
    )
    .await?;

    let result_analysed_types = if analysed_function.results.len() == result_types.len() {
        analysed_function
            .results
            .iter()
            .map(|def| def.typ.clone())
            .collect()
    } else {
        Vec::new() // throw error
    };

    let constructor_params: Vec<JsonValue> = get_stored_constructor_args(store, &params[0])?;

    let results_json_values = {
        let grpc_configuration: GrpcConfiguration =
            serde_json::from_value(constructor_params.last().unwrap().clone())?;

        let descriptor_pool = DescriptorPool::decode(bytes::Bytes::from(
            grpc_remote
                .metadata
                .as_ref()
                .unwrap()
                .file_descriptor_set
                .clone(),
        ))?;

        let service_full_name = format!(
            "{}.{}",
            grpc_remote.metadata.as_ref().unwrap().package_name,
            &function_name
                .site
                .interface_name()
                .unwrap()
                .split("/")
                // skipping hadle at index 0 of params
                .nth(1)
                .unwrap()
                .to_case(Case::Pascal)
        );

        let method_full_name = format!(
            "{}.{}",
            service_full_name,
            function_name
                .function
                .to_string()
                .split(".")
                .nth(1)
                .unwrap()
                .to_case(Case::Pascal)
        );

        let Some(service_descriptor) = descriptor_pool
            .get_service_by_name(&service_full_name) 
        else {
            Err(anyhow!(
                "Cannot find grpc service with full name : {}",
                service_full_name
            ))?
        };
            

        let Some(method_descriptor) = service_descriptor
            .methods()
            .find(|method| method.full_name() == method_full_name)
        else {
            Err(anyhow!(
                "Cannot find grpc service method with full name : {}",
                method_full_name
            ))?
        };

        let message_descriptor: prost_reflect::MessageDescriptor = method_descriptor.input();

        let json_str = params_json_values.last().unwrap().to_string(); // grpc only one message

        let dynamic_message =
            DynamicMessage::deserialize(message_descriptor, &mut Deserializer::from_str(&json_str))
                .unwrap();

        let mut metadata_map = MetadataMap::new();
        metadata_map.insert(
            "Authorization",
            format!("Bearer {}", grpc_configuration.secret_token)
                .parse()
                .unwrap(),
        );

        let uri = &http::Uri::from_str(&grpc_configuration.url)?;

        match GrpcClient::new(uri).await {
            Ok(grpc_client) => {
                match grpc_client
                    .unary_call(&method_descriptor, &dynamic_message, metadata_map)
                    .await
                {
                    Ok(resp) => {
                        let result_dynamic_message = resp.into_parts().1;
                        let mut serializer = serde_json::Serializer::new(vec![]);
                        result_dynamic_message.serialize(&mut serializer).unwrap();
                        let json_value: serde_json::Value = serde_json::from_str(&String::from_utf8(serializer.into_inner())?)?;

                        vec![json!(
                            {
                                "ok": json_value,
                            }
                        )]
                    }
                    Err(status) => {
                        let grpc_status: GrpcStatus = GrpcStatus {
                            code: status.code(),
                            message: status.message().to_string(),
                            details: status.details().to_vec(),
                        };

                        // return result with err
                        vec![json!(
                            {
                                "err" : serde_json::to_value(grpc_status).unwrap(),
                            }
                        )]
                    }
                }
            },
            Err(_) => {
                
                    Err(anyhow!(
                        "Unable to make connection with uri : {}",
                        uri
                    ))?
            },
        }
    };

    to_wasmtime_vals_from_jsonvalue(
        store,
        results_json_values,
        results,
        result_types,
        result_analysed_types,
    )
    .await?;
    Ok(())
}

async fn openapi_call<Ctx: WorkerCtx + HostWasmRpc + ResourceStore + Send>(
    function_name: &ParsedFunctionName,
    component_name: &String,
    params: &[Val],
    param_types: &[Type],
    results: &mut [Val],
    result_types: &[Type],
    open_api_remote: OpenApiRemote,
    store: &mut StoreContextMut<'_, Ctx>,
    call_type: String,
) -> Result<(), anyhow::Error> {
    let component_details = get_component_details(store, component_name).await?;
    let analysed_function = try_get_analysed_function::<Ctx>(
        store.data().component_service(),
        &component_details.0,
        &component_details.1,
        &function_name.to_string(),
    )
    .await?;

    let (_, params_json_values) = get_req_types(
        function_name,
        params,
        param_types,
        analysed_function.clone(),
        store,
    )
    .await?;

    let result_analysed_types = if analysed_function.results.len() == result_types.len() {
        analysed_function
            .results
            .iter()
            .map(|def| def.typ.clone())
            .collect()
    } else {
        Vec::new()
    };

    let constructor_params: Vec<JsonValue> = get_stored_constructor_args(store, &params[0])?;

    let results_json_values = if call_type == "async" {
        OpenApiClient::new(
            function_name,
            params_json_values.clone(),
            constructor_params,
            open_api_remote.metadata,
        )
        .execute_async()
        .await?
    } else {
        OpenApiClient::new(
            function_name,
            params_json_values.clone(),
            constructor_params,
            open_api_remote.metadata,
        )
        .execute()
        .await?
    };

    to_wasmtime_vals_from_jsonvalue(
        store,
        results_json_values,
        results,
        result_types,
        result_analysed_types,
    )
    .await?;
    Ok(())
}

async fn get_req_types<Ctx: WorkerCtx + HostWasmRpc + ResourceStore + Send>(
    function_name: &ParsedFunctionName,
    params: &[Val],
    param_types: &[Type],
    analysed_function: AnalysedFunction,
    store: &mut StoreContextMut<'_, Ctx>,
) -> Result<(Vec<ValueAndType>, Vec<JsonValue>), anyhow::Error> {
    let params_wit_value = encode_parameters_without_skip(params, param_types, store)
        .await
        .context(format!("Encoding parameters of {function_name}"))?;

    let params_value_and_type = if analysed_function.parameters.len() == params_wit_value.len() {
        params_wit_value
            .iter()
            .zip(analysed_function.parameters)
            .map(|(value, def)| ValueAndType::new(value.clone().into(), def.typ.clone()))
            .collect()
    } else {
        Vec::new()
    };

    let params_json_values: Vec<JsonValue> = to_serde_values(
        function_name,
        params,
        param_types,
        store,
        params_value_and_type.clone(),
    )
    .await?;
    Ok((params_value_and_type, params_json_values))
}

fn get_stored_constructor_args<Ctx: WorkerCtx + ResourceStore + Send>(
    store: &mut StoreContextMut<'_, Ctx>,
    param: &Val,
) -> anyhow::Result<Vec<JsonValue>> {
    // For function calls, retrieve stored constructor args
    if let Val::Resource(handle) = param {
        let resource: Resource<WasmRpcEntry> = handle.try_into_resource(&mut *store)?;

        let mut wasi = store.data_mut().as_wasi_view();
        let table = wasi.table();
        if let Some(entry) = table
            .get_any_mut(resource.rep())?
            .downcast_ref::<WasmRpcEntry>()
        {
            let payload = entry
                .payload
                .downcast_ref::<(u64, Vec<JsonValue>)>()
                .unwrap();
            Ok(payload.1.clone())
        } else {
            Err(anyhow!("WasmRpcEntry not found"))
        }
    } else {
        Err(anyhow!("Invalid parameter : {:?}", param))
    }
}

async fn to_wasmtime_vals_from_jsonvalue<Ctx: ResourceStore + Send>(
    store: &mut StoreContextMut<'_, Ctx>,
    results_json_values: Vec<JsonValue>,
    results: &mut [Val],
    result_types: &[Type],
    result_analysed_types: Vec<AnalysedType>,
) -> anyhow::Result<()> {
    for (idx, (json_value, typ)) in results_json_values
        .iter()
        .zip(result_analysed_types)
        .enumerate()
    {
        let value = match TypeAnnotatedValue::parse_with_type(json_value, &typ) {
            Ok(typed_value) => match Value::try_from(typed_value) {
                Ok(value) => Ok(value),
                Err(err) => Err(anyhow!("Error parsing TypeAnnotedValue to Value : {}", err)),
            },
            Err(err) => Err(anyhow!(
                "Error parsing result json to TypeAnnotedValue : {}",
                err.iter().join("\n")
            )),
        }?;
        let result = decode_param(&value, &result_types[idx], store.data_mut())
            .await
            .map_err(|err| anyhow!(format!("Failed to decode result value {idx}: {err}")))?;
        results[idx] = result.val;
    }
    Ok(())
}

async fn to_serde_values<Ctx: ResourceStore + Send>(
    function_name: &ParsedFunctionName,
    params: &[Val],
    param_types: &[Type],
    store: &mut StoreContextMut<'_, Ctx>,
    params_value_and_type: Vec<ValueAndType>,
) -> Result<Vec<JsonValue>, anyhow::Error> {
    let params_value = encode_parameters_to_value(params, param_types, store)
        .await
        .context(format!("Encoding parameters of {function_name}"))?;
    let mut params_json_values: Vec<JsonValue> = Vec::new();
    for (value, vt) in params_value.iter().zip(&params_value_and_type) {
        let typed_value =
            create_from_type(value, &ProtoType::from(&AnalysedType::from(vt.clone())));
        match typed_value {
            Ok(value) => {
                params_json_values.push(value.to_json_value());
            }
            Err(_) => {
                return Err(anyhow!("Error creating typed value"));
            }
        }
    }
    Ok(params_json_values.clone())
}

fn register_wasm_rpc_entry_custom<Ctx: WorkerCtx + HostWasmRpc + ResourceStore>(
    store: &mut StoreContextMut<'_, Ctx>,
    resource_id: u64,
    constructor_params: Vec<JsonValue>,
) -> anyhow::Result<Resource<WasmRpcEntry>> {
    let mut wasi = store.data_mut().as_wasi_view();
    let table = wasi.table();
    Ok(table.push(WasmRpcEntry {
        payload: Box::new((resource_id, constructor_params)), // grpc and openapi
    })?)
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
            if entry
                .payload
                .downcast_ref::<(u64, Vec<JsonValue>)>()
                .is_some()
            {
                // grpc or openapi
                (false, None)
            } else if let Some(payload) = entry.payload.downcast_ref::<WasmRpcEntryPayload>() {
                let span_id = payload.span_id();
                (
                    matches!(payload, WasmRpcEntryPayload::Resource { .. }),
                    Some(span_id.clone()),
                )
            } else {
                (false, None)
            }
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

pub async fn encode_parameters<Ctx: ResourceStore + Send>(
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

pub async fn encode_parameters_without_skip<Ctx: ResourceStore + Send>(
    params: &[Val],
    param_types: &[Type],
    store: &mut StoreContextMut<'_, Ctx>,
) -> anyhow::Result<Vec<WitValue>> {
    let mut wit_value_params = Vec::new();
    for (idx, (param, typ)) in params.iter().zip(param_types).enumerate() {
        let value: Value = encode_output(param, typ, store.data_mut())
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

async fn schedule_remote_invocation<Ctx: WorkerCtx + HostWasmRpc + HostFutureInvokeResult>(
    scheduled_for: golem_wasm_rpc::wasi::clocks::wall_clock::Datetime,
    target_function_name: &ParsedFunctionName,
    params: &[Val],
    param_types: &[Type],
    store: &mut StoreContextMut<'_, Ctx>,
    handle: Resource<WasmRpcEntry>,
) -> anyhow::Result<Resource<CancellationTokenEntry>> {
    let wit_value_params = encode_parameters(params, param_types, store)
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

fn val_to_datetime(val: Val) -> anyhow::Result<golem_wasm_rpc::wasi::clocks::wall_clock::Datetime> {
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

    Ok(golem_wasm_rpc::wasi::clocks::wall_clock::Datetime {
        seconds,
        nanoseconds,
    })
}

async fn create_default_durable_rpc_target<Ctx: WorkerCtx>(
    store: &mut StoreContextMut<'_, Ctx>,
    component_name: &String,
    target_worker_name: Val,
) -> anyhow::Result<(OwnedWorkerId, Box<dyn RpcDemand>)> {
    let worker_name = match target_worker_name {
        Val::String(name) => name,
        _ => return Err(anyhow!("Missing or invalid worker name parameter. Expected to get a string worker name as constructor parameter, got {target_worker_name:?}")),
    };

    let result = store
        .data()
        .component_service()
        .resolve_component(
            component_name.clone(),
            store.data().component_metadata().component_owner.clone(),
        )
        .await?;

    if let Some(component_id) = result {
        let remote_worker_id = WorkerId {
            component_id,
            worker_name,
        };
        let remote_worker_id = OwnedWorkerId::new(
            &store.data().owned_worker_id().account_id,
            &remote_worker_id,
        );
        let demand = store.data().rpc().create_demand(&remote_worker_id).await;
        Ok((remote_worker_id, demand))
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

async fn create_durable_rpc_target<Ctx: WorkerCtx>(
    store: &mut StoreContextMut<'_, Ctx>,
    worker_id: Val,
) -> anyhow::Result<(OwnedWorkerId, Box<dyn RpcDemand>)> {
    let remote_worker_id = decode_worker_id(worker_id.clone()).ok_or_else(|| anyhow!("Missing or invalid worker id parameter. Expected to get a worker-id value as a custom constructor parameter, got {worker_id:?}"))?;

    let remote_worker_id = OwnedWorkerId::new(
        &store.data().owned_worker_id().account_id,
        &remote_worker_id,
    );
    let demand = store.data().rpc().create_demand(&remote_worker_id).await;
    Ok((remote_worker_id, demand))
}

async fn create_default_ephemeral_rpc_target<Ctx: WorkerCtx>(
    store: &mut StoreContextMut<'_, Ctx>,
    component_name: &String,
) -> anyhow::Result<(OwnedWorkerId, Box<dyn RpcDemand>)> {
    let result = store
        .data()
        .component_service()
        .resolve_component(
            component_name.clone(),
            store.data().component_metadata().component_owner.clone(),
        )
        .await?;

    if let Some(component_id) = result {
        let remote_worker_id = store
            .data_mut()
            .generate_unique_local_worker_id(TargetWorkerId {
                component_id,
                worker_name: None,
            })
            .await?;
        let remote_worker_id = OwnedWorkerId::new(
            &store.data().owned_worker_id().account_id,
            &remote_worker_id,
        );
        let demand = store.data().rpc().create_demand(&remote_worker_id).await;
        Ok((remote_worker_id, demand))
    } else {
        Err(anyhow!("Failed to resolve component {component_name}"))
    }
}

async fn create_ephemeral_rpc_target<Ctx: WorkerCtx>(
    store: &mut StoreContextMut<'_, Ctx>,
    component_id: Val,
) -> anyhow::Result<(OwnedWorkerId, Box<dyn RpcDemand>)> {
    let component_id = decode_component_id(component_id.clone()).ok_or_else(|| anyhow!("Missing or invalid component id parameter. Expected to get a component-id value as a custom constructor parameter, got {component_id:?}"))?;
    let remote_worker_id = store
        .data_mut()
        .generate_unique_local_worker_id(TargetWorkerId {
            component_id,
            worker_name: None,
        })
        .await?;
    let remote_worker_id = OwnedWorkerId::new(
        &store.data().owned_worker_id().account_id,
        &remote_worker_id,
    );
    let demand = store.data().rpc().create_demand(&remote_worker_id).await;
    Ok((remote_worker_id, demand))
}

#[derive(Debug, Clone)]
enum DynamicRpcCall {
    GlobalStubConstructor {
        component_name: String,
        component_type: ComponentType,
    },
    GlobalCustomConstructor {
        component_type: ComponentType,
    },
    ResourceStubConstructor {
        component_name: String,
        component_type: ComponentType,
        target_constructor_name: ParsedFunctionName,
    },
    ResourceCustomConstructor {
        component_type: ComponentType,
        target_constructor_name: ParsedFunctionName,
    },
    BlockingFunctionCall {
        target_function_name: ParsedFunctionName,
        component_name: String,
    },
    ScheduledFunctionCall {
        target_function_name: ParsedFunctionName,
    },
    FireAndForgetFunctionCall {
        target_function_name: ParsedFunctionName,
    },
    AsyncFunctionCall {
        target_function_name: ParsedFunctionName,
        component_name: String,
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
                        component_type: target.component_type,
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
                        component_type: target.component_type,
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
                                let target = rpc_metadata
                                    .target(resource_name)
                                    .map_err(|err| anyhow!(err))
                                    .context(context(rpc_metadata))?;

                                Ok(Some(DynamicRpcCall::GlobalCustomConstructor {
                                    component_type: target.component_type,
                                }))
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
                                    component_type: target.component_type,
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
                                component_name: target.component_name,
                            }))
                        } else if scheduled {
                            Ok(Some(DynamicRpcCall::ScheduledFunctionCall {
                                target_function_name,
                            }))
                        } else if !result_types.is_empty() {
                            Ok(Some(DynamicRpcCall::AsyncFunctionCall {
                                target_function_name,
                                component_name: target.component_name,
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

async fn encode_parameters_to_value<Ctx: ResourceStore + Send>(
    params: &[Val],
    param_types: &[Type],
    store: &mut StoreContextMut<'_, Ctx>,
) -> anyhow::Result<Vec<Value>> {
    let mut value_params = Vec::new();
    for (idx, (param, typ)) in params.iter().zip(param_types).enumerate() {
        let value: Value = encode_output(param, typ, store.data_mut())
            .await
            .map_err(|err| anyhow!(format!("Failed to encode parameter {idx}: {err}")))?;
        value_params.push(value);
    }
    Ok(value_params)
}

async fn get_component_details<Ctx: WorkerCtx>(
    store: &mut StoreContextMut<'_, Ctx>,
    component_name: &String,
) -> anyhow::Result<(AccountId, ComponentId)> {
    let result = store
        .data()
        .component_service()
        .resolve_component(
            component_name.clone(),
            store.data().component_metadata().component_owner.clone(),
        )
        .await?;

    if let Some(component_id) = result {
        Ok((store.data().owned_worker_id().account_id(), component_id))
    } else {
        Err(anyhow!("Failed to resolve component {component_name}"))
    }
}
