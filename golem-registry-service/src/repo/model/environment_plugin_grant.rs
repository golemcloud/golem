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

use super::audit::ImmutableAuditFields;
use golem_common::error_forwarding;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::environment_plugin_grant::{
    EnvironmentPluginGrant, EnvironmentPluginGrantId,
};
use golem_common::model::plugin_registration::PluginRegistrationId;
use golem_service_base::repo::RepoError;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum EnvironmentPluginGrantRepoError {
    #[error("There is already a grant for this (environment, plugin) combination")]
    PluginGrantViolatesUniqueness,
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

error_forwarding!(EnvironmentPluginGrantRepoError, RepoError);

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct EnvironmentPluginGrantRecord {
    pub environment_plugin_grant_id: Uuid,
    pub environment_id: Uuid,
    pub plugin_id: Uuid,

    #[sqlx(flatten)]
    pub audit: ImmutableAuditFields,
}

impl EnvironmentPluginGrantRecord {
    pub fn from_model(model: EnvironmentPluginGrant, audit: ImmutableAuditFields) -> Self {
        Self {
            environment_plugin_grant_id: model.id.0,
            environment_id: model.environment_id.0,
            plugin_id: model.plugin_registration_id.0,
            audit,
        }
    }
}

impl From<EnvironmentPluginGrantRecord> for EnvironmentPluginGrant {
    fn from(value: EnvironmentPluginGrantRecord) -> Self {
        Self {
            id: EnvironmentPluginGrantId(value.environment_plugin_grant_id),
            environment_id: EnvironmentId(value.environment_id),
            plugin_registration_id: PluginRegistrationId(value.plugin_id),
        }
    }
}
