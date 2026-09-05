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

use super::audit::ImmutableAuditFields;
use super::tool_release::ToolReleaseWithOwnerRecord;
use golem_common::model::account::{AccountId, AccountSummary};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::environment_tool_grant::{
    EnvironmentToolGrant, EnvironmentToolGrantId, EnvironmentToolGrantLifecycle,
    EnvironmentToolGrantWithDetails,
};
use golem_common::model::tool_release::{ToolRelease, ToolReleaseId, ToolReleaseMetadata};
use golem_service_base::repo::SqlDateTime;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, FromRow)]
pub struct EnvironmentToolGrantRecord {
    pub environment_tool_grant_id: Uuid,
    pub environment_id: Uuid,
    pub tool_release_id: Uuid,
    pub protected: bool,
    pub automatic: bool,
    pub follow_coordinates: bool,
    pub state_changed_at: SqlDateTime,
    pub state_changed_by: Uuid,
    #[sqlx(flatten)]
    pub audit: ImmutableAuditFields,
}

impl EnvironmentToolGrantRecord {
    pub fn creation(
        environment_id: EnvironmentId,
        tool_release_id: ToolReleaseId,
        protected: bool,
        automatic: bool,
        follow_coordinates: bool,
        actor: AccountId,
    ) -> Self {
        let now = SqlDateTime::now();
        Self {
            environment_tool_grant_id: EnvironmentToolGrantId::new().0,
            environment_id: environment_id.0,
            tool_release_id: tool_release_id.0,
            protected,
            automatic,
            follow_coordinates,
            state_changed_at: now,
            state_changed_by: actor.0,
            audit: ImmutableAuditFields::new(actor.0),
        }
    }
}

impl From<EnvironmentToolGrantRecord> for EnvironmentToolGrant {
    fn from(value: EnvironmentToolGrantRecord) -> Self {
        Self {
            id: EnvironmentToolGrantId(value.environment_tool_grant_id),
            environment_id: EnvironmentId(value.environment_id),
            tool_release_id: ToolReleaseId(value.tool_release_id),
            protected: value.protected,
            automatic: value.automatic,
            follow_coordinates: value.follow_coordinates,
            lifecycle: grant_lifecycle(value.audit.deleted_at.is_some()),
            created_at: value.audit.created_at.into(),
            created_by: AccountId(value.audit.created_by),
            state_changed_at: value.state_changed_at.into(),
            state_changed_by: AccountId(value.state_changed_by),
        }
    }
}

#[derive(Debug, Clone, PartialEq, FromRow)]
pub struct EnvironmentToolGrantWithDetailsRecord {
    pub environment_tool_grant_id: Uuid,
    pub environment_id: Uuid,
    pub protected: bool,
    pub automatic: bool,
    pub follow_coordinates: bool,
    pub grant_created_at: SqlDateTime,
    pub grant_created_by: Uuid,
    pub grant_state_changed_at: SqlDateTime,
    pub grant_state_changed_by: Uuid,
    pub grant_deleted_at: Option<SqlDateTime>,
    pub grant_deleted_by: Option<Uuid>,
    #[sqlx(flatten)]
    pub release: ToolReleaseWithOwnerRecord,
}

impl TryFrom<EnvironmentToolGrantWithDetailsRecord> for EnvironmentToolGrantWithDetails {
    type Error = anyhow::Error;

    fn try_from(value: EnvironmentToolGrantWithDetailsRecord) -> Result<Self, Self::Error> {
        let owner: AccountSummary = value.release.owner();
        let release: ToolRelease = value.release.release.try_into()?;
        Ok(Self {
            grant: EnvironmentToolGrant {
                id: EnvironmentToolGrantId(value.environment_tool_grant_id),
                environment_id: EnvironmentId(value.environment_id),
                tool_release_id: release.id,
                protected: value.protected,
                automatic: value.automatic,
                follow_coordinates: value.follow_coordinates,
                lifecycle: grant_lifecycle(value.grant_deleted_at.is_some()),
                created_at: value.grant_created_at.into(),
                created_by: AccountId(value.grant_created_by),
                state_changed_at: value.grant_state_changed_at.into(),
                state_changed_by: AccountId(value.grant_state_changed_by),
            },
            release: ToolReleaseMetadata::from(&release),
            release_owner: owner,
        })
    }
}

fn grant_lifecycle(deleted: bool) -> EnvironmentToolGrantLifecycle {
    if deleted {
        EnvironmentToolGrantLifecycle::Deleted
    } else {
        EnvironmentToolGrantLifecycle::Active
    }
}
