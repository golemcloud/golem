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

use bytes::Bytes;
use golem_common::model::{
    plugin::{
        ComponentTransformerDefinition, OplogProcessorDefinition, PluginDefinition, PluginOwner,
        PluginScope, PluginTypeSpecificDefinition, PluginWasmFileKey,
    },
    PluginId,
};
use golem_service_base::replayable_stream::BoxReplayableStream;

pub enum PluginWasmFileReference {
    BlobStorage(PluginWasmFileKey),
    Data(BoxReplayableStream<'static, Result<Bytes, String>, String>),
}

pub struct LibraryPluginCreation {
    pub data: PluginWasmFileReference,
}

pub struct AppPluginCreation {
    pub data: PluginWasmFileReference,
}

pub enum PluginTypeSpecificCreation {
    ComponentTransformer(ComponentTransformerDefinition),
    OplogProcessor(OplogProcessorDefinition),
    Library(LibraryPluginCreation),
    App(AppPluginCreation),
}

pub struct PluginDefinitionCreation {
    pub name: String,
    pub version: String,
    pub description: String,
    pub icon: Vec<u8>,
    pub homepage: String,
    pub specs: PluginTypeSpecificCreation,
    pub scope: PluginScope,
}

impl PluginDefinitionCreation {
    pub fn into_definition(
        self,
        id: PluginId,
        owner: PluginOwner,
        specs: PluginTypeSpecificDefinition,
    ) -> PluginDefinition {
        PluginDefinition {
            id,
            name: self.name,
            version: self.version,
            description: self.description,
            icon: self.icon,
            homepage: self.homepage,
            scope: self.scope,
            owner,
            specs,
            deleted: false,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::v1::CreatePluginRequest>
    for PluginDefinitionCreation
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::v1::CreatePluginRequest,
    ) -> Result<Self, Self::Error> {
        let plugin = value.plugin.ok_or("missing plugin definition")?;

        let converted = PluginDefinitionCreation {
            name: plugin.name,
            version: plugin.version,
            description: plugin.description,
            icon: plugin.icon,
            homepage: plugin.homepage,
            specs: plugin.specs.ok_or("missing specs")?.try_into()?,
            scope: plugin.scope.ok_or("missing scope")?.try_into()?,
        };

        Ok(converted)
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::PluginTypeSpecificDefinition>
    for PluginTypeSpecificCreation
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::PluginTypeSpecificDefinition,
    ) -> Result<Self, Self::Error> {
        match value.definition.ok_or("Missing plugin type specific definition")? {
            golem_api_grpc::proto::golem::component::plugin_type_specific_definition::Definition::ComponentTransformer(value) => Ok(PluginTypeSpecificCreation::ComponentTransformer(value.try_into()?)),
            golem_api_grpc::proto::golem::component::plugin_type_specific_definition::Definition::OplogProcessor(value) => Ok(PluginTypeSpecificCreation::OplogProcessor(value.try_into()?)),
            golem_api_grpc::proto::golem::component::plugin_type_specific_definition::Definition::Library(value) => Ok(PluginTypeSpecificCreation::Library(value.try_into()?)),
            golem_api_grpc::proto::golem::component::plugin_type_specific_definition::Definition::App(value) => Ok(PluginTypeSpecificCreation::App(value.try_into()?))
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::LibraryPluginDefinition>
    for LibraryPluginCreation
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::LibraryPluginDefinition,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            data: PluginWasmFileReference::BlobStorage(PluginWasmFileKey(value.blob_storage_key)),
        })
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::AppPluginDefinition> for AppPluginCreation {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::AppPluginDefinition,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            data: PluginWasmFileReference::BlobStorage(PluginWasmFileKey(value.blob_storage_key)),
        })
    }
}
