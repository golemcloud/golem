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

use super::hash::SqlBlake3Hash;
use anyhow::anyhow;
use golem_common::model::account::{AccountEmail, AccountId, AccountSummary};
use golem_common::model::component::{ComponentId, ComponentName, ComponentRevision};
use golem_common::model::tool::{HostToolId, RegisteredTool, ToolName, ToolSource};
use golem_common::model::tool_release::{
    SystemToolAvailability, SystemToolReleaseProvision, ToolRelease, ToolReleaseId,
    ToolReleaseLifecycle, ToolReleaseOrigin, tool_metadata_digest,
};
use golem_common::schema::tool::Tool;
use golem_service_base::repo::{Blob, SqlDateTime};
use sqlx::FromRow;
use uuid::Uuid;

pub const TOOL_RELEASE_SOURCE_COMPONENT: i16 = 0;
pub const TOOL_RELEASE_SOURCE_HOST: i16 = 1;
pub const TOOL_RELEASE_LIFECYCLE_PUBLISHED: i16 = 0;
pub const TOOL_RELEASE_LIFECYCLE_DE_PUBLISHED: i16 = 1;
pub const TOOL_RELEASE_ORIGIN_ORDINARY: i16 = 0;
pub const TOOL_RELEASE_ORIGIN_PROTECTED_SYSTEM: i16 = 1;
pub const SYSTEM_TOOL_AVAILABILITY_GRANTABLE: i16 = 0;
pub const SYSTEM_TOOL_AVAILABILITY_AUTO_GRANTED: i16 = 1;
pub const SYSTEM_TOOL_AVAILABILITY_AMBIENT: i16 = 2;

#[derive(Debug, Clone, PartialEq, FromRow)]
pub struct ToolReleaseRecord {
    pub tool_release_id: Uuid,
    pub owner_account_id: Uuid,
    pub tool_name: String,
    pub tool_version: String,
    pub source_kind: i16,
    pub tool_definition: Blob<Tool>,
    pub metadata_version: String,
    pub metadata_digest: SqlBlake3Hash,
    pub lifecycle: i16,
    pub origin: i16,
    pub system_availability: Option<i16>,
    pub created_at: SqlDateTime,
    pub created_by: Uuid,
    pub state_changed_at: SqlDateTime,
    pub state_changed_by: Uuid,
    pub component_id: Option<Uuid>,
    pub component_revision: Option<i64>,
    pub component_name: Option<String>,
    pub host_tool_id: Option<String>,
    pub implementation_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, FromRow)]
pub struct ToolReleaseWithOwnerRecord {
    #[sqlx(flatten)]
    pub release: ToolReleaseRecord,
    pub owner_account_name: String,
    pub owner_account_email: String,
}

impl ToolReleaseRecord {
    pub fn from_registered_tool(tool: &RegisteredTool, actor: AccountId) -> anyhow::Result<Self> {
        let name = tool
            .definition
            .name()
            .ok_or_else(|| anyhow!("published tool definition has no root name"))?;
        let now = SqlDateTime::now();
        let mut record = Self {
            tool_release_id: ToolReleaseId::new().0,
            owner_account_id: tool.owner_account_id.0,
            tool_name: name.to_string(),
            tool_version: tool.definition.version.clone(),
            source_kind: TOOL_RELEASE_SOURCE_COMPONENT,
            tool_definition: Blob::new(tool.definition.clone()),
            metadata_version: tool.metadata_version.clone(),
            metadata_digest: tool_metadata_digest(&tool.metadata_version, &tool.definition)?.into(),
            lifecycle: TOOL_RELEASE_LIFECYCLE_PUBLISHED,
            origin: TOOL_RELEASE_ORIGIN_ORDINARY,
            system_availability: None,
            created_at: now.clone(),
            created_by: actor.0,
            state_changed_at: now,
            state_changed_by: actor.0,
            component_id: None,
            component_revision: None,
            component_name: None,
            host_tool_id: None,
            implementation_version: None,
        };
        record.set_source(&tool.source);
        Ok(record)
    }

    pub fn from_system_provision(
        owner_account_id: AccountId,
        provision: SystemToolReleaseProvision,
        actor: AccountId,
    ) -> anyhow::Result<Self> {
        if provision.definition.name() != Some(provision.name.as_str())
            || provision.definition.version != provision.version
        {
            return Err(anyhow!(
                "system tool release coordinate does not match its definition"
            ));
        }
        if !matches!(provision.source, ToolSource::Host { .. }) {
            return Err(anyhow!(
                "protected system tool releases must use a host source"
            ));
        }
        let now = SqlDateTime::now();
        let mut record = Self {
            tool_release_id: ToolReleaseId::new().0,
            owner_account_id: owner_account_id.0,
            tool_name: provision.name.into_inner(),
            tool_version: provision.version,
            source_kind: TOOL_RELEASE_SOURCE_HOST,
            metadata_digest: tool_metadata_digest(
                &provision.metadata_version,
                &provision.definition,
            )?
            .into(),
            tool_definition: Blob::new(provision.definition),
            metadata_version: provision.metadata_version,
            lifecycle: TOOL_RELEASE_LIFECYCLE_PUBLISHED,
            origin: TOOL_RELEASE_ORIGIN_PROTECTED_SYSTEM,
            system_availability: Some(availability_to_i16(provision.availability)),
            created_at: now.clone(),
            created_by: actor.0,
            state_changed_at: now,
            state_changed_by: actor.0,
            component_id: None,
            component_revision: None,
            component_name: None,
            host_tool_id: None,
            implementation_version: None,
        };
        record.set_source(&provision.source);
        Ok(record)
    }

