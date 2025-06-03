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

use super::common::*;
use crate::durable_host::grpc::{DynamicGrpc, GrpcEntry, GrpcEntryPayload};
use crate::workerctx::WorkerCtx;
use anyhow::{anyhow, Context};
use golem_common::model::component_metadata::DynamicLinkedGrpc;

use golem_wasm_rpc::wasmtime::{decode_param, encode_output, ResourceStore};
use golem_wasm_rpc::{Value, WitValue};
use heck::ToPascalCase;
use prost_reflect::DynamicMessage;
use rib::ParsedFunctionName;
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;
use tonic::{Response, Status};
use tracing::{warn, Instrument};
use wasmtime::component::types::{ComponentInstance, ComponentItem};
use wasmtime::component::{LinkerInstance, Resource, ResourceType, Type, Val};
use wasmtime::{AsContextMut, Engine, StoreContextMut};
use wasmtime_wasi::WasiView;

pub fn dynamic_grpc_link<Ctx: WorkerCtx + ResourceStore + DynamicGrpc>(
    name: &str,
    rpc_metadata: &DynamicLinkedGrpc,
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
            ComponentItem::Resource(_resource) => {
                resources.entry((name, inner_name)).or_default();
            }
            _ => {}
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
            Some(DynamicRpcResource::InvokeResult) => {}
            Some(DynamicRpcResource::Stub) | Some(DynamicRpcResource::ResourceStub) => {
                instance.resource_async(
                    &resource_name,
                    ResourceType::host::<GrpcEntry>(),
                    move |_, _| {
                        Box::new(async move { drop_linked_resource().await }.in_current_span())
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
                    let rpc_metadata = rpc_metadata.clone();

                    Box::new(
                        async move {
                            dynamic_function_call(
                                store,
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

async fn drop_linked_resource() -> anyhow::Result<()> {
    Ok(())
}

async fn register_grpc_entry<Ctx: WorkerCtx>(
    store: &mut StoreContextMut<'_, Ctx>,
    constructor_params: String,
    rx_stream: Option<tokio::sync::Mutex<tonic::Streaming<DynamicMessage>>>,
    resp_rx: Option<oneshot::Receiver<Result<Response<DynamicMessage>, Status>>>,
    sender: Option<UnboundedSender<DynamicMessage>>,
) -> anyhow::Result<Resource<GrpcEntry>> {
    let mut wasi = store.data_mut().as_wasi_view();
    let table = wasi.table();
    Ok(table.push(GrpcEntry {
        payload: Box::new(GrpcEntryPayload {
            constructor_params,
            rx_stream,
            resp_rx,
            sender,
        }),
    })?)
}

async fn dynamic_function_call<Ctx: WorkerCtx + ResourceStore + DynamicGrpc + Send>(
    mut store_: impl AsContextMut<Data = Ctx> + Send,
    params: &[Val],
    param_types: &[Type],
    results: &mut [Val],
    result_types: &[Type],
    call_type: &DynamicRpcCall,
    rpc_metadata: &DynamicLinkedGrpc,
) -> anyhow::Result<()> {
    let mut store = store_.as_context_mut();
    match call_type {
        DynamicRpcCall::ResourceStubConstructor { .. } => {
            let handle = init(params, &mut store).await?;

            store.data_mut().init().await?;

            results[0] = Val::Resource(handle.try_into_resource_any(store)?);
        }
        DynamicRpcCall::BlockingFunctionCall {
            target_function_name,
            ..
        } => {
            if let Val::Resource(handle) = params[0] {
                let params_witvalue = encode_parameters_without_skip(
                    &params[1..params.len()],
                    &param_types[1..param_types.len()],
                    &mut store,
                )
                .await
                .context(format!(
                    "Encoding parameters of {}",
                    target_function_name.to_string()
                ))?;

                let resource: Resource<GrpcEntry> = handle.try_into_resource(&mut store)?;
                let service_name = target_function_name
                    .clone()
                    .site
                    .interface_name()
                    .unwrap()
                    .split("/")
                    .nth(1)
                    .unwrap()
                    .to_pascal_case();

                let result = store
                    .data_mut()
                    .invoke_and_await_grpc(
                        resource,
                        target_function_name.function().to_string(),
                        service_name,
                        params,
                        &params_witvalue,
                        result_types,
                        rpc_metadata.metadata.clone(),
                    )
                    .await?;
                let witvalue = WitValue::from(result);
                
                let decoded_result = decode_param(&witvalue.into(), &result_types[0], store.data_mut())
                    .await
                    .map_err(|err| {
                        anyhow!(format!("Failed to decode result value 0: {err}"))
                    })?;
                results[0] = decoded_result.val;
            };
        }
        _ => {}
    }
    Ok(())
}

async fn init<Ctx: WorkerCtx + ResourceStore + Send>(
    params: &[Val],
    store: &mut StoreContextMut<'_, Ctx>,
) -> Result<Resource<GrpcEntry>, anyhow::Error> {
    let constructor_params_json_values: Vec<JsonValue> = to_json_values_(params)?;

    let handle = register_grpc_entry(
        store,
        serde_json::to_string(&constructor_params_json_values)?,
        None,
        None,
        None,
    )
    .await?;
    Ok(handle)
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
