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

//! A2A v1.0.1 wire encoding shared by servers and clients.
//!
//! Protocol types and JSON-RPC envelopes come from `a2a-lf`; payloads use
//! `a2a-pb` ProtoJSON, never the native types' serde representation.
pub use a2a;
pub use a2a_pb::protojson_conv::ProtoJsonPayloadError;

pub mod http;
pub mod jsonrpc;
pub mod stream;
pub mod wire;

pub const MEDIA_TYPE: &str = "application/a2a+json";

fn deserialize_error_details<'de, D>(deserializer: D) -> Result<Vec<a2a::TypedDetail>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let details: Vec<serde_json::Value> = serde::Deserialize::deserialize(deserializer)?;
    if details.iter().any(|detail| {
        detail
            .get("@type")
            .and_then(serde_json::Value::as_str)
            .is_none_or(str::is_empty)
    }) {
        return Err(serde::de::Error::custom(
            "A2A error details require a nonempty string @type",
        ));
    }
    details
        .into_iter()
        .map(serde_json::from_value)
        .collect::<Result<_, _>>()
        .map_err(serde::de::Error::custom)
}

#[cfg(test)]
test_r::enable!();

#[cfg(test)]
mod tests;
