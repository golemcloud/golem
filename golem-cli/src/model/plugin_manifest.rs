// Copyright 2024 Golem Cloud
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

use golem_client::model::{
    PluginDefinitionDefaultPluginOwnerDefaultPluginScope, PluginTypeSpecificDefinition,
};
use golem_common::model::plugin::{DefaultPluginOwner, DefaultPluginScope};
use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PluginTypeSpecificManifest {
    ComponentTransformer(ComponentTransformerManifest),
    OplogProcessor(OplogProcessorManifest),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentTransformerManifest {
    pub provided_wit_package: Option<String>,
    pub json_schema: Option<String>,
    pub validate_url: String,
    pub transform_url: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OplogProcessorManifest {
    pub component: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    pub description: String,
    pub icon: PathBuf,
    pub homepage: String,
    pub specs: PluginTypeSpecificManifest,
}

impl PluginManifest {
    pub fn into_definition<PluginDefinition: FromPluginManifest>(
        self,
        scope: <PluginDefinition as FromPluginManifest>::PluginScope,
        owner: <PluginDefinition as FromPluginManifest>::PluginOwner,
        specs: PluginTypeSpecificDefinition,
        icon: Vec<u8>,
    ) -> PluginDefinition {
        PluginDefinition::from_plugin_manifest(self, scope, owner, specs, icon)
    }
}

pub trait FromPluginManifest {
    type PluginScope;
    type PluginOwner;

    fn from_plugin_manifest(
        manifest: PluginManifest,
        scope: Self::PluginScope,
        owner: Self::PluginOwner,
        specs: PluginTypeSpecificDefinition,
        icon: Vec<u8>,
    ) -> Self;
}

impl FromPluginManifest for PluginDefinitionDefaultPluginOwnerDefaultPluginScope {
    type PluginScope = DefaultPluginScope;
    type PluginOwner = DefaultPluginOwner;

    fn from_plugin_manifest(
        manifest: PluginManifest,
        scope: Self::PluginScope,
        owner: Self::PluginOwner,
        specs: PluginTypeSpecificDefinition,
        icon: Vec<u8>,
    ) -> Self {
        PluginDefinitionDefaultPluginOwnerDefaultPluginScope {
            name: manifest.name,
            version: manifest.version,
            description: manifest.description,
            icon,
            homepage: manifest.homepage,
            specs,
            scope,
            owner,
        }
    }
}
