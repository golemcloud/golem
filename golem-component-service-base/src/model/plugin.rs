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
        ComponentTransformerDefinition, DefaultPluginScope, OplogProcessorDefinition,
        PluginDefinition, PluginOwner, PluginScope, PluginTypeSpecificDefinition,
        PluginWasmFileKey,
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

pub struct PluginDefinitionCreation<Scope: PluginScope> {
    pub name: String,
    pub version: String,
    pub description: String,
    pub icon: Vec<u8>,
    pub homepage: String,
    pub specs: PluginTypeSpecificCreation,
    pub scope: Scope,
}

impl<Scope: PluginScope> PluginDefinitionCreation<Scope> {
    pub fn into_definition<Owner: PluginOwner>(
        self,
        id: PluginId,
        owner: Owner,
        specs: PluginTypeSpecificDefinition,
    ) -> PluginDefinition<Owner, Scope> {
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

impl TryFrom<golem_api_grpc::proto::golem::component::PluginDefinitionCreation>
    for PluginDefinitionCreation<DefaultPluginScope>
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::PluginDefinitionCreation,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            name: value.name,
            version: value.version,
            description: value.description,
            icon: value.icon,
            homepage: value.homepage,
            specs: value.specs.ok_or("Missing plugin specs")?.try_into()?,
            scope: value.scope.ok_or("Missing plugin scope")?.try_into()?,
        })
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
