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

pub mod client;
pub mod serialized;

use std::str::FromStr;

use anyhow::anyhow;
use async_trait::async_trait;
use base64::{prelude::BASE64_STANDARD, Engine};
use client::*;
use golem_common::model::{component_metadata::GrpcMetadata, oplog::DurableFunctionType};
use golem_wasm_rpc::json::TypeAnnotatedValueJsonExtensions;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::wasmtime::{encode_output_without_store, type_to_analysed_type};
use golem_wasm_rpc::{ValueAndType, WitValue};
use heck::{ToKebabCase, ToPascalCase};
use itertools::Itertools;
use prost_reflect::{
    prost_types, DescriptorPool, DeserializeOptions, DynamicMessage, MessageDescriptor,
    MethodDescriptor, ReflectMessage, SerializeOptions,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Deserializer, Value as JsonValue};
use serialized::*;
use tokio::sync::{mpsc::UnboundedSender, oneshot};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tonic::{metadata::MetadataMap, Status};
use wasmtime::component::{Resource, Type, Val};
use wasmtime_wasi::ResourceTable;

use crate::durable_host::dynamic_linking::common::json_to_val;
use crate::workerctx::WorkerCtx;

use super::{
    dynamic_linking::common::to_json_values_, serialized::SerializableError, Durability,
    DurabilityHost, DurableWorkerCtx,
};

#[async_trait]
pub trait DynamicGrpc {
    async fn init(&mut self) -> anyhow::Result<()>;

    async fn invoke_and_await_grpc(
        &mut self,
        resource: Resource<GrpcEntry>,
        function_str: String,
        service_name: String,
        params: &[Val],
        params_witvalue: &[WitValue],
        result_types: &[Type],
        grpc_metadata: GrpcMetadata,
    ) -> anyhow::Result<ValueAndType>;
}

#[async_trait]
impl<Ctx: WorkerCtx> DynamicGrpc for DurableWorkerCtx<Ctx> {
    async fn init(&mut self) -> anyhow::Result<()> {
        self.observe_function_call("golem::rpc::grpc", "new");
        Ok(())
    }

