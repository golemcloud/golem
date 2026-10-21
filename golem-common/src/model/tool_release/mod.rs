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

pub use crate::base_model::tool_release::*;

use crate::model::diff;
use crate::schema::tool::Tool;

pub fn tool_metadata_digest(
    metadata_version: &str,
    definition: &Tool,
) -> anyhow::Result<diff::Hash> {
    let mut input = Vec::from(b"golem:tool-metadata:v1\0".as_slice());
    input.extend_from_slice(metadata_version.as_bytes());
    input.push(0);
    input.extend_from_slice(&desert_rust::serialize_to_byte_vec(definition)?);
    Ok(blake3::hash(&input).into())
}

#[cfg(test)]
mod tests;
