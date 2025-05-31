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
use heck::ToPascalCase;
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

use crate::workerctx::WorkerCtx;

use super::{
    dynamic_linking::common::{to_json_values_, to_vals_},
    serialized::SerializableError,
    Durability, DurabilityHost, DurableWorkerCtx,
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
        results: &mut [Val],
        result_types: &[Type],
        grpc_metadata: GrpcMetadata,
        _call_type: String,
    ) -> anyhow::Result<()>;
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
        results: &mut [Val],
        result_types: &[Type],
        grpc_metadata: GrpcMetadata,
        _call_type: String,
    ) -> anyhow::Result<()> {
        self.observe_function_call("golem::rpc::grpc", "invoke-and-await-grpc");

        let params_json_values = to_json_values_(
            &params[1..params.len()], // skip handle
        )?;

        let constructor_params: Vec<JsonValue> =
            get_stored_constructor_params(resource.rep(), self.table(), params[0].clone())?;

        let durability = Durability::<String, SerializableError>::new(
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
                params_json_values.clone(),
                constructor_params.clone(),
                resource,
            )
            .await;

            let input = SerializableRequest {
                function_name: function_str,
                function_params: vec![],
                constructor_params: vec![],
                grpc_metadata,
            };

            durability.persist(self, input, result).await
        } else {
            durability.replay(self).await
        };

        let results_json_values = serde_json::from_str::<Vec<JsonValue>>(&result?)?;

        to_vals_(results_json_values, results, result_types)?;

        Ok(())
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

    let uri = &http::Uri::from_str(&grpc_configuration.url)?;

    let mut results_json_values = { vec![grpc_status_to_json(tonic::Status::unknown("unknown"))] };

    // token validation
    match uri.scheme().map(|s| s.as_str()) {
        Some("https") => {
            if grpc_configuration.secret_token.is_none() {
                let status = GrpcStatus {
                    code: tonic::Code::Unavailable,
                    message: "Secret token cannot be none for secure connection".to_string(),
                    details: vec![],
                };
                results_json_values = vec![json!({ "err": serde_json::to_value(status).unwrap() })];
                return Ok(serde_json::to_string(&results_json_values)?);
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

    // dummy initialization, we wont use it

    let rpc_type = get_rpc_type(&method_descriptor);

    let parts: Vec<&str> = function_str.split('.').collect();

    match GrpcClient::new(uri).await {
        Ok(grpc_client) => {
            match rpc_type {
                RpcType::Unary => {
                    let dynamic_message = deserialize_dynamic_message(
                        &message_descriptor,
                        params_json_values.last().unwrap().clone(),
                    )
                    .await?;
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
                    let operation_type =
                        OperationType::try_from(parts.get(1).copied().unwrap_or(""))?;

                    if let Some(entry) = table
                        .get_any_mut(resource.rep())?
                        .downcast_mut::<GrpcEntry>()
                    {
                        match operation_type {
                            OperationType::Send => {
                                let dynamic_message = deserialize_dynamic_message(
                                    &message_descriptor,
                                    params_json_values.last().unwrap().clone(),
                                )
                                .await?;

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
                    let operation_type =
                        OperationType::try_from(parts.get(1).copied().unwrap_or(""))?;

                    if let Some(entry) = table
                        .get_any_mut(resource.rep())?
                        .downcast_mut::<GrpcEntry>()
                    {
                        let payload: &mut Box<GrpcEntryPayload> = &mut entry.payload;

                        match operation_type {
                            OperationType::Send => {
                                let dynamic_message = deserialize_dynamic_message(
                                    &message_descriptor,
                                    params_json_values.last().unwrap().clone(),
                                )
                                .await?;

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
                    let operation_type =
                        OperationType::try_from(parts.get(1).copied().unwrap_or(""))?;

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
            results_json_values = vec![json!({ "err": serde_json::to_value(status).unwrap() })];
            Ok(serde_json::to_string(&results_json_values)?)
        }
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
        handle_bytes(descriptor, &mut json_value)?;

        json_value.to_string()
    };
    DynamicMessage::deserialize_with_options(
        descriptor.clone(),
        &mut Deserializer::from_str(&json_str),
        &DeserializeOptions::new().deny_unknown_fields(true),
    )
    .map_err(|e| anyhow!("Failed to deserialize DynamicMessage: {}", e))
}

fn handle_bytes(
    descriptor: &MessageDescriptor,
    json_value: &mut JsonValue,
) -> Result<(), anyhow::Error> {
    match *json_value {
        JsonValue::Object(ref mut map) => {
            for (field_name, value) in map.iter_mut() {
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
                                            handle_bytes(
                                                &field.field_descriptor_proto().descriptor(),
                                                value,
                                            )?;
                                        }
                                    }
                                    _ => {}
                                };
                            } else {
                                handle_bytes(&field.field_descriptor_proto().descriptor(), value)?;
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
    // pub span_id: SpanId,
    pub constructor_params: String,
    pub rx_stream: Option<tokio::sync::Mutex<tonic::Streaming<DynamicMessage>>>,
    pub resp_rx: Option<oneshot::Receiver<Result<tonic::Response<DynamicMessage>, Status>>>,
    pub sender: Option<UnboundedSender<DynamicMessage>>,
}
