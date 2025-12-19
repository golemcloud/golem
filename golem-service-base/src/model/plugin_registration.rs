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

use golem_common::model::Empty;
use golem_common::model::account::AccountId;
use golem_common::model::base64::Base64;
use golem_common::model::plugin_registration::WasmContentHash;
use golem_common::model::plugin_registration::{
    ComponentTransformerPluginSpec, OplogProcessorPluginSpec,
};
use golem_common::model::plugin_registration::{
    PluginRegistrationDto, PluginRegistrationId, PluginSpecDto,
};

#[derive(Debug, Clone)]
pub struct PluginRegistration {
    pub id: PluginRegistrationId,
    pub account_id: AccountId,
    pub name: String,
    pub version: String,
    pub description: String,
    pub icon: Vec<u8>,
    pub homepage: String,
    pub spec: PluginSpec,
}

impl From<PluginRegistration> for PluginRegistrationDto {
    fn from(value: PluginRegistration) -> Self {
        Self {
            id: value.id,
            account_id: value.account_id,
            name: value.name,
            version: value.version,
            description: value.description,
            icon: Base64(value.icon),
            homepage: value.homepage,
            spec: value.spec.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AppPluginSpec {
    pub wasm_content_hash: WasmContentHash,
}

#[derive(Debug, Clone)]
pub struct LibraryPluginSpec {
    pub wasm_content_hash: WasmContentHash,
}

#[derive(Debug, Clone)]
pub enum PluginSpec {
    ComponentTransformer(ComponentTransformerPluginSpec),
    OplogProcessor(OplogProcessorPluginSpec),
    App(AppPluginSpec),
    Library(LibraryPluginSpec),
}

impl From<PluginSpec> for PluginSpecDto {
    fn from(value: PluginSpec) -> Self {
        match value {
            PluginSpec::App(_inner) => Self::App(Empty {}),
            PluginSpec::Library(_inner) => Self::Library(Empty {}),
            PluginSpec::ComponentTransformer(inner) => Self::ComponentTransformer(inner),
            PluginSpec::OplogProcessor(inner) => Self::OplogProcessor(inner),
        }
    }
}
