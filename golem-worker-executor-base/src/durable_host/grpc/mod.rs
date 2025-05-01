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
use tokio_stream::wrappers::UnboundedReceiverStream;

use crate::durable_host::{DurabilityHost, DurableWorkerCtx};

use crate::workerctx::{InvocationContextManagement, WorkerCtx};
use anyhow::anyhow;
use async_trait::async_trait;
use client::*;
use golem_common::model::invocation_context::{AttributeValue, InvocationContextSpan, SpanId};
use golem_common::model::IdempotencyKey;
use golem_grpc::golem_grpc_0_1_x::types::GrpcConfiguration;
use golem_grpc::golem_grpc_0_1_x::types::GrpcMetadata;
use golem_grpc::GrpcEntry;
use golem_grpc::GrpcEntryPayload;
use golem_grpc::HostGrpc;
use prost_reflect::MethodDescriptor;
use prost_reflect::{DescriptorPool, SerializeOptions};
use prost_reflect::{DynamicMessage, MessageDescriptor};
use serde::Serialize;
use serde_json::Deserializer;
use serde_json::{json, Value as JsonValue};
use std::fmt::Debug;
use std::str::FromStr;
use std::sync::Arc;
use tonic::metadata::MetadataMap;
use wasmtime::component::Resource;
use wasmtime_wasi::WasiView;

async fn handle_invoke<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    resource: Resource<GrpcEntry>,
    function_name: String,
    service_full_name: String,
    method_full_name: String,
    params: String,
    grpc_configuration: GrpcConfiguration,
    grpc_metadata: GrpcMetadata,
) -> anyhow::Result<Vec<JsonValue>> {
    let params_json_values = serde_json::from_str::<Vec<JsonValue>>(&params)?;
    let descriptor_pool = DescriptorPool::decode(bytes::Bytes::from(grpc_metadata.fds.clone()))?;

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

    let parts: Vec<&str> = function_name.split('.').collect();

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

    let operation_type = OperationType::try_from(parts.get(1).copied().unwrap_or(""))?;

    let mut wasi = ctx.as_wasi_view();
    let table = wasi.table();

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
                                            return Ok(results_json_values);
                                        }
                                        Err(status) => {
                                            results_json_values = vec![grpc_status_to_json(status)];
                                            return Ok(results_json_values);
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
                                            return Ok(results_json_values);
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
            Ok(results_json_values)
        }
        Err(error) => Err(anyhow!(
            "Unable to establish the connection to : {}, error : {}",
            uri,
            error.to_string()
        ))?,
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostGrpc for DurableWorkerCtx<Ctx> {
    async fn new(&mut self) -> anyhow::Result<Resource<GrpcEntry>> {
        self.observe_function_call("golem::grpc::grpc", "new");

        construct_grpc_resource(self).await
    }

    async fn invoke_and_await(
        &mut self,
        resource: Resource<GrpcEntry>,
        function_name: String,
        service_full_name: String,
        method_full_name: String,
        params: String,
        grpc_configuration: GrpcConfiguration,
        grpc_metadata: GrpcMetadata,
    ) -> anyhow::Result<Result<String, String>> {
        self.observe_function_call("golem::grpc::grpc", "invoke_and_await");

        let results_json_values = handle_invoke(
            self,
            resource,
            function_name,
            service_full_name,
            method_full_name,
            params,
            grpc_configuration,
            grpc_metadata,
        )
        .await?;
        let result_str = serde_json::to_string(&results_json_values)?;
        Ok(Ok(result_str))
    }

    async fn drop(&mut self, rep: Resource<GrpcEntry>) -> anyhow::Result<()> {
        self.observe_function_call("golem::grpc::grpc", "drop");

        let entry = self.table().delete(rep)?;
        let payload = entry.payload.as_ref();
        self.finish_span(payload.span_id()).await?;

        Ok(())
    }
}

async fn construct_grpc_resource<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
) -> anyhow::Result<Resource<GrpcEntry>> {
    let span = create_grpc_connection_span(ctx).await?;

    // dummy entry
    let entry = ctx.table().push(GrpcEntry {
        payload: Box::new(GrpcEntryPayload {
            span_id: span.span_id().clone(),
            constructor_params: "".to_string(),
            rx_stream: None,
            resp_rx: None,
            sender: None,
        }),
    })?;
    Ok(entry)
}

pub async fn create_grpc_connection_span<Ctx: InvocationContextManagement>(
    ctx: &mut Ctx,
) -> anyhow::Result<Arc<InvocationContextSpan>> {
    let attributes = [(
        "name".to_string(),
        AttributeValue::String("rpc-connection".to_string()),
    )];

    Ok(ctx.start_span(&attributes).await?)
}

pub async fn create_grpc_invocation_span<Ctx: InvocationContextManagement>(
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
