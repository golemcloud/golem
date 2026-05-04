// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source Available License v1.1 (the "License");
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

use super::deployment::DeployRepoError;
use crate::repo::model::audit::{AuditFields, DeletableRevisionAuditFields};
use crate::repo::model::hash::SqlBlake3Hash;
use anyhow::anyhow;
use golem_common::error_forwarding;
use golem_common::model::account::AccountId;
use golem_common::model::application::ApplicationId;
use golem_common::model::component::ComponentId;
use golem_common::model::component::{ComponentName, ComponentRevision};
use golem_common::model::component_metadata::ComponentMetadata;
use golem_common::model::deployment::DeploymentPlanComponentEntry;
use golem_common::model::diff::{self, Hashable};
use golem_common::model::environment::EnvironmentId;
use golem_service_base::model::component::Component;
use golem_service_base::repo::Blob;
use golem_service_base::repo::NumericU64;
use golem_service_base::repo::RepoError;
use golem_wasm::json::ValueAndTypeJsonExtensions;
use sqlx::FromRow;
use std::fmt::Debug;
use uuid::Uuid;

use golem_common::base_model::json::NormalizedJsonValue;
use golem_common::model::diff::AgentTypeProvisionConfig;

#[derive(Debug, thiserror::Error)]
pub enum ComponentRepoError {
    #[error("There is already a component with this name in the environment")]
    ComponentViolatesUniqueness,
    #[error("Concurrent modification")]
    ConcurrentModification,
    #[error("Version already exists: {version}")]
    VersionAlreadyExists { version: String },
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

error_forwarding!(ComponentRepoError, RepoError);

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct ComponentRecord {
    pub component_id: Uuid,
    pub name: String,
    pub environment_id: Uuid,
    #[sqlx(flatten)]
    pub audit: AuditFields,
    pub current_revision_id: i64,
}

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct ComponentRevisionRecord {
    pub component_id: Uuid,
    pub revision_id: i64,
    pub hash: SqlBlake3Hash, // NOTE: set by repo during insert
    #[sqlx(flatten)]
    pub audit: DeletableRevisionAuditFields,
    pub size: NumericU64,
    pub metadata: Blob<ComponentMetadata>,
    pub object_store_key: String,
    pub binary_hash: SqlBlake3Hash, // NOTE: expected to be provided by service-layer
}

impl ComponentRevisionRecord {
    pub(in crate::repo) fn for_recreation(
        mut self,
        component_id: Uuid,
        revision_id: i64,
    ) -> Result<Self, ComponentRepoError> {
        let revision: ComponentRevision = revision_id.try_into()?;
        let next_revision_id = revision.next()?.into();

        self.component_id = component_id;
        self.revision_id = next_revision_id;

        Ok(self)
    }

    pub fn creation(
        component_id: ComponentId,
        component_size: u64,
        metadata: ComponentMetadata,
        wasm_hash: diff::Hash,
        object_store_key: String,
        actor: AccountId,
    ) -> Self {
        let component_id = component_id.0;
        let revision_id: i64 = ComponentRevision::INITIAL.into();

        Self {
            component_id,
            revision_id,
            hash: SqlBlake3Hash::empty(),
            audit: DeletableRevisionAuditFields::new(actor.0),
            size: component_size.into(),
            metadata: Blob::new(metadata),
            object_store_key,
            binary_hash: wasm_hash.into(),
        }
    }

    pub fn from_model(value: Component, actor: AccountId) -> Self {
        let component_id = value.id.0;
        let revision_id: i64 = value.revision.into();

        Self {
            component_id,
            revision_id,
            size: value.component_size.into(),
            metadata: Blob::new(value.metadata),
            hash: SqlBlake3Hash::empty(),
            audit: DeletableRevisionAuditFields::new(actor.0),
            object_store_key: value.object_store_key,
            binary_hash: value.wasm_hash.into(),
        }
    }

    pub fn deletion(
        created_by: Uuid,
        component_id: Uuid,
        revision_id: i64,
    ) -> Result<Self, ComponentRepoError> {
        let mut value = Self {
            component_id,
            revision_id,
            hash: SqlBlake3Hash::empty(),
            audit: DeletableRevisionAuditFields::deletion(created_by),
            size: 0.into(),
            metadata: Blob::new(ComponentMetadata::default()),
            object_store_key: "".to_string(),
            binary_hash: SqlBlake3Hash::empty(),
        };
        value.update_hash()?;
        Ok(value)
    }