    async fn invoke_and_await_grpc(
        &mut self,
        resource: Resource<GrpcEntry>,
        function_str: String,
        service_name: String,
        params: &[Val],
        params_witvalue: &[WitValue],
        result_types: &[Type],
        grpc_metadata: GrpcMetadata,
    ) -> anyhow::Result<ValueAndType> {
        self.observe_function_call("golem::rpc::grpc", "invoke-and-await-grpc");

        // Need these in json format, so i can construct dynamicMessage and GrpcConfiguration
        let params_json_values = to_json_values_(
            &params[1..params.len()], // skip handle
        )?;
        let constructor_params: Vec<JsonValue> =
            get_stored_constructor_params(resource.rep(), self.table(), params[0].clone())?;

        let durability = Durability::<ValueAndType, SerializableError>::new(
            self,
            "golem::rpc::grpc",
            "invoke-and-await-grpc result",
            DurableFunctionType::WriteRemote,
        )
        .await?;

        let result = if durability.is_live() {
            let result = handle_invoke_and_await_grpc(
                function_str.clone(),
                service_name,
                grpc_metadata.clone(),
                self.table(),
                result_types,
                params_json_values.clone(),
                constructor_params.clone(),
                resource,
            )
            .await;

            // just for oplog
            let function_params_value_and_types =
                try_get_value_and_type(&result_types, params_witvalue)?;

            let input = SerializableRequest {
                function_name: function_str,
                function_params: function_params_value_and_types,
            };

            durability.persist(self, input, result).await
        } else {
            durability.replay(self).await
        };

        match result {
            Ok(typed_value) => Ok(typed_value),
            Err(err) => Err(anyhow!("{}", err.to_string())),
        }
    }
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

async fn handle_invoke_and_await_grpc(
    function_str: String,
    service_name: String,
    grpc_metadata: GrpcMetadata,
    table: &mut ResourceTable,
    result_types: &[Type],
    params_json_values: Vec<JsonValue>,
    constructor_params: Vec<JsonValue>,
    resource: Resource<GrpcEntry>,
) -> anyhow::Result<ValueAndType> {
    let grpc_configuration: GrpcConfiguration =
        serde_json::from_value(constructor_params.last().unwrap().clone())?;

    let service_full_name = format!("{}.{}", grpc_metadata.package_name, service_name);

    let function_name_parts: Vec<&str> = function_str.split('.').collect();

    let method_full_name = get_method_full_name(&service_full_name, function_name_parts.clone());

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

    let uri = &http::Uri::from_str(&grpc_configuration.url)?;

    // token exists?
    match uri.scheme().map(|s| s.as_str()) {
        Some("https") => {
            if grpc_configuration.secret_token.is_none() {
                let status = GrpcStatus {
                    code: tonic::Code::Unavailable,
                    message: "Secret token cannot be none for secure connection".to_string(),
                    details: vec![],
                };
                let mut results_json_value =
                    json!({ "err": serde_json::to_value(status).unwrap() });

                let val = json_to_val(&results_json_value, &result_types[0]);
                let witvalue = encode_output_without_store(&val, &result_types[0])
                    .await
                    .map_err(|err| anyhow!(format!("encoding error: {err}")))?;
                let value_and_type =
                    ValueAndType::new(witvalue, type_to_analysed_type(&result_types[0]).map_err(|err|
                                anyhow!(format!("{err}")))?);

                return Ok(value_and_type);
            }
        }
        _ => {}
    };

    let mut metadata_map = MetadataMap::new();

    if grpc_configuration.secret_token.is_some() {
        metadata_map.insert(
            "authorization",
            format!("Bearer {}", grpc_configuration.secret_token.unwrap())
                .parse()
                .unwrap(),
        );
    };

    let rpc_type = get_rpc_type(&method_descriptor);

    match GrpcClient::new(uri).await {
        Ok(grpc_client) => match rpc_type {
            RpcType::Unary => {
                handle_unary_rpc(
                    result_types,
                    &params_json_values,
                    &method_descriptor,
                    &message_descriptor,
                    metadata_map,
                    grpc_client,
                )
                .await
            }
            RpcType::ServerStreaming => {
                handle_server_streaming_rpc(
                    table,
                    result_types,
                    &params_json_values,
                    &resource,
                    &method_descriptor,
                    &message_descriptor,
                    metadata_map,
                    &function_name_parts.clone(),
                    grpc_client,
                )
                .await
            }
            RpcType::ClientStreaming => {
                handle_client_streaming_rpc(
                    table,
                    result_types,
                    &params_json_values,
                    &resource,
                    method_descriptor,
                    &message_descriptor,
                    metadata_map,
                    &function_name_parts.clone(),
                    grpc_client,
                )
                .await
            }
            RpcType::BidirectionalStreaming => {
                handle_bidirectional_streaming_rpc(
                    table,
                    result_types,
                    params_json_values,
                    resource,
                    method_descriptor,
                    message_descriptor,
                    metadata_map,
                    &function_name_parts.clone(),
                    grpc_client,
                )
                .await
            }
        },
        Err(error) => {
            let status = GrpcStatus {
                code: tonic::Code::Unavailable,
                message: format!(
                    "Unable to establish the connection to : {}, error : {}",
                    uri,
                    error.to_string()
                ),
                details: vec![],
            };
            let mut results_json_value = json!({ "err": serde_json::to_value(status).unwrap()});

            let val = json_to_val(&results_json_value, &result_types[0]);
            let witvalue = encode_output_without_store(&val, &result_types[0])
                .await
                .map_err(|err| anyhow!(format!("encoding error: {err}")))?;
            let value_and_type =
                ValueAndType::new(witvalue, type_to_analysed_type(&result_types[0]).map_err(|err|
                                anyhow!(format!("{err}")))?);

            Ok(value_and_type)
        }
    }
}

async fn handle_bidirectional_streaming_rpc(
    table: &mut ResourceTable,
    result_types: &[Type],
    params_json_values: Vec<JsonValue>,
    resource: Resource<GrpcEntry>,
    method_descriptor: MethodDescriptor,
    message_descriptor: MessageDescriptor,
    metadata_map: MetadataMap,
    function_name_parts: &Vec<&str>,
    grpc_client: GrpcClient,
) -> Result<ValueAndType, anyhow::Error> {
    let operation_type =
        OperationType::try_from(function_name_parts.get(1).copied().unwrap_or(""))?;

    if let Some(entry) = table
        .get_any_mut(resource.rep())?
        .downcast_mut::<GrpcEntry>()
    {
        match operation_type {
            OperationType::Send => {
                let payload = &mut entry.payload;

                let dynamic_message = deserialize_dynamic_message(
                    &message_descriptor,
                    params_json_values.last().unwrap().clone(),
                )
                .await?;

                if payload.sender.is_none() {
                    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<DynamicMessage>();
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
                            let mut results_json_value = grpc_status_to_json(status);

                            let val = json_to_val(&results_json_value, &result_types[0]);
                            let witvalue = encode_output_without_store(&val, &result_types[0])
                                .await
                                .map_err(|err| anyhow!(format!("encoding error: {err}")))?;
                            let value_and_type = ValueAndType::new(
                                witvalue,
                                type_to_analysed_type(&result_types[0]).map_err(|err|
                                anyhow!(format!("{err}")))?,
                            );

                            return Ok(value_and_type);
                        }
                    }
                };

                match payload.send(dynamic_message).await {
                    Ok(is_success) => {
                        let json_value: serde_json::Value = serde_json::to_value(is_success)?;

                        let mut results_json_value = json!(
                            {
                                "ok": json_value,
                            }
                        );

                        let val = json_to_val(&results_json_value, &result_types[0]);
                        let witvalue = encode_output_without_store(&val, &result_types[0])
                            .await
                            .map_err(|err| anyhow!(format!("encoding error: {err}")))?;
                        let value_and_type =
                            ValueAndType::new(witvalue, type_to_analysed_type(&result_types[0]).map_err(|err|
                                anyhow!(format!("{err}")))?);

                        Ok(value_and_type)
                    }
                    Err(status) => {
                        let mut results_json_value = grpc_status_to_json(status);

                        let val = json_to_val(&results_json_value, &result_types[0]);
                        let witvalue = encode_output_without_store(&val, &result_types[0])
                            .await
                            .map_err(|err| anyhow!(format!("encoding error: {err}")))?;
                        let value_and_type =
                            ValueAndType::new(witvalue, type_to_analysed_type(&result_types[0]).map_err(|err|
                                anyhow!(format!("{err}")))?);

                        Ok(value_and_type)
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

                        let mut results_json_value = json!(
                            {
                                "ok": json_value,
                            }
                        );

                        let val = json_to_val(&results_json_value, &result_types[0]);
                        let witvalue = encode_output_without_store(&val, &result_types[0])
                            .await
                            .map_err(|err| anyhow!(format!("encoding error: {err}")))?;
                        let value_and_type =
                            ValueAndType::new(witvalue, type_to_analysed_type(&result_types[0]).map_err(|err|
                                anyhow!(format!("{err}")))?);

                        Ok(value_and_type)
                    }
                    Err(status) => {
                        let mut results_json_value = grpc_status_to_json(status);

                        let val = json_to_val(&results_json_value, &result_types[0]);
                        let witvalue = encode_output_without_store(&val, &result_types[0])
                            .await
                            .map_err(|err| anyhow!(format!("encoding error: {err}")))?;
                        let value_and_type =
                            ValueAndType::new(witvalue, type_to_analysed_type(&result_types[0]).map_err(|err|
                                anyhow!(format!("{err}")))?);

                        Ok(value_and_type)
                    }
                }
            }
            OperationType::Finish => {
                let payload = &mut entry.payload;

                payload.sender.take();
                payload.rx_stream.take();

                let mut results_json_value = json!(
                    {
                        "ok": JsonValue::Bool(true),
                    }
                );

                let val = json_to_val(&results_json_value, &result_types[0]);
                let witvalue = encode_output_without_store(&val, &result_types[0])
                    .await
                    .map_err(|err| anyhow!(format!("encoding error: {err}")))?;
                let value_and_type =
                    ValueAndType::new(witvalue, type_to_analysed_type(&result_types[0]).map_err(|err|
                                anyhow!(format!("{err}")))?);

                Ok(value_and_type)
            }
        }
    } else {
        Err(anyhow!("GrpcEntry not found"))?
    }
}

