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

//! JSON-RPC envelopes are upstream types; payloads are ProtoJSON values.
use a2a::A2AError;
pub use a2a::jsonrpc::*;
use serde_json::Value;

/// A malformed JSON body has no trustworthy request ID (§9.5).
pub fn parse_request(body: &[u8]) -> Result<JsonRpcRequest, JsonRpcResponse> {
    let value: Value = serde_json::from_slice(body).map_err(|error| {
        JsonRpcResponse::error(
            JsonRpcId::Null,
            A2AError::parse_error(error.to_string()).to_jsonrpc_error(),
        )
    })?;
    let id = value
        .get("id")
        .cloned()
        .and_then(|id| serde_json::from_value(id).ok())
        .unwrap_or(JsonRpcId::Null);
    // Check presence before serde turns an explicit null into None.
    if value
        .get("params")
        .is_some_and(|params| !params.is_object() && !params.is_array())
    {
        return Err(JsonRpcResponse::error(
            id,
            A2AError::invalid_request("params must be an object or array").to_jsonrpc_error(),
        ));
    }
    let request: JsonRpcRequest = serde_json::from_value(value).map_err(|error| {
        JsonRpcResponse::error(
            id,
            A2AError::invalid_request(error.to_string()).to_jsonrpc_error(),
        )
    })?;
    if request.jsonrpc != "2.0" {
        return Err(JsonRpcResponse::error(
            request.id,
            A2AError::invalid_request("jsonrpc must be 2.0").to_jsonrpc_error(),
        ));
    }
    Ok(request)
}

pub fn decode_error(error: JsonRpcError) -> Result<A2AError, serde_json::Error> {
    let details = error
        .data
        .map(crate::deserialize_error_details)
        .transpose()?;
    Ok(A2AError {
        code: error.code,
        message: error.message,
        details,
    })
}
