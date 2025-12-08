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
use super::datetime::SqlDateTime;
use super::hash::SqlBlake3Hash;
use super::plugin::PluginRecord;
use golem_common::error_forwarding;
use golem_common::model::account::AccountId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::environment_plugin_grant::{
    EnvironmentPluginGrant, EnvironmentPluginGrantId,
};
use golem_common::model::plugin_registration::{PluginRegistrationDto, PluginRegistrationId};
use golem_service_base::model::plugin_registration::PluginRegistration;
use golem_service_base::repo::RepoError;
use sqlx::FromRow;
use sqlx::types::Json;
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
    pub fn creation(
        environment_id: EnvironmentId,
        plugin_registration_id: PluginRegistrationId,
        actor: AccountId,
    ) -> Self {
        Self {
            environment_plugin_grant_id: EnvironmentPluginGrantId::new().0,
            environment_id: environment_id.0,
            plugin_id: plugin_registration_id.0,
            audit: ImmutableAuditFields::new(actor.0),
        }
    }

    pub fn into_model(self, plugin: PluginRegistrationDto) -> EnvironmentPluginGrant {
        EnvironmentPluginGrant {
            id: EnvironmentPluginGrantId(self.environment_plugin_grant_id),
            environment_id: EnvironmentId(self.environment_id),
            plugin,
        }
    }
}

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct EnvironmentPluginGrantWithDetailsRecord {
    pub environment_plugin_grant_id: Uuid,
    pub environment_id: Uuid,

    #[sqlx(flatten)]
    pub audit: ImmutableAuditFields,

    // flattened plugin fields
    pub plugin_id: Uuid,
    pub plugin_account_id: Uuid,
    pub plugin_name: String,
    pub plugin_version: String,
    pub plugin_description: String,
    pub plugin_icon: Vec<u8>,
    pub plugin_homepage: String,
    pub plugin_plugin_type: i16,

    // for ComponentTransformer plugin type
    pub plugin_provided_wit_package: Option<String>,
    pub plugin_json_schema: Option<Json<serde_json::Value>>,
    pub plugin_validate_url: Option<String>,
    pub plugin_transform_url: Option<String>,

    // for OplogProcessor plugin type
    pub plugin_component_id: Option<Uuid>,
    pub plugin_component_revision_id: Option<i64>,

    // for Library and App plugin type
    pub plugin_wasm_content_hash: Option<SqlBlake3Hash>,

    // plugin audit
    pub plugin_created_at: SqlDateTime,
    pub plugin_created_by: Uuid,
    pub plugin_deleted_at: Option<SqlDateTime>,
    pub plugin_deleted_by: Option<Uuid>,
}

impl TryFrom<EnvironmentPluginGrantWithDetailsRecord> for EnvironmentPluginGrant {
    type Error = anyhow::Error;
    fn try_from(value: EnvironmentPluginGrantWithDetailsRecord) -> Result<Self, Self::Error> {
        let plugin_record = PluginRecord {
            plugin_id: value.plugin_id,
            account_id: value.plugin_account_id,
            name: value.plugin_name,
            version: value.plugin_version,
            audit: ImmutableAuditFields {
                created_at: value.plugin_created_at,
                created_by: value.plugin_created_by,
                deleted_at: value.plugin_deleted_at,
                deleted_by: value.plugin_deleted_by,
            },
            description: value.plugin_description,
            icon: value.plugin_icon,
            homepage: value.plugin_homepage,
            plugin_type: value.plugin_plugin_type,
            provided_wit_package: value.plugin_provided_wit_package,
            json_schema: value.plugin_json_schema,
            validate_url: value.plugin_validate_url,
            transform_url: value.plugin_transform_url,
            component_id: value.plugin_component_id,
            component_revision_id: value.plugin_component_revision_id,
            wasm_content_hash: value.plugin_wasm_content_hash,
        };

        Ok(Self {
            id: EnvironmentPluginGrantId(value.environment_plugin_grant_id),
            environment_id: EnvironmentId(value.environment_id),
            plugin: PluginRegistration::try_from(plugin_record)?.into(),
        })
    }
}
