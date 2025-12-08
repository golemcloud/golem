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

use crate::model::component::{Component, FinalizedComponentRevision};
use crate::repo::model::audit::{AuditFields, DeletableRevisionAuditFields, RevisionAuditFields};
use crate::repo::model::hash::SqlBlake3Hash;
use anyhow::anyhow;
use golem_common::error_forwarding;
use golem_common::model::account::AccountId;
use golem_common::model::application::ApplicationId;
use golem_common::model::component::ComponentId;
use golem_common::model::component::PluginPriority;
use golem_common::model::component::{
    ComponentFileContentHash, ComponentFilePath, ComponentFilePermissions, ComponentName,
    ComponentRevision, InitialComponentFile, InstalledPlugin,
};
use golem_common::model::component_metadata::{ComponentMetadata, dynamic_linking_to_diffable};
use golem_common::model::deployment::DeploymentPlanComponentEntry;
use golem_common::model::diff::{self, Hashable};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::environment_plugin_grant::EnvironmentPluginGrantId;
use golem_common::model::plugin_registration::PluginRegistrationId;
use golem_service_base::repo::RepoError;
use golem_service_base::repo::blob::Blob;
use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use sqlx::types::Json;
use sqlx::{Database, FromRow};
use std::collections::BTreeMap;
use std::fmt::Debug;
use std::ops::Deref;
use uuid::Uuid;

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

#[derive(Clone, Copy, PartialEq)]
pub struct SqlComponentFilePermissions {
    permissions: ComponentFilePermissions,
}

impl SqlComponentFilePermissions {
    pub fn new(permissions: ComponentFilePermissions) -> Self {
        Self { permissions }
    }

    pub fn to_common_model(&self) -> ComponentFilePermissions {
        self.permissions
    }
}

impl Debug for SqlComponentFilePermissions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Debug::fmt(&self.permissions, f)
    }
}

impl From<ComponentFilePermissions> for SqlComponentFilePermissions {
    fn from(permissions: ComponentFilePermissions) -> Self {
        Self::new(permissions)
    }
}

impl From<SqlComponentFilePermissions> for ComponentFilePermissions {
    fn from(permissions: SqlComponentFilePermissions) -> Self {
        permissions.permissions
    }
}

impl Deref for SqlComponentFilePermissions {
    type Target = ComponentFilePermissions;

    fn deref(&self) -> &Self::Target {
        &self.permissions
    }
}

impl<DB: Database> sqlx::Type<DB> for SqlComponentFilePermissions
where
    for<'a> &'a str: sqlx::Type<DB>,
{
    fn type_info() -> DB::TypeInfo {
        <&str>::type_info()
    }

    fn compatible(ty: &DB::TypeInfo) -> bool {
        <&str>::compatible(ty)
    }
}

impl<'q, DB: Database> sqlx::Encode<'q, DB> for SqlComponentFilePermissions
where
    &'q str: sqlx::Encode<'q, DB>,
{
    fn encode_by_ref(
        &self,
        buf: &mut <DB as Database>::ArgumentBuffer<'q>,
    ) -> Result<IsNull, BoxDynError> {
        self.permissions.as_compact_str().encode_by_ref(buf)
    }

    fn size_hint(&self) -> usize {
        self.permissions.as_compact_str().size_hint()
    }
}

impl<'r, DB: Database> sqlx::Decode<'r, DB> for SqlComponentFilePermissions
where
    &'r str: sqlx::Decode<'r, DB>,
{
    fn decode(value: <DB as Database>::ValueRef<'r>) -> Result<Self, BoxDynError> {
        Ok(Self {
            permissions: ComponentFilePermissions::from_compact_str(<&'r str>::decode(value)?)?,
        })
    }
}

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
    pub version: String,
    pub hash: SqlBlake3Hash, // NOTE: set by repo during insert
    #[sqlx(flatten)]
    pub audit: DeletableRevisionAuditFields,
    pub size: i32,
    pub metadata: Blob<ComponentMetadata>,
    pub original_env: Json<BTreeMap<String, String>>,
    pub env: Json<BTreeMap<String, String>>,
    pub object_store_key: String,
    pub binary_hash: SqlBlake3Hash, // NOTE: expected to be provided by service-layer
    pub transformed_object_store_key: String,

    #[sqlx(skip)]
    pub original_files: Vec<ComponentFileRecord>,
    #[sqlx(skip)]
    pub plugins: Vec<ComponentPluginInstallationRecord>,

    #[sqlx(skip)]
    pub files: Vec<ComponentFileRecord>,
}

