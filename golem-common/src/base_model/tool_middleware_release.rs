// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use crate::base_model::account::{AccountEmail, AccountId};
use crate::base_model::diff::Hash;
use crate::base_model::tool_middleware::{ToolMiddlewareName, ToolMiddlewareSource};
use crate::schema::tool::ToolMiddleware;
use crate::{declare_enums, declare_structs, declare_unions, newtype_uuid};
use chrono::{DateTime, Utc};
use std::fmt::{Display, Formatter};

newtype_uuid!(ToolMiddlewareReleaseId);
pub type ToolMiddlewareReleaseSource = ToolMiddlewareSource;

declare_enums! {
    pub enum ToolMiddlewareReleaseLifecycle {
        Published,
        DePublished,
        Superseded
    }
    pub enum ToolMiddlewareReleaseOrigin {
        Ordinary,
        ProtectedSystem
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Enum))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub enum ToolMiddlewarePublicationPlanAction {
    NoChange,
    Publish,
    Conflict,
}

impl Display for ToolMiddlewarePublicationPlanAction {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NoChange => "no change",
            Self::Publish => "publish",
            Self::Conflict => "conflict",
        })
    }
}

declare_structs! {
    pub struct ToolMiddlewareRelease {
        pub id: ToolMiddlewareReleaseId,
        pub owner_account_id: AccountId,
        pub name: ToolMiddlewareName,
        pub version: String,
        pub source: ToolMiddlewareReleaseSource,
        pub definition: ToolMiddleware,
        pub metadata_version: String,
        pub metadata_digest: Hash,
        pub immutable: bool,
        pub lifecycle: ToolMiddlewareReleaseLifecycle,
        pub origin: ToolMiddlewareReleaseOrigin,
        pub created_at: DateTime<Utc>,
        pub created_by: AccountId,
        pub state_changed_at: DateTime<Utc>,
        pub state_changed_by: AccountId,
    }
    pub struct ToolMiddlewareReleaseMetadata {
        pub id: ToolMiddlewareReleaseId,
        pub name: ToolMiddlewareName,
        pub version: String,
        pub definition: ToolMiddleware,
        pub metadata_version: String,
        pub metadata_digest: Hash,
        pub source_digest: Hash,
    }
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    #[cfg_attr(feature = "full", desert(evolution()))]
    #[derive(Eq, PartialOrd, Ord, Hash)]
    pub struct ToolMiddlewareReleaseById {
        pub release_id: ToolMiddlewareReleaseId
    }
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    #[cfg_attr(feature = "full", desert(evolution()))]
    #[derive(Eq, PartialOrd, Ord, Hash)]
    pub struct ToolMiddlewareReleaseByCoordinates {
        pub account: AccountEmail,
        pub name: ToolMiddlewareName,
        pub version: String
    }
    pub struct ToolMiddlewarePublication {
        pub name: ToolMiddlewareName,
        pub definition: ToolMiddleware
    }
    #[derive(Eq)]
    pub struct ToolMiddlewarePublicationPlanEntry {
        pub action: ToolMiddlewarePublicationPlanAction,
        pub name: String,
        pub version: String,
        pub reason: Option<String>
    }
}

declare_unions! {
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    #[cfg_attr(feature = "full", desert(evolution()))]
    #[derive(Eq, PartialOrd, Ord, Hash)]
    pub enum ToolMiddlewareReleaseReference {
        ById(ToolMiddlewareReleaseById),
        ByCoordinates(ToolMiddlewareReleaseByCoordinates)
    }
}

impl From<&ToolMiddlewareRelease> for ToolMiddlewareReleaseMetadata {
    fn from(value: &ToolMiddlewareRelease) -> Self {
        Self {
            id: value.id,
            name: value.name.clone(),
            version: value.version.clone(),
            definition: value.definition.clone(),
            metadata_version: value.metadata_version.clone(),
            metadata_digest: value.metadata_digest,
            source_digest: tool_middleware_source_digest(&value.source),
        }
    }
}

pub fn tool_middleware_source_digest(source: &ToolMiddlewareReleaseSource) -> Hash {
    let mut input = Vec::from(b"golem:tool-middleware-source:v1\0".as_slice());
    let ToolMiddlewareReleaseSource::Component {
        component_id,
        component_revision,
        component_name,
    } = source;
    input.extend_from_slice(component_id.0.as_bytes());
    input.extend_from_slice(&component_revision.get().to_le_bytes());
    input.extend_from_slice(component_name.0.as_bytes());
    blake3::hash(&input).into()
}
