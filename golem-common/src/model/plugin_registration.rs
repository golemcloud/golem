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

newtype_uuid!(
    PluginRegistrationId,
    golem_api_grpc::proto::golem::component::PluginRegistrationId
);

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

mod protobuf {
    use super::*;

    impl From<ComponentTransformerPluginSpec>
        for golem_api_grpc::proto::golem::component::ComponentTransformerDefinition
    {
        fn from(value: ComponentTransformerPluginSpec) -> Self {
            Self {
                provided_wit_package: value.provided_wit_package,
                json_schema: value
                    .json_schema
                    .map(|js| serde_json::to_string(&js).expect("Failed to serialize json schema")),
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
                json_schema: value
                    .json_schema
                    .map(|s| serde_json::from_str(&s))
                    .transpose()
                    .map_err(|e| format!("Failed parsing json schema: {e}"))?,
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
}