impl ComponentRevisionRecord {
    pub fn for_recreation(
        mut self,
        component_id: Uuid,
        revision_id: i64,
    ) -> Result<Self, ComponentRepoError> {
        let revision: ComponentRevision = revision_id.into();
        let next_revision_id = revision.next()?.into();

        for file in &mut self.original_files {
            file.component_id = component_id;
            file.revision_id = next_revision_id;
        }
        for file in &mut self.files {
            file.component_id = component_id;
            file.revision_id = next_revision_id;
        }
        for plugin in &mut self.plugins {
            plugin.component_id = component_id;
            plugin.revision_id = next_revision_id;
        }

        self.component_id = component_id;
        self.revision_id = next_revision_id;

        Ok(self)
    }

    pub fn deletion(created_by: Uuid, component_id: Uuid, revision_id: i64) -> Self {
        let mut value = Self {
            component_id,
            revision_id,
            version: "".to_string(),
            hash: SqlBlake3Hash::empty(),
            audit: DeletableRevisionAuditFields::deletion(created_by),
            size: 0,
            metadata: Blob::new(ComponentMetadata::default()),
            env: Default::default(),
            original_env: Default::default(),
            object_store_key: "".to_string(),
            binary_hash: SqlBlake3Hash::empty(),
            transformed_object_store_key: "".to_string(),
            original_files: vec![],
            plugins: vec![],
            files: vec![],
        };
        value.update_hash();
        value
    }

    pub fn to_diffable(&self) -> diff::Component {
        diff::Component {
            metadata: diff::ComponentMetadata {
                version: Some("TODO".to_string()), // TODO: atomic: Some(self.version.clone()),
                env: self
                    .env
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
                dynamic_linking_wasm_rpc: dynamic_linking_to_diffable(
                    self.metadata.value().dynamic_linking(),
                ),
            }
            .into(),
            wasm_hash: self.binary_hash.into(),
            files_by_path: self
                .original_files
                .iter()
                .map(|file| {
                    (
                        file.file_path.clone(),
                        diff::ComponentFile {
                            hash: file.file_content_hash.into(),
                            permissions: file.file_permissions.into(),
                        }
                        .into(),
                    )
                })
                .collect(),
            plugins_by_priority: self
                .plugins
                .iter()
                .map(|plugin| {
                    (
                        plugin.priority.to_string(),
                        diff::PluginInstallation {
                            plugin_id: plugin.plugin_registration_id,
                            parameters: plugin.parameters.0.clone(),
                        },
                    )
                })
                .collect(),
        }
    }

    pub fn update_hash(&mut self) {
        self.hash = self.to_diffable().hash().into()
    }

    pub fn with_updated_hash(mut self) -> Self {
        self.update_hash();
        self
    }

    pub fn from_model(value: FinalizedComponentRevision, actor: &AccountId) -> Self {
        let component_id = value.component_id.0;
        let revision_id: i64 = value.component_revision.into();

        Self {
            files: value
                .files
                .into_iter()
                .map(|f| ComponentFileRecord::from_model(f, component_id, revision_id, actor))
                .collect(),
            plugins: value
                .installed_plugins
                .into_iter()
                .map(|p| {
                    ComponentPluginInstallationRecord::from_model(
                        p,
                        component_id,
                        revision_id,
                        actor,
                    )
                })
                .collect(),
            original_files: value
                .original_files
                .into_iter()
                .map(|f| ComponentFileRecord::from_model(f, component_id, revision_id, actor))
                .collect(),
            component_id,
            revision_id,
            version: value
                .metadata
                .root_package_version()
                .clone()
                .unwrap_or_default(),
            size: value.component_size as i32,
            metadata: Blob::new(value.metadata),
            hash: SqlBlake3Hash::empty(),
            original_env: Json(value.original_env),
            env: Json(value.env),
            audit: DeletableRevisionAuditFields::new(actor.0),
            object_store_key: value.object_store_key,
            transformed_object_store_key: value.transformed_object_store_key,
            binary_hash: value.wasm_hash.into(),
        }
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
            revision: ComponentRevision(self.revision.revision_id as u64),
            environment_id: EnvironmentId(self.environment_id),
            application_id,
            account_id,
            component_name: ComponentName(self.name),
            component_size: self.revision.size as u64,
            metadata: self.revision.metadata.into_value(),
            created_at: self.revision.audit.created_at.into(),
            files: self
                .revision
                .files
                .into_iter()
                .map(|f| f.try_into())
                .collect::<Result<_, _>>()?,
            installed_plugins: self
                .revision
                .plugins
                .into_iter()
                .map(|p| p.try_into())
                .collect::<Result<_, _>>()?,
            env: self.revision.env.0,
            object_store_key: self.revision.object_store_key,
            wasm_hash: self.revision.binary_hash.into(),
            original_files: self
                .revision
                .original_files
                .into_iter()
                .map(|f| f.try_into())
                .collect::<Result<_, _>>()?,
            original_env: self.revision.original_env.0,
            transformed_object_store_key: self.revision.transformed_object_store_key,
            hash: self.revision.hash.into(),
        })
    }
}

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct ComponentFileRecord {
    pub component_id: Uuid,
    pub revision_id: i64,
    pub file_path: String,
    #[sqlx(flatten)]
    pub audit: RevisionAuditFields,
    pub file_content_hash: SqlBlake3Hash,
    pub file_permissions: SqlComponentFilePermissions,
}

