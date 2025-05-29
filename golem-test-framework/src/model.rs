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

use golem_common::model::plugin::{DefaultPluginScope, PluginTypeSpecificDefinition};

#[derive(Debug, Clone)]
pub struct PluginDefinitionCreation {
    pub name: String,
    pub version: String,
    pub description: String,
    pub icon: Vec<u8>,
    pub homepage: String,
    pub scope: DefaultPluginScope,
    pub specs: PluginTypeSpecificDefinition,
}

impl From<PluginDefinitionCreation>
    for golem_api_grpc::proto::golem::component::PluginDefinitionCreation
{
    fn from(value: PluginDefinitionCreation) -> Self {
        golem_api_grpc::proto::golem::component::PluginDefinitionCreation {
            name: value.name,
            version: value.version,
            description: value.description,
            icon: value.icon,
            homepage: value.homepage,
            specs: Some(value.specs.into()),
            scope: Some(value.scope.into()),
        }
    }
}