async fn handle_client_streaming_rpc(
    table: &mut ResourceTable,
    result_types: &[Type],
    params_json_values: &Vec<JsonValue>,
    resource: &Resource<GrpcEntry>,
    method_descriptor: MethodDescriptor,
    message_descriptor: &MessageDescriptor,
    metadata_map: MetadataMap,
    function_name_parts: &Vec<&str>,
    grpc_client: GrpcClient,
) -> Result<ValueAndType, anyhow::Error> {
    let operation_type =
        OperationType::try_from(function_name_parts.get(1).copied().unwrap_or(""))?;

    if let Some(entry) = table
        .get_any_mut(resource.rep())?
        .downcast_mut::<GrpcEntry>()
    {
        let payload: &mut Box<GrpcEntryPayload> = &mut entry.payload;

        match operation_type {
            OperationType::Send => {
                let dynamic_message = deserialize_dynamic_message(
                    message_descriptor,
                    params_json_values.last().unwrap().clone(),
                )
                .await?;

                if payload.sender.is_none() {
                    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<DynamicMessage>();
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
                        let json_value: serde_json::Value = serde_json::to_value(is_success)?;
                        let mut results_json_value = json!({ "ok": json_value });

                        let val = json_to_val(&results_json_value, &result_types[0]);
                        let witvalue = encode_output_without_store(&val, &result_types[0])
                            .await
                            .map_err(|err| anyhow!(format!("encoding error: {err}")))?;
                        let value_and_type =
                            ValueAndType::new(witvalue, type_to_analysed_type(&result_types[0]).map_err(|err|
                                anyhow!(format!("{err}")))?);

                        Ok(value_and_type)
                    }
                    Err(status) => {
                        let mut results_json_value = grpc_status_to_json(status);

                        let val = json_to_val(&results_json_value, &result_types[0]);
                        let witvalue = encode_output_without_store(&val, &result_types[0])
                            .await
                            .map_err(|err| anyhow!(format!("encoding error: {err}")))?;
                        let value_and_type =
                            ValueAndType::new(witvalue, type_to_analysed_type(&result_types[0]).map_err(|err|
                                anyhow!(format!("{err}")))?);

                        Ok(value_and_type)
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

                            let mut results_json_value = json!({ "ok": json_value });
                            // Clean up
                            payload.resp_rx.take();

                            let val = json_to_val(&results_json_value, &result_types[0]);
                            let witvalue = encode_output_without_store(&val, &result_types[0])
                                .await
                                .map_err(|err| anyhow!(format!("encoding error: {err}")))?;
                            let value_and_type = ValueAndType::new(
                                witvalue,
                                type_to_analysed_type(&result_types[0]).map_err(|err|
                                anyhow!(format!("{err}")))?,
                            );

                            return Ok(value_and_type);
                        }
                        Ok(Err(status)) => {
                            let mut results_json_value = grpc_status_to_json(status);

                            // Clean up
                            payload.resp_rx.take();

                            let val = json_to_val(&results_json_value, &result_types[0]);
                            let witvalue = encode_output_without_store(&val, &result_types[0])
                                .await
                                .map_err(|err| anyhow!(format!("encoding error: {err}")))?;
                            let value_and_type = ValueAndType::new(
                                witvalue,
                                type_to_analysed_type(&result_types[0]).map_err(|err|
                                anyhow!(format!("{err}")))?,
                            );

                            return Ok(value_and_type);
                        }
                        Err(_) => {
                            // Clean up
                            payload.resp_rx.take();
                            return Err(anyhow!("Client stream response channel dropped"));
                        }
                    }
                } else {
                    // Clean up
                    payload.resp_rx.take();
                    return Err(anyhow!("Client streaming response future not found"));
                }
            }
            _ => return Err(anyhow!("Invalid operation type")),
        }
    } else {
        return Err(anyhow!("GrpcEntry not found"));
    }
}

