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
use crate::base_model::tool::{ToolName, ToolSource};
use crate::schema::tool::Tool;
use crate::{declare_enums, declare_structs, declare_unions, newtype_uuid};
use chrono::{DateTime, Utc};

newtype_uuid!(ToolReleaseId);

pub type ToolReleaseSource = ToolSource;

declare_enums! {
    pub enum ToolReleaseLifecycle {
        Published,
        DePublished,
        Superseded,
    }

    pub enum ToolReleaseOrigin {
        Ordinary,
        ProtectedSystem,
    }

    pub enum SystemToolAvailability {
        Grantable,
        AutoGranted,
        Ambient,
    }
}

declare_structs! {
    pub struct ToolRelease {
        pub id: ToolReleaseId,
        pub owner_account_id: AccountId,
        pub name: ToolName,
        pub version: String,
        pub source: ToolReleaseSource,
        pub definition: Tool,
        pub metadata_version: String,
        pub metadata_digest: Hash,
        pub immutable: bool,
        pub lifecycle: ToolReleaseLifecycle,
        pub origin: ToolReleaseOrigin,
        pub system_availability: Option<SystemToolAvailability>,
        pub created_at: DateTime<Utc>,
        pub created_by: AccountId,
        pub state_changed_at: DateTime<Utc>,
        pub state_changed_by: AccountId,
    }

    /// Safe release metadata available to a consumer through an active environment grant.
    /// Executable source identities remain publisher-only.
    pub struct ToolReleaseMetadata {
        pub id: ToolReleaseId,
        pub name: ToolName,
        pub version: String,
        pub definition: Tool,
        pub metadata_version: String,
        pub metadata_digest: Hash,
        pub source_digest: Hash,
    }

    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    #[cfg_attr(feature = "full", desert(evolution()))]
    pub struct ToolReleaseById {
        pub release_id: ToolReleaseId,
    }

    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    #[cfg_attr(feature = "full", desert(evolution()))]
    pub struct ToolReleaseByCoordinates {
        pub account: AccountEmail,
        pub name: ToolName,
        pub version: String,
    }

    pub struct SystemToolReleaseProvision {
        pub name: ToolName,
        pub version: String,
        pub source: ToolReleaseSource,
        pub definition: Tool,
        pub metadata_version: String,
        pub availability: SystemToolAvailability,
    }
}

declare_unions! {
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    #[cfg_attr(feature = "full", desert(evolution()))]
    pub enum ToolReleaseReference {
        ById(ToolReleaseById),
        ByCoordinates(ToolReleaseByCoordinates),
    }
}

impl From<&ToolRelease> for ToolReleaseMetadata {
    fn from(value: &ToolRelease) -> Self {
        Self {
            id: value.id,
            name: value.name.clone(),
            version: value.version.clone(),
            definition: value.definition.clone(),
            metadata_version: value.metadata_version.clone(),
            metadata_digest: value.metadata_digest,
            source_digest: tool_source_digest(&value.source),
        }
    }
}

pub fn tool_source_digest(source: &ToolReleaseSource) -> Hash {
    let mut input = Vec::from(b"golem:tool-source:v1\0".as_slice());
    match source {
        ToolReleaseSource::Component {
            component_id,
            component_revision,
            component_name,
        } => {
            input.extend_from_slice(b"component\0");
            input.extend_from_slice(component_id.0.as_bytes());
            input.extend_from_slice(&component_revision.get().to_le_bytes());
            input.extend_from_slice(component_name.0.as_bytes());
        }
        ToolReleaseSource::Host {
            host_tool_id,
            implementation_version,
        } => {
            input.extend_from_slice(b"host\0");
            input.extend_from_slice(host_tool_id.as_str().as_bytes());
            input.push(0);
            input.extend_from_slice(implementation_version.as_bytes());
        }
    }
    blake3::hash(&input).into()
}
