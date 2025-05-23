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
use golem_common::model::oplog::DurableFunctionType;
use prost_reflect::{DynamicMessage, MethodDescriptor};
use serde::Deserialize;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tonic::{Response, Status};

use super::common::*;
use crate::durable_host::grpc::client::*;
use crate::durable_host::serialized::SerializableError;
use crate::durable_host::{
    Durability, DurableWorkerCtx, DynamicGrpcClient, GrpcEntry, GrpcEntryPayload,
};
use crate::workerctx::{InvocationContextManagement, WorkerCtx};
use anyhow::anyhow;
use async_trait::async_trait;
use golem_common::model::component_metadata::{DynamicLinkedGrpc, GrpcMetadata};
use golem_common::model::invocation_context::{AttributeValue, InvocationContextSpan};
use golem_common::model::IdempotencyKey;
use golem_wasm_rpc::wasmtime::ResourceStore;
use heck::ToPascalCase;
use prost_reflect::MessageDescriptor;
use prost_reflect::{DescriptorPool, SerializeOptions};
use rib::ParsedFunctionName;
use serde::Serialize;
use serde_json::Deserializer;
use serde_json::{json, Value as JsonValue};
use std::collections::HashMap;
use std::fmt::Debug;
use std::str::FromStr;
use std::sync::Arc;
use tonic::metadata::MetadataMap;
use tracing::{warn, Instrument};
use wasmtime::component::types::{ComponentInstance, ComponentItem};
use wasmtime::component::{LinkerInstance, Resource, ResourceType, Type, Val};
use wasmtime::{AsContextMut, Engine, StoreContextMut};
use wasmtime_wasi::{ResourceTable, WasiView};

