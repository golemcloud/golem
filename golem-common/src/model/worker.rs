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

use golem_wasm_rpc::IntoValue;
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
pub struct WorkerCreationRequest {
    pub name: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    #[cfg_attr(feature = "poem", oai(default))]
    pub wasi_config_vars: WasiConfigVars,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
pub struct WasiConfigVarsEntry {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::NewType))]
#[cfg_attr(
    feature = "poem",
    oai(from_multipart = false, from_parameter = false, to_header = false)
)]
pub struct WasiConfigVars(pub Vec<WasiConfigVarsEntry>);

impl Default for WasiConfigVars {
    fn default() -> Self {
        Self::new()
    }
}

impl WasiConfigVars {
    pub fn new() -> Self {
        Self(Vec::new())
    }
}

impl From<WasiConfigVars> for BTreeMap<String, String> {
    fn from(value: WasiConfigVars) -> Self {
        value.0.into_iter().map(|e| (e.key, e.value)).collect()
    }
}

impl From<BTreeMap<String, String>> for WasiConfigVars {
    fn from(value: BTreeMap<String, String>) -> Self {
        Self(
            value
                .into_iter()
                .map(|(key, value)| WasiConfigVarsEntry { key, value })
                .collect(),
        )
    }
}

impl IntoValue for WasiConfigVars {
    fn get_type() -> golem_wasm_ast::analysis::AnalysedType {
        BTreeMap::<String, String>::get_type()
    }
    fn into_value(self) -> golem_wasm_rpc::Value {
        BTreeMap::from(self).into_value()
    }
}

#[cfg(feature = "protobuf")]
mod protobuf {
    use super::WasiConfigVars;
    use super::WasiConfigVarsEntry;

    impl From<golem_api_grpc::proto::golem::worker::WasiConfigVars> for WasiConfigVars {
        fn from(value: golem_api_grpc::proto::golem::worker::WasiConfigVars) -> Self {
            Self(
                value
                    .entries
                    .into_iter()
                    .map(|e| WasiConfigVarsEntry {
                        key: e.key,
                        value: e.value,
                    })
                    .collect(),
            )
        }
    }

    impl From<WasiConfigVars> for golem_api_grpc::proto::golem::worker::WasiConfigVars {
        fn from(value: WasiConfigVars) -> Self {
            Self {
                entries: value
                    .0
                    .into_iter()
                    .map(
                        |e| golem_api_grpc::proto::golem::worker::WasiConfigVarsEntry {
                            key: e.key,
                            value: e.value,
                        },
                    )
                    .collect(),
            }
        }
    }
}
