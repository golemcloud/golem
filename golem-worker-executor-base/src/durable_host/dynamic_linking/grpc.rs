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

use golem_common::model::invocation_context::SpanId;
use golem_grpc::golem_grpc_0_1_x::types::{
    GrpcConfiguration as GrpcConfiguration_, GrpcMetadata as GrpcMetadata_,
};
use golem_grpc::{GrpcEntry, GrpcEntryPayload, HostGrpc};
use prost_reflect::DynamicMessage;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;
use tonic::{Response, Status};

use super::common::*;
use crate::durable_host::grpc::{create_grpc_connection_span, GrpcStatus};
// use crate::durable_host::wasm_rpc::try_get_analysed_function;
use crate::workerctx::WorkerCtx;
use anyhow::anyhow;
use convert_case::{Case, Casing};
use golem_common::model::component_metadata::{DynamicLinkedGrpc, GrpcMetadata};
use golem_grpc::GrpcConfiguration;
use golem_wasm_rpc::wasmtime::ResourceStore;
use rib::ParsedFunctionName;
use serde_json::{json, Value as JsonValue};
use std::collections::HashMap;
use tracing::{warn, Instrument};
use wasmtime::component::types::{ComponentInstance, ComponentItem};
use wasmtime::component::{LinkerInstance, Resource, ResourceType, Type, Val};
use wasmtime::{AsContextMut, Engine, StoreContextMut};
use wasmtime_wasi::WasiView;

