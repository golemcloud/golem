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

use crate::model::plugin_manifest::{FromPluginManifest, PluginManifest};
use golem_client::model::PluginTypeSpecificDefinition;
use golem_cloud_client::model::{
    PluginDefinitionCloudPluginOwnerCloudPluginScope, PluginDefinitionCreationCloudPluginScope,
};
use golem_cloud_client::CloudPluginScope;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PluginDefinition(pub PluginDefinitionCloudPluginOwnerCloudPluginScope);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PluginDefinitionCreation(pub PluginDefinitionCreationCloudPluginScope);

impl FromPluginManifest for PluginDefinitionCreation {
    type PluginScope = CloudPluginScope;

    #[allow(unused_variables)]
    #[allow(unreachable_code)]
    fn from_plugin_manifest(
        manifest: PluginManifest,
        scope: Self::PluginScope,
        specs: PluginTypeSpecificDefinition,
        icon: Vec<u8>,
    ) -> Self {
        PluginDefinitionCreation(PluginDefinitionCreationCloudPluginScope {
            name: manifest.name,
            version: manifest.version,
            description: manifest.description,
            icon,
            homepage: manifest.homepage,
            specs: todo!(), // TODO: implement in the next update when we no longer have two versions of everything
            scope,
        })
    }
}