async fn handle_server_streaming_rpc(
    table: &mut ResourceTable,
    result_types: &[Type],
    params_json_values: &Vec<JsonValue>,
    resource: &Resource<GrpcEntry>,
    method_descriptor: &MethodDescriptor,
    message_descriptor: &MessageDescriptor,
    metadata_map: MetadataMap,
    function_name_parts: &Vec<&str>,
    grpc_client: GrpcClient,
) -> Result<ValueAndType, anyhow::Error> {
    let operation_type =
        OperationType::try_from(function_name_parts.get(1).copied().unwrap_or(""))?;

    if let Some(entry) = table
        .get_any_mut(resource.rep())?
        .downcast_mut::<GrpcEntry>()
    {
        match operation_type {
            OperationType::Send => {
                let dynamic_message = deserialize_dynamic_message(
                    message_descriptor,
                    params_json_values.last().unwrap().clone(),
                )
                .await?;

                if entry.payload.rx_stream.is_none() {
                    match grpc_client
                        .server_streaming_call(method_descriptor, &dynamic_message, metadata_map)
                        .await
                    {
                        Ok(resp) => {
                            let stream = resp.into_inner();
                            entry.payload.rx_stream = Some(stream.into());

                            let mut results_json_value = json!(
                                {
                                    "ok": serde_json::to_value(true).unwrap(),
                                }
                            );

                            let val = json_to_val(&results_json_value, &result_types[0]);
                            let witvalue = encode_output_without_store(&val, &result_types[0])
                                .await
                                .map_err(|err| anyhow!(format!("encoding error: {err}")))?;
                            let value_and_type = ValueAndType::new(
                                witvalue,
                                type_to_analysed_type(&result_types[0]).map_err(|err|
                                anyhow!(format!("{err}")))?,
                            );

                            return Ok(value_and_type)
                        }
                        Err(status) => {
                            let mut results_json_value = grpc_status_to_json(status);

                            let val = json_to_val(&results_json_value, &result_types[0]);
                            let witvalue = encode_output_without_store(&val, &result_types[0])
                                .await
                                .map_err(|err| anyhow!(format!("encoding error: {err}")))?;
                            let value_and_type = ValueAndType::new(
                                witvalue,
                                type_to_analysed_type(&result_types[0]).map_err(|err|
                                anyhow!(format!("{err}")))?,
                            );

                            return Ok(value_and_type)
                        }
                    };
                } else {
                    let status = GrpcStatus {
                        code: tonic::Code::Aborted,
                        message: "stream unavailable".to_string(),
                        details: vec![],
                    };
                    let mut results_json_value =
                        json!({ "err": serde_json::to_value(status).unwrap() });

                    let val = json_to_val(&results_json_value, &result_types[0]);
                    let witvalue = encode_output_without_store(&val, &result_types[0])
                        .await
                        .map_err(|err| anyhow!(format!("encoding error: {err}")))?;
                    let value_and_type =
                        ValueAndType::new(witvalue, type_to_analysed_type(&result_types[0]).map_err(|err|
                                anyhow!(format!("{err}")))?);

                    return Ok(value_and_type)
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

                        let mut results_json_value = json!(
                            {
                                "ok": json_value,
                            }
                        );

                        let val = json_to_val(&results_json_value, &result_types[0]);
                        let witvalue = encode_output_without_store(&val, &result_types[0])
                            .await
                            .map_err(|err| anyhow!(format!("encoding error: {err}")))?;
                        let value_and_type =
                            ValueAndType::new(witvalue, type_to_analysed_type(&result_types[0]).map_err(|err|
                                anyhow!(format!("{err}")))?);

                        return Ok(value_and_type)
                    }
                    Err(status) => {
                        let mut results_json_value = grpc_status_to_json(status);

                        let val = json_to_val(&results_json_value, &result_types[0]);
                        let witvalue = encode_output_without_store(&val, &result_types[0])
                            .await
                            .map_err(|err| anyhow!(format!("encoding error: {err}")))?;
                        let value_and_type =
                            ValueAndType::new(witvalue, type_to_analysed_type(&result_types[0]).map_err(|err|
                                anyhow!(format!("{err}")))?);
                            
                        return Ok(value_and_type)
                    }
                }
            }
            OperationType::Finish => {
                let payload = &mut entry.payload;

                payload.sender.take();
                payload.rx_stream.take();

                let mut results_json_value = json!(
                    {
                        "ok": JsonValue::Bool(true),
                    }
                );

                let val = json_to_val(&results_json_value, &result_types[0]);
                let witvalue = encode_output_without_store(&val, &result_types[0])
                    .await
                    .map_err(|err| anyhow!(format!("encoding error: {err}")))?;
                let value_and_type =
                    ValueAndType::new(witvalue, type_to_analysed_type(&result_types[0]).map_err(|err|
                                anyhow!(format!("{err}")))?);

                Ok(value_and_type)
            }
        }
    } else {
        Err(anyhow!("GrpcEntry not found"))?
    }
}