pub fn dynamic_grpc_link<Ctx: WorkerCtx + HostGrpc + ResourceStore>(
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
            Some(DynamicRpcResource::InvokeResult) => {}
            Some(DynamicRpcResource::Stub) | Some(DynamicRpcResource::ResourceStub) => {
                instance.resource_async(
                    &resource_name,
                    ResourceType::host::<GrpcEntry>(),
                    move |store, rep| {
                        Box::new(
                            async move { drop_linked_resource(store, rep).await }.in_current_span(),
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

async fn drop_linked_resource<Ctx: WorkerCtx>(
    mut store: StoreContextMut<'_, Ctx>,
    rep: u32,
) -> anyhow::Result<()> {
    let span_id = {
        let mut wasi = store.data_mut().as_wasi_view();
        let table = wasi.table();
        if let Some(entry) = table.get_any_mut(rep)?.downcast_ref::<GrpcEntry>() {
            let payload = &entry.payload;
            let span_id = payload.span_id();

            Some(span_id.clone())
        } else {
            None
        }
    };

    if let Some(span_id) = span_id {
        store.data_mut().finish_span(&span_id).await?;
    }

    Ok(())
}

fn get_stored_constructor_params<Ctx: WorkerCtx + HostGrpc>(
    store: &mut StoreContextMut<'_, Ctx>,
    param: Val,
) -> anyhow::Result<(Vec<JsonValue>, Resource<GrpcEntry>)> {
    if let Val::Resource(handle) = param {
        let resource: Resource<GrpcEntry> = handle.try_into_resource(&mut *store)?;

        let mut wasi = store.data_mut().as_wasi_view();
        let table = wasi.table();
        if let Some(entry) = table
            .get_any_mut(resource.rep())?
            .downcast_ref::<GrpcEntry>()
        {
            let payload: &GrpcEntryPayload = entry.payload.as_ref();
            Ok((
                serde_json::from_str::<Vec<JsonValue>>(&payload.constructor_params.clone())?,
                resource,
            ))
        } else {
            Err(anyhow!("GrpcEntry not found"))
        }
    } else {
        Err(anyhow!("Invalid handle parameter : {:?}", param))
    }
}

async fn register_grpc_entry<Ctx: WorkerCtx>(
    store: &mut StoreContextMut<'_, Ctx>,
    spand_id: SpanId,
    constructor_params: String,
    rx_stream: Option<tokio::sync::Mutex<tonic::Streaming<DynamicMessage>>>,
    resp_rx: Option<oneshot::Receiver<Result<Response<DynamicMessage>, Status>>>,
    sender: Option<UnboundedSender<DynamicMessage>>,
) -> anyhow::Result<Resource<GrpcEntry>> {
    let mut wasi = store.data_mut().as_wasi_view();
    let table = wasi.table();
    Ok(table.push(GrpcEntry {
        payload: Box::new(GrpcEntryPayload {
            span_id: spand_id,
            constructor_params,
            rx_stream,
            resp_rx,
            sender,
        }),
    })?)
}

async fn dynamic_function_call<Ctx: WorkerCtx + HostGrpc>(
    mut store: impl AsContextMut<Data = Ctx> + Send,
    _function_name: &ParsedFunctionName,
    params: &[Val],
    results: &mut [Val],
    result_types: &[Type],
    call_type: &DynamicRpcCall,
    rpc_metadata: &DynamicLinkedGrpc,
) -> anyhow::Result<()> {
    let mut store = store.as_context_mut();
    match call_type {
        DynamicRpcCall::ResourceStubConstructor { .. } => {
            let handle = init(params, &mut store).await?;

            results[0] = Val::Resource(handle.try_into_resource_any(store)?);
        }
        DynamicRpcCall::BlockingFunctionCall {
            target_function_name,
            ..
        } => {
            invoke_grpc(
                target_function_name,
                params,
                results,
                result_types,
                rpc_metadata.metadata.clone(),
                &mut store,
                "blocking".to_string(),
            )
            .await?;
        }
        DynamicRpcCall::ResourceCustomConstructor { .. } => {}
        DynamicRpcCall::GlobalStubConstructor { .. } => {}
        DynamicRpcCall::GlobalCustomConstructor { .. } => {}
        DynamicRpcCall::FireAndForgetFunctionCall { .. } => {}
        DynamicRpcCall::ScheduledFunctionCall { .. } => {}
        DynamicRpcCall::FutureInvokeResultSubscribe => {}
        DynamicRpcCall::FutureInvokeResultGet => {}
        DynamicRpcCall::AsyncFunctionCall { .. } => {}
    }
    Ok(())
}

async fn invoke_grpc<Ctx: WorkerCtx + HostGrpc + ResourceStore + Send>(
    function_name: &ParsedFunctionName,
    params: &[Val],
    results: &mut [Val],
    result_types: &[Type],
    grpc_metadata: GrpcMetadata,
    store: &mut StoreContextMut<'_, Ctx>,
    _call_type: String,
) -> Result<(), anyhow::Error> {
    let params_json_values = to_json_values_(
        &params[1..params.len()], // skip handle
    )?;

    let (constructor_params, handle): (Vec<JsonValue>, Resource<GrpcEntry>) =
        get_stored_constructor_params(store, params[0].clone())?;

    let results_json_values = {
        let grpc_configuration: GrpcConfiguration =
            serde_json::from_value(constructor_params.last().unwrap().clone())?;

        let service_full_name = format!(
            "{}.{}",
            grpc_metadata.package_name,
            &function_name
                .clone()
                .site
                .interface_name()
                .unwrap()
                .split("/")
                .nth(1)
                .unwrap()
                .to_case(Case::Pascal)
        );

        let function_str = function_name.function.to_string();
        let parts: Vec<&str> = function_str.split('.').collect();

        let method_full_name = if parts[0].ends_with("-resource-server-streaming") {
            format!(
                "{}.{}",
                service_full_name,
                parts[0]
                    .strip_suffix("-resource-server-streaming")
                    .unwrap() // strip suffix resource name
                    .to_case(Case::Pascal)
            )
        } else if parts[0].ends_with("-resource-client-streaming") {
            format!(
                "{}.{}",
                service_full_name,
                parts[0]
                    .strip_suffix("-resource-client-streaming")
                    .unwrap() // strip suffix resource name
                    .to_case(Case::Pascal)
            )
        } else if parts[0].ends_with("-resource-bidirectional-streaming") {
            format!(
                "{}.{}",
                service_full_name,
                parts[0]
                    .strip_suffix("-resource-bidirectional-streaming")
                    .unwrap() // strip suffix resource name
                    .to_case(Case::Pascal)
            )
        } else {
            format!("{}.{}", service_full_name, parts[1].to_case(Case::Pascal))
        };

        match store
            .data_mut()
            .invoke_and_await(
                handle,
                function_name.function.to_string(),
                service_full_name,
                method_full_name,
                serde_json::to_string(&params_json_values)?,
                GrpcConfiguration_ {
                    url: grpc_configuration.url,
                    secret_token: grpc_configuration.secret_token,
                },
                GrpcMetadata_ {
                    fds: grpc_metadata.file_descriptor_set,
                    package_name: grpc_metadata.package_name,
                },
            )
            .await
        {
            Ok(result) => match result {
                Ok(s) => serde_json::from_str::<Vec<JsonValue>>(&s)?,
                Err(err) => {
                    let grpc_status: GrpcStatus = GrpcStatus {
                        code: tonic::Code::Unknown,
                        message: err,
                        details: vec![],
                    };
                    vec![json!(
                        {
                            "err" : serde_json::to_value(grpc_status).unwrap(),
                        }
                    )]
                }
            },
            Err(err) => Err(anyhow!(err.to_string()))?,
        }
    };

    to_vals_(results_json_values, results, result_types)?;

    Ok(())
}

async fn init<Ctx: WorkerCtx + HostGrpc + ResourceStore + Send>(
    params: &[Val],
    store: &mut StoreContextMut<'_, Ctx>,
) -> Result<Resource<GrpcEntry>, anyhow::Error> {
    let constructor_params_json_values: Vec<JsonValue> = to_json_values_(params)?;

    let span = create_grpc_connection_span(store.data_mut()).await?;

    let handle = register_grpc_entry(
        store,
        span.span_id().clone(),
        serde_json::to_string(&constructor_params_json_values)?,
        None,
        None,
        None,
    )
    .await?;
    Ok(handle)
}
