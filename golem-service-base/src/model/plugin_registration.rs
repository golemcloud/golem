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
use golem_common::model::plugin_registration::PluginWasmFileKey;
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
    pub blob_storage_key: PluginWasmFileKey,
}

#[derive(Debug, Clone)]
pub struct LibraryPluginSpec {
    pub blob_storage_key: PluginWasmFileKey,
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

mod protobuf {
    use super::*;

    impl TryFrom<golem_api_grpc::proto::golem::component::PluginTypeSpecificDefinition>
        for PluginSpec
    {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::component::PluginTypeSpecificDefinition,
        ) -> Result<Self, Self::Error> {
            match value.definition.ok_or("Missing plugin type specific definition")? {
                golem_api_grpc::proto::golem::component::plugin_type_specific_definition::Definition::ComponentTransformer(value) => Ok(Self::ComponentTransformer(value.try_into()?)),
                golem_api_grpc::proto::golem::component::plugin_type_specific_definition::Definition::OplogProcessor(value) => Ok(Self::OplogProcessor(value.try_into()?)),
                golem_api_grpc::proto::golem::component::plugin_type_specific_definition::Definition::Library(value) => Ok(Self::Library(value.try_into()?)),
                golem_api_grpc::proto::golem::component::plugin_type_specific_definition::Definition::App(value) => Ok(Self::App(value.try_into()?))
            }
        }
    }

    impl From<LibraryPluginSpec>
        for golem_api_grpc::proto::golem::component::LibraryPluginDefinition
    {
        fn from(value: LibraryPluginSpec) -> Self {
            golem_api_grpc::proto::golem::component::LibraryPluginDefinition {
                blob_storage_key: value.blob_storage_key.0,
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::component::LibraryPluginDefinition>
        for LibraryPluginSpec
    {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::component::LibraryPluginDefinition,
        ) -> Result<Self, Self::Error> {
            Ok(Self {
                blob_storage_key: PluginWasmFileKey(value.blob_storage_key),
            })
        }
    }

    impl From<AppPluginSpec> for golem_api_grpc::proto::golem::component::AppPluginDefinition {
        fn from(value: AppPluginSpec) -> Self {
            golem_api_grpc::proto::golem::component::AppPluginDefinition {
                blob_storage_key: value.blob_storage_key.0,
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::component::AppPluginDefinition> for AppPluginSpec {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::component::AppPluginDefinition,
        ) -> Result<Self, Self::Error> {
            Ok(Self {
                blob_storage_key: PluginWasmFileKey(value.blob_storage_key),
            })
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::component::PluginRegistration> for PluginRegistration {
        type Error = String;
        fn try_from(value: golem_api_grpc::proto::golem::component::PluginRegistration) -> Result<Self, Self::Error> {
            Ok(Self {
                id: value.id.ok_or("Missing plugin id")?.try_into()?,
                account_id: value.account_id.ok_or("Missing account id")?.try_into()?,
                name: value.name,
                version: value.version,
                description: value.description,
                icon: value.icon,
                homepage: value.homepage,
                spec: value.specs.ok_or("Missing plugin specs")?.try_into()?,
            })
        }
    }
}