async fn handle_unary_rpc(
    result_types: &[Type],
    params_json_values: &Vec<JsonValue>,
    method_descriptor: &MethodDescriptor,
    message_descriptor: &MessageDescriptor,
    metadata_map: MetadataMap,
    grpc_client: GrpcClient,
) -> Result<ValueAndType, anyhow::Error> {
    let dynamic_message = deserialize_dynamic_message(
        message_descriptor,
        params_json_values.last().unwrap().clone(),
    )
    .await?;
    match grpc_client
        .unary_call(method_descriptor, &dynamic_message, metadata_map)
        .await
    {
        Ok(resp) => {
            let message = resp.into_parts().1;

            let json_value = serialize_dynamic_message(&message)?;
            println!("json 0 : {}", json_value);

            let mut results_json_value = json!(
            {
                "ok": json_value,
            });

            let val = json_to_val(&results_json_value, &result_types[0]);
            let witvalue = encode_output_without_store(&val, &result_types[0])
                .await
                .map_err(|err| anyhow!(format!("Encoding error: {err}")))?;

            let value_and_type =
                ValueAndType::new(witvalue, type_to_analysed_type(&result_types[0]).map_err(|err|
                                anyhow!(format!("{err}")))?);

            Ok(value_and_type)
        }
        Err(status) => {
            let mut results_json_value = grpc_status_to_json(status);

            let val = json_to_val(&results_json_value, &result_types[0]);
            let witvalue = encode_output_without_store(&val, &result_types[0])
                .await
                .map_err(|err| anyhow!(format!("encoding error: {err}")))?;
            let value_and_type =
                ValueAndType::new(witvalue, type_to_analysed_type(&result_types[0]).map_err(|err|
                                anyhow!(format!("{err}")))?);

            Ok(value_and_type)
        }
    }
}

