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

use super::account::AccountId;
use super::base64::Base64;
use super::component::ComponentRevision;
use super::ComponentId;
use crate::model::Empty;
use crate::{declare_structs, declare_transparent_newtypes, declare_unions, newtype_uuid};

newtype_uuid!(PluginRegistrationId, golem_api_grpc::proto::golem::component::PluginRegistrationId);

declare_transparent_newtypes! {
    pub struct PluginWasmFileKey(pub String);
}

declare_structs! {
    pub struct PluginRegistrationDto {
        pub id: PluginRegistrationId,
        pub account_id: AccountId,
        pub name: String,
        pub version: String,
        pub description: String,
        pub icon: Base64,
        pub homepage: String,
        pub spec: PluginSpecDto,
    }

    pub struct PluginRegistrationCreation {
        pub name: String,
        pub version: String,
        pub description: String,
        pub icon: Base64,
        pub homepage: String,
        pub spec: PluginSpecDto,
    }

    pub struct ComponentTransformerPluginSpec {
        pub provided_wit_package: Option<String>,
        pub json_schema: Option<serde_json::Value>,
        pub validate_url: String,
        pub transform_url: String,
    }

    pub struct OplogProcessorPluginSpec {
        pub component_id: ComponentId,
        pub component_revision: ComponentRevision
    }
}

declare_unions! {
    pub enum PluginSpecDto {
        ComponentTransformer(ComponentTransformerPluginSpec),
        OplogProcessor(OplogProcessorPluginSpec),
        App(Empty),
        Library(Empty)
    }
}

#[cfg(feature = "protobuf")]
mod protobuf {
    use super::*;

    impl TryFrom<golem_api_grpc::proto::golem::component::PluginTypeSpecificDefinition>
        for PluginSpecDto
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

    impl From<ComponentTransformerPluginSpec>
        for golem_api_grpc::proto::golem::component::ComponentTransformerDefinition
    {
        fn from(value: ComponentTransformerPluginSpec) -> Self {
            Self {
                provided_wit_package: value.provided_wit_package,
                json_schema: value.json_schema.map(|js| serde_json::to_string(&js).expect("Failed to serialize json schema")),
                validate_url: value.validate_url,
                transform_url: value.transform_url,
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::component::ComponentTransformerDefinition>
        for ComponentTransformerPluginSpec
    {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::component::ComponentTransformerDefinition,
        ) -> Result<Self, Self::Error> {
            Ok(Self {
                provided_wit_package: value.provided_wit_package,
                json_schema: value.json_schema.map(|s| serde_json::from_str(&s)).transpose().map_err(|e| format!("Failed parsing json schema: {e}"))?,
                validate_url: value.validate_url,
                transform_url: value.transform_url,
            })
        }
    }

    impl From<OplogProcessorPluginSpec>
        for golem_api_grpc::proto::golem::component::OplogProcessorDefinition
    {
        fn from(value: OplogProcessorPluginSpec) -> Self {
            Self {
                component_id: Some(value.component_id.into()),
                component_version: value.component_revision.0,
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::component::OplogProcessorDefinition>
        for OplogProcessorPluginSpec
    {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::component::OplogProcessorDefinition,
        ) -> Result<Self, Self::Error> {
            Ok(Self {
                component_id: value
                    .component_id
                    .ok_or("Missing component_id")?
                    .try_into()?,
                component_revision: ComponentRevision(value.component_version),
            })
        }
    }

    impl From<LibraryPluginDefinition>
        for golem_api_grpc::proto::golem::component::LibraryPluginDefinition
    {
        fn from(value: LibraryPluginDefinition) -> Self {
            golem_api_grpc::proto::golem::component::LibraryPluginDefinition {
                blob_storage_key: value.blob_storage_key.0,
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::component::LibraryPluginDefinition>
        for LibraryPluginDefinition
    {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::component::LibraryPluginDefinition,
        ) -> Result<Self, Self::Error> {
            Ok(LibraryPluginDefinition {
                blob_storage_key: PluginWasmFileKey(value.blob_storage_key),
            })
        }
    }

    impl From<AppPluginDefinition> for golem_api_grpc::proto::golem::component::AppPluginDefinition {
        fn from(value: AppPluginDefinition) -> Self {
            golem_api_grpc::proto::golem::component::AppPluginDefinition {
                blob_storage_key: value.blob_storage_key.0,
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::component::AppPluginDefinition> for AppPluginDefinition {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::component::AppPluginDefinition,
        ) -> Result<Self, Self::Error> {
            Ok(AppPluginDefinition {
                blob_storage_key: PluginWasmFileKey(value.blob_storage_key),
            })
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::component::PluginRegistration> for PluginRegistrationDto {
        type Error = String;
        fn try_from(value: golem_api_grpc::proto::golem::component::PluginRegistration) -> Result<Self, Self::Error> {
            Ok(PluginRegistrationDto {
                id: value.id.ok_or("Missing plugin id")?.try_into()?,
                account_id: value.account_id.ok_or("Missing account id")?.try_into()?,
                name: value.name,
                version: value.version,
                description: value.description,
                icon: Base64(value.icon),
                homepage: value.homepage,
                specs: value.specs.ok_or("Missing plugin specs")?.try_into()?,
            })
        }
    }
}
