// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

//! Binding-specific stream payloads and shared library SSE framing.
use crate::{ProtoJsonPayloadError, jsonrpc, wire};
use a2a::{A2AError, StreamResponse};
use bytes::Bytes;
use futures::Stream;
use jsonrpc::{JsonRpcId, JsonRpcResponse};
use poem::web::sse::{Event, SSE};
use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Payload(#[from] ProtoJsonPayloadError),
    #[error(transparent)]
    Remote(#[from] A2AError),
    #[error("invalid A2A stream envelope: {0}")]
    Envelope(&'static str),
}

pub fn encode_http(event: &StreamResponse) -> Result<Value, ProtoJsonPayloadError> {
    wire::to_value(event)
}

pub fn decode_http(value: Value) -> Result<StreamResponse, DecodeError> {
    Ok(wire::from_value(value)?)
}

pub fn encode_jsonrpc(
    id: JsonRpcId,
    event: &StreamResponse,
) -> Result<JsonRpcResponse, ProtoJsonPayloadError> {
    Ok(JsonRpcResponse::success(id, wire::to_value(event)?))
}

/// Require the original ID so interleaved or miscorrelated responses cannot
/// silently become events in another request's stream.
pub fn decode_jsonrpc(value: Value, id: &JsonRpcId) -> Result<StreamResponse, DecodeError> {
    if value.get("result").is_some() == value.get("error").is_some() {
        return Err(DecodeError::Envelope(
            "exactly one of result or error must be present",
        ));
    }
    let response: JsonRpcResponse = serde_json::from_value(value)?;
    if response.jsonrpc != "2.0"
        || &response.id != id
        || response.result.is_some() == response.error.is_some()
    {
        return Err(DecodeError::Envelope(
            "version, ID, or result/error exclusivity",
        ));
    }
    if let Some(error) = response.error {
        return Err(DecodeError::Remote(jsonrpc::decode_error(error)?));
    }
    Ok(wire::from_value(response.result.unwrap())?)
}

/// Serialize a binding's already encoded payload as a Poem SSE event.
pub fn sse_event(payload: &impl serde::Serialize) -> Result<Event, serde_json::Error> {
    Ok(Event::message(serde_json::to_string(payload)?))
}

pub fn sse_response(events: impl Stream<Item = Event> + Send + 'static) -> SSE {
    SSE::new(events)
}

pub fn parse_sse<S, E>(bytes: S) -> impl Stream<Item = Result<sse_stream::Sse, sse_stream::Error>>
where
    S: Stream<Item = Result<Bytes, E>>,
    E: std::error::Error + Send + Sync + 'static,
{
    sse_stream::SseStream::from_bytes_stream(bytes)
}

/// Named `error` events are not part of v1.0.1 §11.7. Do not reinterpret them
/// as a nonstandard error binding. Comments and keepalives have no data.
pub fn sse_data(event: sse_stream::Sse) -> Result<Option<Value>, DecodeError> {
    if !matches!(event.event.as_deref(), None | Some("" | "message")) {
        return Err(DecodeError::Envelope("unsupported SSE event name"));
    }
    event
        .data
        .map(|data| serde_json::from_str(&data))
        .transpose()
        .map_err(Into::into)
}