fn _to_type_annotated_value(
    result_types: &[Type],
    results_json_value: &mut JsonValue,
) -> anyhow::Result<TypeAnnotatedValue> {
    let analysed_typ = match type_to_analysed_type(&result_types[0]) {
        Ok(typ) => typ,
        Err(err) => return Err(anyhow!("{err}")),
    };

    println!("json1: {}", results_json_value);

    let _ = _to_kebab_case_fields(results_json_value);

    println!("json: {}", results_json_value);

    // Todo: Need to handle json fields to snake case (better if we have options in DynamicMessage Seserialization)
    // now we need to loop over json fields

    let type_anaotated_value =
        match TypeAnnotatedValue::parse_with_type(results_json_value, &analysed_typ) {
            Ok(typed_value) => Ok(typed_value),
            Err(err) => Err(anyhow!(
                "Error parsing result json to TypeAnnotedValue : {}",
                err.iter().join(", ")
            )),
        }?;
    Ok(type_anaotated_value)
}

fn _to_kebab_case_fields(value: &mut JsonValue) {
    match value {
        JsonValue::Object(map) => {
            let keys: Vec<String> = map.keys().cloned().collect();

            for key in keys {
                if let Some(mut value) = map.remove(&key) {
                    to_kebab_case_fields(&mut value);
                    map.insert(key.to_kebab_case(), value);
                }
            }
        }
        JsonValue::Array(values) => {
            values
                .iter_mut()
                .for_each(|value| to_kebab_case_fields(value));
        }
        _ => {}
    }
}