pub fn dynamic_grpc_link<Ctx: WorkerCtx + ResourceStore + DynamicGrpcClient>(
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

async fn drop_linked_resource() -> anyhow::Result<()> {
    Ok(())
}

fn get_stored_constructor_params(
    rep: u32,
    table: &mut ResourceTable,
    param: Val,
) -> anyhow::Result<Vec<JsonValue>> {
    if let Val::Resource(_handle) = param {
        if let Some(entry) = table.get_any_mut(rep)?.downcast_ref::<GrpcEntry>() {
            let payload: &GrpcEntryPayload = entry.payload.as_ref();
            Ok(serde_json::from_str::<Vec<JsonValue>>(
                &payload.constructor_params.clone(),
            )?)
        } else {
            Err(anyhow!("GrpcEntry not found"))
        }
    } else {
        Err(anyhow!("Invalid handle parameter : {:?}", param))
    }
}

async fn register_grpc_entry<Ctx: WorkerCtx>(
    store: &mut StoreContextMut<'_, Ctx>,
    // spand_id: SpanId,
    constructor_params: String,
    rx_stream: Option<tokio::sync::Mutex<tonic::Streaming<DynamicMessage>>>,
    resp_rx: Option<oneshot::Receiver<Result<Response<DynamicMessage>, Status>>>,
    sender: Option<UnboundedSender<DynamicMessage>>,
) -> anyhow::Result<Resource<GrpcEntry>> {
    let mut wasi = store.data_mut().as_wasi_view();
    let table = wasi.table();
    Ok(table.push(GrpcEntry {
        payload: Box::new(GrpcEntryPayload {
            // span_id: spand_id,
            constructor_params,
            rx_stream,
            resp_rx,
            sender,
        }),
    })?)
}

async fn dynamic_function_call<Ctx: WorkerCtx + ResourceStore + DynamicGrpcClient + Send>(
    mut store_: impl AsContextMut<Data = Ctx> + Send,
    _function_name: &ParsedFunctionName,
    params: &[Val],
    results: &mut [Val],
    result_types: &[Type],
    call_type: &DynamicRpcCall,
    rpc_metadata: &DynamicLinkedGrpc,
) -> anyhow::Result<()> {
    let mut store = store_.as_context_mut();
    match call_type {
        DynamicRpcCall::ResourceStubConstructor { .. } => {
            let handle = init(params, &mut store).await?;

            results[0] = Val::Resource(handle.try_into_resource_any(store)?);
        }
        DynamicRpcCall::BlockingFunctionCall {
            target_function_name,
            ..
        } => {
            if let Val::Resource(handle) = params[0] {
                let resource: Resource<GrpcEntry> = handle.try_into_resource(&mut store)?;

                store
                    .data_mut()
                    .invoke_grpc(
                        resource,
                        target_function_name.function().to_string(),
                        target_function_name
                            .clone()
                            .site
                            .interface_name()
                            .unwrap()
                            .split("/")
                            .nth(1)
                            .unwrap()
                            .to_pascal_case(),
                        params,
                        results,
                        result_types,
                        rpc_metadata.metadata.clone(),
                        "blocking".to_string(),
                    )
                    .await?;
            };
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

async fn init<Ctx: WorkerCtx + ResourceStore + Send>(
    params: &[Val],
    store: &mut StoreContextMut<'_, Ctx>,
) -> Result<Resource<GrpcEntry>, anyhow::Error> {
    let constructor_params_json_values: Vec<JsonValue> = to_json_values_(params)?;

    // let span: Arc<InvocationContextSpan> = create_grpc_connection_span(store.data_mut()).await?;

    let handle = register_grpc_entry(
        store,
        // span.span_id().clone(),
        serde_json::to_string(&constructor_params_json_values)?,
        None,
        None,
        None,
    )
    .await?;
    Ok(handle)
}

// TODO: Durability grpc call

#[async_trait]
impl<Ctx: WorkerCtx> DynamicGrpcClient for DurableWorkerCtx<Ctx> {
    async fn invoke_grpc(
        &mut self,
        resource: Resource<GrpcEntry>,
        function_str: String,
        service_name: String,
        params: &[Val],
        results: &mut [Val],
        result_types: &[Type],
        grpc_metadata: GrpcMetadata,
        _call_type: String,
    ) -> anyhow::Result<()> {
        let params_json_values = to_json_values_(
            &params[1..params.len()], // skip handle
        )?;

        let constructor_params: Vec<JsonValue> =
            get_stored_constructor_params(resource.rep(), self.table(), params[0].clone())?;

        let durability = Durability::<String, SerializableError>::new(
            self,
            "golem::rpc::grpc",
            "invoke-grpc result",
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let result = if durability.is_live() {
            let result = handle_invoke_grpc(
                function_str.clone(),
                service_name,
                grpc_metadata.clone(),
                self.table(),
                params_json_values.clone(),
                constructor_params.clone(),
                resource,
            )
            .await;
            let input = (
                function_str,
                serde_json::to_string(&params_json_values)?,
                serde_json::to_string(&constructor_params)?,
                grpc_metadata,
            );
            durability.persist(self, input, result).await
        } else {
            durability.replay(self).await
        };

        let results_json_values = match result {
            Ok(s) => serde_json::from_str::<Vec<JsonValue>>(&s)?,
            Err(err) => {
                let grpc_status: GrpcStatus = GrpcStatus {
                    code: tonic::Code::Unknown,
                    message: err.to_string(),
                    details: vec![],
                };
                vec![json!(
                    {
                        "err" : serde_json::to_value(grpc_status).unwrap(),
                    }
                )]
            }
        };

        to_vals_(results_json_values, results, result_types)?;

        Ok(())
    }
}

async fn handle_invoke_grpc(
    function_str: String,
    service_name: String,
    grpc_metadata: GrpcMetadata,
    table: &mut ResourceTable,
    params_json_values: Vec<JsonValue>,
    constructor_params: Vec<JsonValue>,
    resource: Resource<GrpcEntry>,
) -> anyhow::Result<String> {
    let grpc_configuration: GrpcConfiguration =
        serde_json::from_value(constructor_params.last().unwrap().clone())?;

    let service_full_name = format!("{}.{}", grpc_metadata.package_name, service_name);

    let parts: Vec<&str> = function_str.split('.').collect();

    let method_full_name = get_method_full_name(&service_full_name, parts);

    let descriptor_pool =
        DescriptorPool::decode(bytes::Bytes::from(grpc_metadata.file_descriptor_set))?;

    let Some(service_descriptor) = descriptor_pool.get_service_by_name(&service_full_name) else {
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

    let mut metadata_map = MetadataMap::new();
    metadata_map.insert(
        "authorization",
        format!("Bearer {}", grpc_configuration.secret_token)
            .parse()
            .unwrap(),
    );

    let uri = &http::Uri::from_str(&grpc_configuration.url)?;

    // dummy initialization, we wont use it
    let mut results_json_values = { vec![grpc_status_to_json(tonic::Status::unknown("unknown"))] };

    let rpc_type = get_rpc_type(&method_descriptor);

    let parts: Vec<&str> = function_str.split('.').collect();

    let operation_type = OperationType::try_from(parts.get(1).copied().unwrap_or(""))?;

    match GrpcClient::new(uri).await {
        Ok(grpc_client) => {
            match rpc_type {
                RpcType::Unary => {
                    let dynamic_message = deserialize_dynamic_message(
                        &message_descriptor,
                        params_json_values.last().unwrap(),
                    )?;
                    match grpc_client
                        .unary_call(&method_descriptor, &dynamic_message, metadata_map)
                        .await
                    {
                        Ok(resp) => {
                            let message = resp.into_parts().1;

                            let json_value = serialize_dynamic_message(&message)?;

                            results_json_values = vec![json!(
                            {
                                "ok": json_value,
                            })];
                        }
                        Err(status) => {
                            results_json_values = vec![grpc_status_to_json(status)];
                        }
                    }
                }
                RpcType::ServerStreaming => {
                    if let Some(entry) = table
                        .get_any_mut(resource.rep())?
                        .downcast_mut::<GrpcEntry>()
                    {
                        match operation_type {
                            OperationType::Send => {
                                let dynamic_message = deserialize_dynamic_message(
                                    &message_descriptor,
                                    params_json_values.last().unwrap(),
                                )?;

                                if entry.payload.rx_stream.is_none() {
                                    match grpc_client
                                        .server_streaming_call(
                                            &method_descriptor,
                                            &dynamic_message,
                                            metadata_map,
                                        )
                                        .await
                                    {
                                        Ok(resp) => {
                                            let stream = resp.into_inner();
                                            entry.payload.rx_stream = Some(stream.into());

                                            results_json_values = vec![json!(
                                                {
                                                    "ok": serde_json::to_value(true).unwrap(),
                                                }
                                            )];
                                            return Ok(serde_json::to_string(
                                                &results_json_values,
                                            )?);
                                        }
                                        Err(status) => {
                                            results_json_values = vec![grpc_status_to_json(status)];
                                            return Ok(serde_json::to_string(
                                                &results_json_values,
                                            )?);
                                        }
                                    }
                                }
                            }
                            OperationType::Receive => {
                                let payload = &mut entry.payload;

                                match payload.receive().await {
                                    Ok(message) => {
                                        let mut json_value: serde_json::Value = JsonValue::Null;

                                        if let Some(message) = message {
                                            json_value = serialize_dynamic_message(&message)?;
                                        };

                                        results_json_values = vec![json!(
                                            {
                                                "ok": json_value,
                                            }
                                        )];
                                    }
                                    Err(status) => {
                                        results_json_values = vec![grpc_status_to_json(status)];
                                    }
                                }
                            }
                            OperationType::Finish => {
                                let payload = &mut entry.payload;

                                payload.sender.take();
                                payload.rx_stream.take();

                                results_json_values = vec![json!(
                                    {
                                        "ok": JsonValue::Bool(true),
                                    }
                                )];
                            }
                        }
                    } else {
                        Err(anyhow!("GrpcEntry not found"))?
                    }
                }
                RpcType::ClientStreaming => {
                    if let Some(entry) = table
                        .get_any_mut(resource.rep())?
                        .downcast_mut::<GrpcEntry>()
                    {
                        let payload: &mut Box<GrpcEntryPayload> = &mut entry.payload;

                        match operation_type {
                            OperationType::Send => {
                                let dynamic_message = deserialize_dynamic_message(
                                    &message_descriptor,
                                    params_json_values.last().unwrap(),
                                )?;

                                if payload.sender.is_none() {
                                    let (tx, rx) =
                                        tokio::sync::mpsc::unbounded_channel::<DynamicMessage>();
                                    payload.sender = Some(tx.clone());

                                    let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
                                    payload.resp_rx = Some(resp_rx);

                                    tokio::spawn(async move {
                                        let result = grpc_client
                                            .client_streaming_call(
                                                &method_descriptor.clone(),
                                                UnboundedReceiverStream::new(rx),
                                                metadata_map.clone(),
                                            )
                                            .await;

                                        let _ = resp_tx.send(result);
                                    });
                                };

                                match payload.send(dynamic_message).await {
                                    Ok(is_success) => {
                                        let json_value: serde_json::Value =
                                            serde_json::to_value(is_success)?;
                                        results_json_values = vec![json!({ "ok": json_value })];
                                    }
                                    Err(status) => {
                                        results_json_values = vec![grpc_status_to_json(status)];
                                    }
                                }
                            }
                            OperationType::Finish => {
                                payload.sender.take();

                                if let Some(resp_rx) = payload.resp_rx.take() {
                                    match resp_rx.await {
                                        Ok(Ok(response)) => {
                                            let message = response.into_inner();
                                            let json_value = serialize_dynamic_message(&message)?;

                                            results_json_values = vec![json!({ "ok": json_value })];
                                        }
                                        Ok(Err(status)) => {
                                            results_json_values = vec![grpc_status_to_json(status)];
                                        }
                                        Err(_) => {
                                            return Err(anyhow!(
                                                "Client stream response channel dropped"
                                            ));
                                        }
                                    }
                                } else {
                                    return Err(anyhow!(
                                        "Client streaming response future not found"
                                    ));
                                }

                                // Clean up
                                payload.resp_rx.take();
                            }
                            _ => {
                                // nothing
                            }
                        }
                    } else {
                        return Err(anyhow!("GrpcEntry not found"));
                    }
                }
                RpcType::BidirectionalStreaming => {
                    if let Some(entry) = table
                        .get_any_mut(resource.rep())?
                        .downcast_mut::<GrpcEntry>()
                    {
                        match operation_type {
                            OperationType::Send => {
                                let payload = &mut entry.payload;

                                let dynamic_message = deserialize_dynamic_message(
                                    &message_descriptor,
                                    params_json_values.last().unwrap(),
                                )?;

                                if payload.sender.is_none() {
                                    let (tx, rx) =
                                        tokio::sync::mpsc::unbounded_channel::<DynamicMessage>();
                                    payload.sender = Some(tx.clone());

                                    match grpc_client
                                        .streaming_call(
                                            &method_descriptor.clone(),
                                            UnboundedReceiverStream::new(rx),
                                            metadata_map.clone(),
                                        )
                                        .await
                                    {
                                        Ok(resp) => {
                                            let stream = resp.into_inner();
                                            payload.rx_stream = Some(stream.into());
                                        }
                                        Err(status) => {
                                            results_json_values = vec![grpc_status_to_json(status)];
                                            return Ok(serde_json::to_string(
                                                &results_json_values,
                                            )?);
                                        }
                                    }
                                };

                                match payload.send(dynamic_message).await {
                                    Ok(is_success) => {
                                        let json_value: serde_json::Value =
                                            serde_json::to_value(is_success)?;

                                        results_json_values = vec![json!(
                                            {
                                                "ok": json_value,
                                            }
                                        )];
                                    }
                                    Err(status) => {
                                        results_json_values = vec![grpc_status_to_json(status)];
                                    }
                                }
                            }
                            OperationType::Receive => {
                                let payload = &mut entry.payload;
                                match payload.receive().await {
                                    Ok(message) => {
                                        let mut json_value: serde_json::Value = JsonValue::Null;

                                        if let Some(message) = message {
                                            json_value = serialize_dynamic_message(&message)?;
                                        };

                                        results_json_values = vec![json!(
                                            {
                                                "ok": json_value,
                                            }
                                        )];
                                    }
                                    Err(status) => {
                                        results_json_values = vec![grpc_status_to_json(status)];
                                    }
                                }
                            }
                            OperationType::Finish => {
                                let payload = &mut entry.payload;

                                payload.sender.take();
                                payload.rx_stream.take();

                                results_json_values = vec![json!(
                                    {
                                        "ok": JsonValue::Bool(true),
                                    }
                                )];
                            }
                        }
                    } else {
                        Err(anyhow!("GrpcEntry not found"))?
                    }
                }
            };
            Ok(serde_json::to_string(&results_json_values)?)
        }
        Err(error) => Err(anyhow!(
            "Unable to establish the connection to : {}, error : {}",
            uri,
            error.to_string()
        ))?,
    }
}

fn get_method_full_name(service_full_name: &String, parts: Vec<&str>) -> String {
    if parts[0].ends_with("-resource-server-streaming") {
        format!(
            "{}.{}",
            service_full_name,
            parts[0]
                .strip_suffix("-resource-server-streaming")
                .unwrap() // strip suffix resource name
                .to_pascal_case()
        )
    } else if parts[0].ends_with("-resource-client-streaming") {
        format!(
            "{}.{}",
            service_full_name,
            parts[0]
                .strip_suffix("-resource-client-streaming")
                .unwrap() // strip suffix resource name
                .to_pascal_case()
        )
    } else if parts[0].ends_with("-resource-bidirectional-streaming") {
        format!(
            "{}.{}",
            service_full_name,
            parts[0]
                .strip_suffix("-resource-bidirectional-streaming")
                .unwrap() // strip suffix resource name
                .to_pascal_case()
        )
    } else {
        format!("{}.{}", service_full_name, parts[1].to_pascal_case())
    }
}

pub async fn _create_grpc_connection_span<Ctx: InvocationContextManagement>(
    ctx: &mut Ctx,
) -> anyhow::Result<Arc<InvocationContextSpan>> {
    let attributes = [(
        "name".to_string(),
        AttributeValue::String("rpc-grpc-connection".to_string()),
    )];

    Ok(ctx.start_span(&attributes).await?)
}

pub async fn _create_grpc_invocation_span<Ctx: InvocationContextManagement>(
    ctx: &mut Ctx,
    connection_span_id: &SpanId,
    method_full_name: &str,
    idempotency_key: &IdempotencyKey,
) -> anyhow::Result<Arc<InvocationContextSpan>> {
    Ok(ctx
        .start_child_span(
            connection_span_id,
            &[
                (
                    "name".to_string(),
                    AttributeValue::String("rpc-invocation".to_string()),
                ),
                (
                    "method full name".to_string(),
                    AttributeValue::String(method_full_name.to_string()),
                ),
                (
                    "idempotency_key".to_string(),
                    AttributeValue::String(idempotency_key.to_string()),
                ),
            ],
        )
        .await?)
}

pub enum RpcType {
    Unary,
    ServerStreaming,
    ClientStreaming,
    BidirectionalStreaming,
}

pub fn get_rpc_type(method: &MethodDescriptor) -> RpcType {
    match method.is_server_streaming() && method.is_client_streaming() {
        true => RpcType::BidirectionalStreaming,
        false => match method.is_server_streaming() {
            true => RpcType::ServerStreaming,
            false => match method.is_client_streaming() {
                true => RpcType::ClientStreaming,
                false => RpcType::Unary,
            },
        },
    }
}

#[derive(Serialize, Debug)]
pub struct GrpcStatus {
    #[serde[serialize_with = "code_serializer"]]
    pub code: tonic::Code,
    pub message: String,
    pub details: Vec<u8>,
}

// code_serializer
fn code_serializer<S>(code: &tonic::Code, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let code_str = match code {
        tonic::Code::Ok => "ok",
        tonic::Code::Cancelled => "cancelled",
        tonic::Code::Unknown => "unknown",
        tonic::Code::InvalidArgument => "invalid-argument",
        tonic::Code::DeadlineExceeded => "deadline-exceeded",
        tonic::Code::NotFound => "not-found",
        tonic::Code::AlreadyExists => "already-exists",
        tonic::Code::PermissionDenied => "permission-denied",
        tonic::Code::ResourceExhausted => "resource-exhausted",
        tonic::Code::FailedPrecondition => "failed-precondition",
        tonic::Code::Aborted => "aborted",
        tonic::Code::OutOfRange => "out-of-range",
        tonic::Code::Unimplemented => "unimplemented",
        tonic::Code::Internal => "internal",
        tonic::Code::Unavailable => "unavailable",
        tonic::Code::DataLoss => "data-loss",
        tonic::Code::Unauthenticated => "unauthenticated",
    };
    serializer.serialize_str(code_str)
}

fn deserialize_dynamic_message(
    descriptor: &MessageDescriptor,
    json_value: &JsonValue,
) -> anyhow::Result<DynamicMessage> {
    let json_str = if descriptor.fields().count() == 0 {
        "{}".to_string()
    } else {
        json_value.to_string()
    };

    DynamicMessage::deserialize(descriptor.clone(), &mut Deserializer::from_str(&json_str))
        .map_err(|e| anyhow!("Failed to deserialize DynamicMessage: {}", e))
}

fn serialize_dynamic_message(message: &DynamicMessage) -> anyhow::Result<JsonValue> {
    let mut serializer = serde_json::Serializer::new(vec![]);
    let options = SerializeOptions::new().skip_default_fields(false);
    message.serialize_with_options(&mut serializer, &options)?;
    let json_str = String::from_utf8(serializer.into_inner())?;
    Ok(serde_json::from_str(&json_str)?)
}

fn grpc_status_to_json(status: tonic::Status) -> JsonValue {
    let grpc_status = GrpcStatus {
        code: status.code(),
        message: status.message().to_string(),
        details: status.details().to_vec(),
    };
    json!({ "err": serde_json::to_value(grpc_status).unwrap() })
}

pub enum OperationType {
    Send,
    Receive,
    Finish,
}

impl TryFrom<&str> for OperationType {
    type Error = anyhow::Error;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        match s {
            "send" => Ok(Self::Send),
            "receive" => Ok(Self::Receive),
            "finish" => Ok(Self::Finish),
            _ => Err(anyhow!("Unknown operation type: {}", s)),
        }
    }
}

impl GrpcEntryPayload {
    pub async fn send(&self, message: DynamicMessage) -> anyhow::Result<Option<bool>, Status> {
        if let Some(sender) = self.sender.as_ref() {
            match sender.send(message) {
                Ok(_) => Ok(Some(true)),
                Err(_) => Ok(None),
            }
        } else {
            Err(tonic::Status::internal("sender not found"))
        }
    }

    pub async fn receive(&mut self) -> anyhow::Result<Option<DynamicMessage>, Status> {
        if let Some(ref mut rx_stream) = self.rx_stream.as_mut() {
            match rx_stream.get_mut().message().await {
                Ok(message_option) => Ok(message_option),
                Err(status) => Err(status),
            }
        } else {
            Err(tonic::Status::internal("receiver not found"))
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct GrpcConfiguration {
    pub url: String,
    pub secret_token: String,
}
