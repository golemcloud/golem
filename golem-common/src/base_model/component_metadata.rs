// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec, poem_openapi::Union))]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(tag = "type")]
pub enum DynamicLinkedInstance {
    WasmRpc(DynamicLinkedWasmRpc),
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec, poem_openapi::Object))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct DynamicLinkedWasmRpc {
    /// Maps resource names within the dynamic linked interface to target information
    pub targets: HashMap<String, WasmRpcTarget>,
}

impl DynamicLinkedWasmRpc {
    pub fn target(&self, stub_resource: &str) -> Result<WasmRpcTarget, String> {
        self.targets.get(stub_resource).cloned().ok_or_else(|| {
            format!("Resource '{stub_resource}' not found in dynamic linked interface")
        })
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize,
)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec, poem_openapi::Object))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct WasmRpcTarget {
    pub interface_name: String,
    pub component_name: String,
}