fn get_method_full_name(service_full_name: &String, function_name_parts: Vec<&str>) -> String {
    if function_name_parts[0].ends_with("-resource-server-streaming") {
        format!(
            "{}.{}",
            service_full_name,
            function_name_parts[0]
                .strip_suffix("-resource-server-streaming")
                .unwrap() // strip suffix resource name
                .to_pascal_case()
        )
    } else if function_name_parts[0].ends_with("-resource-client-streaming") {
        format!(
            "{}.{}",
            service_full_name,
            function_name_parts[0]
                .strip_suffix("-resource-client-streaming")
                .unwrap() // strip suffix resource name
                .to_pascal_case()
        )
    } else if function_name_parts[0].ends_with("-resource-bidirectional-streaming") {
        format!(
            "{}.{}",
            service_full_name,
            function_name_parts[0]
                .strip_suffix("-resource-bidirectional-streaming")
                .unwrap() // strip suffix resource name
                .to_pascal_case()
        )
    } else {
        format!(
            "{}.{}",
            service_full_name,
            function_name_parts[1].to_pascal_case()
        )
    }
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

async fn deserialize_dynamic_message(
    descriptor: &MessageDescriptor,
    mut json_value: JsonValue,
) -> anyhow::Result<DynamicMessage> {
    let json_str = if descriptor.fields().count() == 0 {
        "{}".to_string()
    } else {
        handle_bytes_and_enums(descriptor, &mut json_value)?;
        json_value.to_string()
    };
    DynamicMessage::deserialize_with_options(
        descriptor.clone(),
        &mut Deserializer::from_str(&json_str),
        &DeserializeOptions::new().deny_unknown_fields(true),
    )
    .map_err(|e| anyhow!("Failed to deserialize DynamicMessage: {}", e))
}

fn handle_bytes_and_enums(
    descriptor: &MessageDescriptor,
    json_value: &mut JsonValue,
) -> Result<(), anyhow::Error> {
    match *json_value {
        JsonValue::Object(ref mut map) => {
            for (field_name, value) in map.iter_mut() {
                // Todo: handle value to be array
                if let Some(field) = descriptor.get_field_by_name(&field_name) {
                    match field.field_descriptor_proto().r#type() {
                        prost_types::field_descriptor_proto::Type::Bytes => {
                            if !field.is_list() {
                                let bytes_vec: Vec<u8> = serde_json::from_value(value.clone())?;

                                *value = serde_json::to_value(BASE64_STANDARD.encode(bytes_vec))?;
                            } else {
                                match value {
                                    JsonValue::Array(_) => {
                                        let bytes_vecs: Vec<Vec<u8>> =
                                            serde_json::from_value(value.clone())?;

                                        *value = serde_json::to_value(
                                            bytes_vecs
                                                .iter()
                                                .map(|bytes_vec| BASE64_STANDARD.encode(bytes_vec))
                                                .collect::<Vec<String>>(),
                                        )?;
                                    }
                                    _ => {}
                                }
                            }
                        }
                        prost_types::field_descriptor_proto::Type::Message => {
                            if field.is_list() {
                                match value {
                                    JsonValue::Array(values) => {
                                        for value in values {
                                            handle_bytes_and_enums(
                                                &field.field_descriptor_proto().descriptor(),
                                                value,
                                            )?;
                                        }
                                    }
                                    _ => {}
                                };
                            } else {
                                handle_bytes_and_enums(
                                    &field.field_descriptor_proto().descriptor(),
                                    value,
                                )?;
                            }
                        }
                        prost_types::field_descriptor_proto::Type::Enum => {
                            if field.is_list() {
                                match value {
                                    JsonValue::Array(values) => {
                                        for value in values {
                                            // handle_enums_input(value);
                                        }
                                    }
                                    _ => {}
                                };
                            } else {
                                // handle_enums_input(value);
                            }
                        }
                        _ => {}
                    }
                };
            }
        }
        _ => {}
    };
    Ok(())
}

fn handle_enums_input(value: &mut JsonValue) {
    // remove e before a digit
    if let Some(enum_value) = value.as_str() {
        *value = JsonValue::String(enum_value.replace("e", ""))
    }
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
pub struct GrpcConfiguration {
    pub url: String,
    pub secret_token: Option<String>,
}

pub struct GrpcEntry {
    pub payload: Box<GrpcEntryPayload>,
}

pub struct GrpcEntryPayload {
    pub constructor_params: String,
    pub rx_stream: Option<tokio::sync::Mutex<tonic::Streaming<DynamicMessage>>>,
    pub resp_rx: Option<oneshot::Receiver<Result<tonic::Response<DynamicMessage>, Status>>>,
    pub sender: Option<UnboundedSender<DynamicMessage>>,
}

fn try_get_value_and_type(
    params: &[Type],
    params_wit: &[WitValue],
) -> anyhow::Result<Vec<ValueAndType>> {
    params
        .iter()
        .zip(params_wit)
        .map(|(typ, wit_value)| {
            type_to_analysed_type(typ)
                .map(|analysed_type| ValueAndType::new(wit_value.clone().into(), analysed_type))
                .map_err(anyhow::Error::msg)
        })
        .collect::<Result<_, _>>()
}