    pub fn to_diffable(&self) -> Result<diff::Component, diff::DiffError> {
        let agent_type_provision_configs =
            self.metadata
                .value()
                .agent_type_provision_configs()
                .iter()
                .map(|(name, config)| {
                    let state =
                        AgentTypeProvisionConfig {
                            env: config.env.clone(),
                            config: config
                                .config
                                .iter()
                                .map(|e| {
                                    Ok((
                                        e.path.join("."),
                                        NormalizedJsonValue::new(e.value.to_json_value().map_err(
                                            |reason| diff::DiffError::TypedConfigJsonConversion {
                                                operation:
                                                    "component revision to_diffable config entry conversion",
                                                path: e.path.join("."),
                                                reason,
                                            },
                                        )?),
                                    ))
                                })
                                .collect::<Result<_, _>>()?,
                            files_by_path: config
                                .files
                                .iter()
                                .map(|file| {
                                    (
                                        file.path.to_abs_string(),
                                        diff::AgentFile {
                                            hash: file.content_hash.0,
                                            permissions: file.permissions,
                                        }
                                        .into(),
                                    )
                                })
                                .collect(),
                            plugins_by_grant_id: config
                                .plugins
                                .iter()
                                .map(|plugin| {
                                    (
                                        plugin.environment_plugin_grant_id.0,
                                        diff::PluginInstallation {
                                            priority: plugin.priority.0,
                                            name: plugin.plugin_name.clone(),
                                            version: plugin.plugin_version.clone(),
                                            grant_id: plugin.environment_plugin_grant_id.0,
                                            parameters: plugin.parameters.clone(),
                                        },
                                    )
                                })
                                .collect(),
                        };
                    Ok((name.0.clone(), state.into()))
                })
                .collect::<Result<_, _>>()?;

        Ok(diff::Component {
            wasm_hash: self.binary_hash.into(),
            agent_type_provision_configs,
        })
    }

    pub fn update_hash(&mut self) -> Result<(), ComponentRepoError> {
        self.hash = self
            .to_diffable()
            .map_err(|err| ComponentRepoError::InternalError(anyhow!(err)))?
            .hash()
            .map_err(|err| ComponentRepoError::InternalError(anyhow!(err)))?
            .into();
        Ok(())
    }

    pub fn with_updated_hash(mut self) -> Result<Self, ComponentRepoError> {
        self.update_hash()?;
        Ok(self)
    }
}

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct ComponentExtRevisionRecord {
    pub name: String,
    pub environment_id: Uuid,
    #[sqlx(flatten)]
    pub revision: ComponentRevisionRecord,
}

impl ComponentExtRevisionRecord {
    pub fn try_into_model(
        self,
        application_id: ApplicationId,
        account_id: AccountId,
    ) -> Result<Component, RepoError> {
        Ok(Component {
            id: ComponentId(self.revision.component_id),
            revision: self.revision.revision_id.try_into()?,
            environment_id: EnvironmentId(self.environment_id),
            application_id,
            account_id,
            component_name: ComponentName(self.name),
            component_size: self.revision.size.into(),
            metadata: self.revision.metadata.into_value(),
            created_at: self.revision.audit.created_at.into(),
            object_store_key: self.revision.object_store_key,
            wasm_hash: self.revision.binary_hash.into(),
            hash: self.revision.hash.into(),
        })
    }
}

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct ComponentRevisionIdentityRecord {
    pub component_id: Uuid,
    pub name: String,
    pub revision_id: i64,
    pub hash: SqlBlake3Hash,
}

impl TryFrom<ComponentRevisionIdentityRecord> for DeploymentPlanComponentEntry {
    type Error = DeployRepoError;
    fn try_from(value: ComponentRevisionIdentityRecord) -> Result<Self, Self::Error> {
        Ok(Self {
            id: ComponentId(value.component_id),
            revision: value.revision_id.try_into()?,
            name: ComponentName(value.name),
            hash: value.hash.into(),
        })
    }
}
