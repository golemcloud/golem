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

use crate::base_model::environment::EnvironmentId;
use crate::base_model::validate_lower_kebab_case_identifier;
use crate::{
    declare_enums, declare_revision, declare_structs, declare_transparent_newtypes, newtype_uuid,
};
use derive_more::Display;
use std::str::FromStr;

newtype_uuid!(
    SecuritySchemeId,
    golem_api_grpc::proto::golem::registry::SecuritySchemeId
);

declare_revision!(SecuritySchemeRevision);

declare_transparent_newtypes! {
    #[derive(Display, Eq, Hash, PartialOrd, Ord)]
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    #[cfg_attr(feature = "full", desert(transparent))]
    pub struct SecuritySchemeName(pub String);
}

impl TryFrom<String> for SecuritySchemeName {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        validate_lower_kebab_case_identifier("Security Scheme", &value)?;
        Ok(SecuritySchemeName(value))
    }
}

impl FromStr for SecuritySchemeName {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s.to_string())
    }
}

declare_structs! {
    pub struct SecuritySchemeCreation {
        pub name: SecuritySchemeName,
        pub provider_type: Provider,
        pub client_id: String,
        pub client_secret: String,
        pub redirect_url: String,
        pub scopes: Vec<String>,
    }

    pub struct SecuritySchemeUpdate {
        pub current_revision: SecuritySchemeRevision,
        pub provider_type: Option<Provider>,
        pub client_id: Option<String>,
        pub client_secret: Option<String>,
        pub redirect_url: Option<String>,
        pub scopes: Option<Vec<String>>,
    }

    pub struct SecuritySchemeDto {
        pub id: SecuritySchemeId,
        pub revision: SecuritySchemeRevision,
        pub name: SecuritySchemeName,
        pub environment_id: EnvironmentId,
        pub provider_type: Provider,
        pub client_id: String,
        pub redirect_url: String,
        pub scopes: Vec<String>,
    }
}

declare_enums! {
    pub enum Provider {
        Google,
        Facebook,
        Microsoft,
        Gitlab,
    }
}
