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
use golem_common::model::tool_middleware::{
    RegisteredToolMiddleware, ToolMiddlewareName, ToolMiddlewareSource,
};
use golem_common::model::tool_middleware_release::{
    ToolMiddlewareRelease, ToolMiddlewareReleaseId, ToolMiddlewareReleaseLifecycle,
    ToolMiddlewareReleaseOrigin, tool_middleware_metadata_digest,
};
use golem_common::schema::tool::ToolMiddleware;
use golem_service_base::repo::{Blob, SqlDateTime};
use sqlx::FromRow;
use uuid::Uuid;

pub const TOOL_RELEASE_SOURCE_COMPONENT: i16 = 0;
pub const TOOL_RELEASE_LIFECYCLE_PUBLISHED: i16 = 0;
pub const TOOL_RELEASE_LIFECYCLE_DE_PUBLISHED: i16 = 1;
pub const TOOL_RELEASE_LIFECYCLE_SUPERSEDED: i16 = 2;
pub const TOOL_RELEASE_ORIGIN_ORDINARY: i16 = 0;
pub const TOOL_RELEASE_ORIGIN_PROTECTED_SYSTEM: i16 = 1;

#[derive(Debug, Clone, PartialEq, FromRow)]
pub struct ToolMiddlewareReleaseRecord {
    pub tool_middleware_release_id: Uuid,
    pub owner_account_id: Uuid,
    pub tool_middleware_name: String,
    pub tool_version: String,
    pub source_kind: i16,
    pub tool_definition: Blob<ToolMiddleware>,
    pub metadata_version: String,
    pub metadata_digest: SqlBlake3Hash,
    pub immutable: bool,
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
pub struct ToolMiddlewareReleaseWithOwnerRecord {
    #[sqlx(flatten)]
    pub release: ToolMiddlewareReleaseRecord,
    pub owner_account_name: String,
    pub owner_account_email: String,
}

impl ToolMiddlewareReleaseRecord {
    pub fn from_registered_tool_middleware(
        tool_middleware: &RegisteredToolMiddleware,
        immutable: bool,
        actor: AccountId,
    ) -> anyhow::Result<Self> {
        let name = &tool_middleware.definition.name;
        let now = SqlDateTime::now();
        let mut record = Self {
            tool_middleware_release_id: ToolMiddlewareReleaseId::new().0,
            owner_account_id: tool_middleware.owner_account_id.0,
            tool_middleware_name: name.to_string(),
            tool_version: tool_middleware.definition.version.clone(),
            source_kind: TOOL_RELEASE_SOURCE_COMPONENT,
            tool_definition: Blob::new(tool_middleware.definition.clone()),
            metadata_version: tool_middleware.metadata_version.clone(),
            metadata_digest: tool_middleware_metadata_digest(
                &tool_middleware.metadata_version,
                &tool_middleware.definition,
            )?
            .into(),
            immutable,
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
        record.set_source(&tool_middleware.source);
        Ok(record)
    }

    fn set_source(&mut self, source: &ToolMiddlewareSource) {
        match source {
            ToolMiddlewareSource::Component {
                component_id,
                component_revision,
                component_name,
            } => {
                self.source_kind = TOOL_RELEASE_SOURCE_COMPONENT;
                self.component_id = Some(component_id.0);
                self.component_revision = Some((*component_revision).into());
                self.component_name = Some(component_name.0.clone());
            }
        }
    }

    pub fn immutable_fields_match(&self, other: &Self) -> bool {
        self.owner_account_id == other.owner_account_id
            && self.tool_middleware_name == other.tool_middleware_name
            && self.tool_version == other.tool_version
            && self.source_kind == other.source_kind
            && self.tool_definition == other.tool_definition
            && self.metadata_version == other.metadata_version
            && self.metadata_digest == other.metadata_digest
            && self.origin == other.origin
            && self.component_id == other.component_id
            && self.component_revision == other.component_revision
            && self.component_name == other.component_name
    }

    pub fn publication_content_matches(&self, other: &Self) -> bool {
        self.tool_definition == other.tool_definition
            && self.metadata_version == other.metadata_version
            && self.metadata_digest == other.metadata_digest
    }
}

impl TryFrom<ToolMiddlewareReleaseRecord> for ToolMiddlewareRelease {
    type Error = anyhow::Error;

    fn try_from(value: ToolMiddlewareReleaseRecord) -> Result<Self, Self::Error> {
        let source = match value.source_kind {
            TOOL_RELEASE_SOURCE_COMPONENT => ToolMiddlewareSource::Component {
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
            other => {
                return Err(anyhow!(
                    "unknown tool_middleware release source kind {other}"
                ));
            }
        };

        Ok(Self {
            id: ToolMiddlewareReleaseId(value.tool_middleware_release_id),
            owner_account_id: AccountId(value.owner_account_id),
            name: ToolMiddlewareName::try_from(value.tool_middleware_name)
                .map_err(anyhow::Error::msg)?,
            version: value.tool_version,
            source,
            definition: value.tool_definition.into_value(),
            metadata_version: value.metadata_version,
            metadata_digest: value.metadata_digest.into(),
            immutable: value.immutable,
            lifecycle: lifecycle_from_i16(value.lifecycle)?,
            origin: origin_from_i16(value.origin)?,
            created_at: value.created_at.into(),
            created_by: AccountId(value.created_by),
            state_changed_at: value.state_changed_at.into(),
            state_changed_by: AccountId(value.state_changed_by),
        })
    }
}

impl ToolMiddlewareReleaseWithOwnerRecord {
    pub fn owner(&self) -> AccountSummary {
        AccountSummary {
            id: AccountId(self.release.owner_account_id),
            name: self.owner_account_name.clone(),
            email: AccountEmail::new(self.owner_account_email.clone()),
        }
    }
}

fn lifecycle_from_i16(value: i16) -> anyhow::Result<ToolMiddlewareReleaseLifecycle> {
    match value {
        TOOL_RELEASE_LIFECYCLE_PUBLISHED => Ok(ToolMiddlewareReleaseLifecycle::Published),
        TOOL_RELEASE_LIFECYCLE_DE_PUBLISHED => Ok(ToolMiddlewareReleaseLifecycle::DePublished),
        TOOL_RELEASE_LIFECYCLE_SUPERSEDED => Ok(ToolMiddlewareReleaseLifecycle::Superseded),
        other => Err(anyhow!("unknown tool_middleware release lifecycle {other}")),
    }
}

fn origin_from_i16(value: i16) -> anyhow::Result<ToolMiddlewareReleaseOrigin> {
    match value {
        TOOL_RELEASE_ORIGIN_ORDINARY => Ok(ToolMiddlewareReleaseOrigin::Ordinary),
        TOOL_RELEASE_ORIGIN_PROTECTED_SYSTEM => Ok(ToolMiddlewareReleaseOrigin::ProtectedSystem),
        other => Err(anyhow!("unknown tool_middleware release origin {other}")),
    }
}