impl ComponentFileRecord {
    fn from_model(
        file: InitialComponentFile,
        component_id: Uuid,
        revision_id: i64,
        actor: &AccountId,
    ) -> Self {
        Self {
            component_id,
            revision_id,
            file_path: file.path.to_abs_string(),
            file_content_hash: file.content_hash.0.into(),
            file_permissions: file.permissions.into(),
            audit: RevisionAuditFields::new(actor.0),
        }
    }
}

impl TryFrom<ComponentFileRecord> for InitialComponentFile {
    type Error = RepoError;
    fn try_from(value: ComponentFileRecord) -> Result<Self, Self::Error> {
        Ok(Self {
            content_hash: ComponentFileContentHash(value.file_content_hash.into()),
            path: ComponentFilePath::from_abs_str(&value.file_path)
                .map_err(|e| anyhow!("Failed converting component file record to model: {e}"))?,
            permissions: value.file_permissions.into(),
        })
    }
}

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct ComponentPluginInstallationRecord {
    pub component_id: Uuid,
    pub revision_id: i64,
    pub priority: i32,
    #[sqlx(flatten)]
    pub audit: RevisionAuditFields,
    pub environment_plugin_grant_id: Uuid,
    pub parameters: Json<BTreeMap<String, String>>,

    pub plugin_registration_id: Uuid, // NOTE: not used directly in the repo, but needed for hash calculation
    pub plugin_name: Option<String>,  // NOTE: returned by repo, not required to set
    pub plugin_version: Option<String>, // NOTE: returned by repo, not required to set
    pub oplog_processor_component_id: Option<Uuid>, // NOTE: returned by repo, not required to set
    pub oplog_processor_component_revision_id: Option<i64>, // NOTE: returned by repo, not required to set
}

impl ComponentPluginInstallationRecord {
    fn from_model(
        plugin_installation: InstalledPlugin,
        component_id: Uuid,
        revision_id: i64,
        actor: &AccountId,
    ) -> Self {
        Self {
            component_id,
            revision_id,
            environment_plugin_grant_id: plugin_installation.environment_plugin_grant_id.0,
            audit: RevisionAuditFields::new(actor.0),
            priority: plugin_installation.priority.0,
            parameters: Json::from(
                plugin_installation
                    .parameters
                    .into_iter()
                    .collect::<BTreeMap<_, _>>(),
            ),
            plugin_registration_id: plugin_installation.plugin_registration_id.0,
            plugin_name: None,
            plugin_version: None,
            oplog_processor_component_id: None,
            oplog_processor_component_revision_id: None,
        }
    }
}

impl TryFrom<ComponentPluginInstallationRecord> for InstalledPlugin {
    type Error = anyhow::Error;
    fn try_from(value: ComponentPluginInstallationRecord) -> Result<Self, Self::Error> {
        Ok(Self {
            environment_plugin_grant_id: EnvironmentPluginGrantId(
                value.environment_plugin_grant_id,
            ),
            priority: PluginPriority(value.priority),
            parameters: value.parameters.0,

            plugin_registration_id: PluginRegistrationId(value.plugin_registration_id),
            plugin_name: value.plugin_name.ok_or(anyhow!("missing plugin name"))?,
            plugin_version: value
                .plugin_version
                .ok_or(anyhow!("missing plugin version"))?,
            oplog_processor_component_id: value.oplog_processor_component_id.map(ComponentId),
            oplog_processor_component_revision: value
                .oplog_processor_component_revision_id
                .map(ComponentRevision::from),
        })
    }
}

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct ComponentRevisionIdentityRecord {
    pub component_id: Uuid,
    pub name: String,
    pub revision_id: i64,
    pub version: String,
    pub hash: SqlBlake3Hash,
}

impl From<ComponentRevisionIdentityRecord> for DeploymentPlanComponentEntry {
    fn from(value: ComponentRevisionIdentityRecord) -> Self {
        Self {
            id: ComponentId(value.component_id),
            revision: value.revision_id.into(),
            name: ComponentName(value.name),
            hash: value.hash.into(),
        }
    }
}