    fn set_source(&mut self, source: &ToolSource) {
        match source {
            ToolSource::Component {
                component_id,
                component_revision,
                component_name,
            } => {
                self.source_kind = TOOL_RELEASE_SOURCE_COMPONENT;
                self.component_id = Some(component_id.0);
                self.component_revision = Some((*component_revision).into());
                self.component_name = Some(component_name.0.clone());
            }
            ToolSource::Host {
                host_tool_id,
                implementation_version,
            } => {
                self.source_kind = TOOL_RELEASE_SOURCE_HOST;
                self.host_tool_id = Some(host_tool_id.as_str().to_string());
                self.implementation_version = Some(implementation_version.clone());
            }
        }
    }

    pub fn immutable_fields_match(&self, other: &Self) -> bool {
        self.owner_account_id == other.owner_account_id
            && self.tool_name == other.tool_name
            && self.tool_version == other.tool_version
            && self.source_kind == other.source_kind
            && self.tool_definition == other.tool_definition
            && self.metadata_version == other.metadata_version
            && self.metadata_digest == other.metadata_digest
            && self.origin == other.origin
            && self.system_availability == other.system_availability
            && self.component_id == other.component_id
            && self.component_revision == other.component_revision
            && self.component_name == other.component_name
            && self.host_tool_id == other.host_tool_id
            && self.implementation_version == other.implementation_version
    }
}

impl TryFrom<ToolReleaseRecord> for ToolRelease {
    type Error = anyhow::Error;

    fn try_from(value: ToolReleaseRecord) -> Result<Self, Self::Error> {
        let source = match value.source_kind {
            TOOL_RELEASE_SOURCE_COMPONENT => ToolSource::Component {
                component_id: ComponentId(
                    value
                        .component_id
                        .ok_or_else(|| anyhow!("missing component id"))?,
                ),
                component_revision: ComponentRevision::try_from(
                    value
                        .component_revision
                        .ok_or_else(|| anyhow!("missing component revision"))?,
                )?,
                component_name: ComponentName(
                    value
                        .component_name
                        .ok_or_else(|| anyhow!("missing component name"))?,
                ),
            },
            TOOL_RELEASE_SOURCE_HOST => ToolSource::Host {
                host_tool_id: HostToolId::try_from(
                    value
                        .host_tool_id
                        .ok_or_else(|| anyhow!("missing host tool id"))?,
                )
                .map_err(anyhow::Error::msg)?,
                implementation_version: value
                    .implementation_version
                    .ok_or_else(|| anyhow!("missing implementation version"))?,
            },
            other => return Err(anyhow!("unknown tool release source kind {other}")),
        };

        Ok(Self {
            id: ToolReleaseId(value.tool_release_id),
            owner_account_id: AccountId(value.owner_account_id),
            name: ToolName::try_from(value.tool_name).map_err(anyhow::Error::msg)?,
            version: value.tool_version,
            source,
            definition: value.tool_definition.into_value(),
            metadata_version: value.metadata_version,
            metadata_digest: value.metadata_digest.into(),
            lifecycle: lifecycle_from_i16(value.lifecycle)?,
            origin: origin_from_i16(value.origin)?,
            system_availability: value
                .system_availability
                .map(availability_from_i16)
                .transpose()?,
            created_at: value.created_at.into(),
            created_by: AccountId(value.created_by),
            state_changed_at: value.state_changed_at.into(),
            state_changed_by: AccountId(value.state_changed_by),
        })
    }
}

impl ToolReleaseWithOwnerRecord {
    pub fn owner(&self) -> AccountSummary {
        AccountSummary {
            id: AccountId(self.release.owner_account_id),
            name: self.owner_account_name.clone(),
            email: AccountEmail::new(self.owner_account_email.clone()),
        }
    }
}

fn lifecycle_from_i16(value: i16) -> anyhow::Result<ToolReleaseLifecycle> {
    match value {
        TOOL_RELEASE_LIFECYCLE_PUBLISHED => Ok(ToolReleaseLifecycle::Published),
        TOOL_RELEASE_LIFECYCLE_DE_PUBLISHED => Ok(ToolReleaseLifecycle::DePublished),
        other => Err(anyhow!("unknown tool release lifecycle {other}")),
    }
}

fn origin_from_i16(value: i16) -> anyhow::Result<ToolReleaseOrigin> {
    match value {
        TOOL_RELEASE_ORIGIN_ORDINARY => Ok(ToolReleaseOrigin::Ordinary),
        TOOL_RELEASE_ORIGIN_PROTECTED_SYSTEM => Ok(ToolReleaseOrigin::ProtectedSystem),
        other => Err(anyhow!("unknown tool release origin {other}")),
    }
}

fn availability_to_i16(value: SystemToolAvailability) -> i16 {
    match value {
        SystemToolAvailability::Grantable => SYSTEM_TOOL_AVAILABILITY_GRANTABLE,
        SystemToolAvailability::AutoGranted => SYSTEM_TOOL_AVAILABILITY_AUTO_GRANTED,
        SystemToolAvailability::Ambient => SYSTEM_TOOL_AVAILABILITY_AMBIENT,
    }
}

fn availability_from_i16(value: i16) -> anyhow::Result<SystemToolAvailability> {
    match value {
        SYSTEM_TOOL_AVAILABILITY_GRANTABLE => Ok(SystemToolAvailability::Grantable),
        SYSTEM_TOOL_AVAILABILITY_AUTO_GRANTED => Ok(SystemToolAvailability::AutoGranted),
        SYSTEM_TOOL_AVAILABILITY_AMBIENT => Ok(SystemToolAvailability::Ambient),
        other => Err(anyhow!("unknown system tool availability {other}")),
    }
}
