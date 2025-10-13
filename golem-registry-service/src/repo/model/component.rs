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
use golem_common::model::component::{
    ComponentFilePath, ComponentFilePermissions, ComponentName, ComponentRevision,
    InitialComponentFile, InitialComponentFileKey, InstalledPlugin,
};
use golem_common::model::component::{ComponentId, ComponentType};
use golem_common::model::component_metadata::{
    ComponentMetadata, DynamicLinkedInstance, DynamicLinkedWasmRpc,
};
use golem_common::model::deployment::DeploymentPlanComponentEntry;
use golem_common::model::diff::{self, Hash, Hashable};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::plugin_registration::{PluginPriority, PluginRegistrationId};
use golem_service_base::repo::RepoError;
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

#[derive(Clone, PartialEq)]
pub struct SqlComponentMetadata {
    metadata: ComponentMetadata,
}

impl SqlComponentMetadata {
    pub fn new(metadata: ComponentMetadata) -> Self {
        Self { metadata }
    }

    pub fn as_common_model(&self) -> &ComponentMetadata {
        &self.metadata
    }

    pub fn into_common_model(self) -> ComponentMetadata {
        self.metadata
    }
}

impl Debug for SqlComponentMetadata {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.metadata.fmt(f)
    }
}

impl Deref for SqlComponentMetadata {
    type Target = ComponentMetadata;

    fn deref(&self) -> &Self::Target {
        &self.metadata
    }
}

impl From<SqlComponentMetadata> for ComponentMetadata {
    fn from(metadata: SqlComponentMetadata) -> Self {
        metadata.metadata
    }
}

impl From<ComponentMetadata> for SqlComponentMetadata {
    fn from(metadata: ComponentMetadata) -> Self {
        Self::new(metadata)
    }
}

impl<DB: Database> sqlx::Type<DB> for SqlComponentMetadata
where
    Vec<u8>: sqlx::Type<DB>,
{
    fn type_info() -> DB::TypeInfo {
        <Vec<u8>>::type_info()
    }

    fn compatible(ty: &DB::TypeInfo) -> bool {
        <Vec<u8>>::compatible(ty)
    }
}

#[repr(u8)]
enum ComponentMetadataSerializationVersion {
    V1 = 1,
}

impl ComponentMetadataSerializationVersion {
    fn from_u8(version: u8) -> Option<Self> {
        match version {
            1 => Some(Self::V1),
            _ => None,
        }
    }
}

impl TryFrom<u8> for ComponentMetadataSerializationVersion {
    type Error = String;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match Self::from_u8(value) {
            Some(version) => Ok(version),
            None => Err(format!(
                "Unknown component metadata serialization version: {value}"
            )),
        }
    }
}

impl<'q, DB: Database> sqlx::Encode<'q, DB> for SqlComponentMetadata
where
    Vec<u8>: sqlx::Encode<'q, DB>,
{
    fn encode_by_ref(
        &self,
        buf: &mut <DB as Database>::ArgumentBuffer<'q>,
    ) -> Result<IsNull, BoxDynError> {
        use golem_api_grpc::proto::golem::component::ComponentMetadata as ComponentMetadataProto;
        use prost::Message;

        let metadata_proto = ComponentMetadataProto::from(self.metadata.clone());

        let mut buffer_proto = Vec::with_capacity(metadata_proto.encoded_len());
        buffer_proto.push(ComponentMetadataSerializationVersion::V1 as u8);
        metadata_proto.encode(&mut buffer_proto)?;

        buffer_proto.encode_by_ref(buf)
    }

    fn size_hint(&self) -> usize {
        blake3::OUT_LEN
    }
}

impl<'r, DB: Database> sqlx::Decode<'r, DB> for SqlComponentMetadata
where
    Vec<u8>: sqlx::Decode<'r, DB>,
{
    fn decode(value: <DB as Database>::ValueRef<'r>) -> Result<Self, BoxDynError> {
        use golem_api_grpc::proto::golem::component::ComponentMetadata as ComponentMetadataProto;
        use prost::Message;

        let bytes = <Vec<u8> as sqlx::Decode<DB>>::decode(value)?;
        let (version, data) = bytes.split_at(1);
        let version: u8 = version[0];

        match ComponentMetadataSerializationVersion::try_from(version)? {
            ComponentMetadataSerializationVersion::V1 => Ok(Self {
                metadata: ComponentMetadata::try_from(
                    ComponentMetadataProto::decode(data).map_err(|err| {
                        format!("Failed to deserialize component metadata v1: {err}")
                    })?,
                )?,
            }),
        }
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
    pub revision_id: i64, // NOTE: set by repo during insert
    pub version: String,
    pub hash: SqlBlake3Hash, // NOTE: set by repo during insert
    #[sqlx(flatten)]
    pub audit: DeletableRevisionAuditFields,
    pub component_type: i32,
    pub size: i32,
    pub metadata: SqlComponentMetadata,
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
    pub fn ensure_first(self) -> Self {
        Self {
            revision_id: 0,
            audit: self.audit.ensure_new(),
            ..self
        }
    }

    pub fn ensure_new(self, current_revision_id: i64) -> Self {
        Self {
            revision_id: current_revision_id + 1,
            audit: self.audit.ensure_new(),
            ..self
        }
    }

    pub fn deletion(created_by: Uuid, component_id: Uuid, current_revision_id: i64) -> Self {
        Self {
            component_id,
            revision_id: current_revision_id + 1,
            version: "".to_string(),
            hash: SqlBlake3Hash::empty(),
            audit: DeletableRevisionAuditFields::deletion(created_by),
            component_type: 0,
            size: 0,
            metadata: ComponentMetadata::default().into(),
            env: Default::default(),
            original_env: Default::default(),
            object_store_key: "".to_string(),
            binary_hash: SqlBlake3Hash::empty(),
            transformed_object_store_key: "".to_string(),
            original_files: vec![],
            plugins: vec![],
            files: vec![],
        }
    }

    pub fn to_diffable(&self) -> diff::Component {
        diff::Component {
            metadata: diff::ComponentMetadata {
                version: Some(self.version.clone()),
                component_type: ComponentType::from_repr(self.component_type)
                    .expect("expected valid component type"),
                env: self
                    .env
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
                dynamic_linking_wasm_rpc: self
                    .metadata
                    .dynamic_linking()
                    .iter()
                    .map(|(name, link)| match link {
                        DynamicLinkedInstance::WasmRpc(DynamicLinkedWasmRpc { targets }) => (
                            name.clone(),
                            targets
                                .iter()
                                .map(|(name, target)| {
                                    (
                                        name.clone(),
                                        diff::ComponentWasmRpcTarget {
                                            interface_name: target.interface_name.clone(),
                                            component_name: target.component_name.clone(),
                                            component_type: target.component_type,
                                        },
                                    )
                                })
                                .collect::<BTreeMap<_, _>>(),
                        ),
                    })
                    .collect(),
            }
            .into(),
            binary_hash: self.binary_hash.into_blake3_hash().into(),
            files_by_path: self
                .original_files
                .iter()
                .map(|file| {
                    (
                        file.file_path.clone(),
                        diff::ComponentFile {
                            hash: file.hash.into_blake3_hash().into(),
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
                            plugin_id: plugin.plugin_id,
                            parameters: plugin.parameters.0.clone(),
                        },
                    )
                })
                .collect(),
        }
    }

    pub fn update_hash(&mut self) {
        self.hash = self.to_diffable().hash().into_blake3().into()
    }

    pub fn with_updated_hash(mut self) -> Self {
        self.update_hash();
        self
    }

    pub fn from_model(value: FinalizedComponentRevision, actor: &AccountId) -> Self {
        let component_id = value.component_id.0;

        Self {
            files: value
                .files
                .into_iter()
                .map(|f| ComponentFileRecord::from_model(f, component_id, actor))
                .collect(),
            plugins: value
                .installed_plugins
                .into_iter()
                .map(|p| ComponentPluginInstallationRecord::from_model(p, component_id, actor))
                .collect(),
            original_files: value
                .original_files
                .into_iter()
                .map(|f| ComponentFileRecord::from_model(f, component_id, actor))
                .collect(),
            component_id,
            revision_id: 0,
            version: value
                .metadata
                .root_package_version()
                .clone()
                .unwrap_or_default(),
            component_type: value.component_type as i32,
            size: value.component_size as i32,
            metadata: value.metadata.into(),
            hash: SqlBlake3Hash::empty(),
            original_env: Json(value.original_env),
            env: Json(value.env),
            audit: DeletableRevisionAuditFields::new(actor.0),
            object_store_key: value.object_store_key,
            transformed_object_store_key: value.transformed_object_store_key,
            binary_hash: value.wasm_hash.into_blake3().into(),
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

impl TryFrom<ComponentExtRevisionRecord> for Component {
    type Error = RepoError;

    fn try_from(value: ComponentExtRevisionRecord) -> Result<Self, Self::Error> {
        Ok(Self {
            id: ComponentId(value.revision.component_id),
            revision: ComponentRevision(value.revision.revision_id as u64),
            environment_id: EnvironmentId(value.environment_id),
            component_name: ComponentName(value.name),
            component_size: value.revision.size as u64,
            metadata: value.revision.metadata.into(),
            created_at: value.revision.audit.created_at.into(),
            component_type: ComponentType::from_repr(value.revision.component_type)
                .ok_or(anyhow!("Failed converting component type"))?,
            files: value
                .revision
                .files
                .into_iter()
                .map(|f| f.try_into())
                .collect::<Result<_, _>>()?,
            installed_plugins: value
                .revision
                .plugins
                .into_iter()
                .map(|p| p.into())
                .collect(),
            env: value.revision.env.0,
            object_store_key: value.revision.object_store_key,
            wasm_hash: blake3::Hash::from(value.revision.binary_hash).into(),
            original_files: value
                .revision
                .original_files
                .into_iter()
                .map(|f| f.try_into())
                .collect::<Result<_, _>>()?,
            original_env: value.revision.original_env.0,
            transformed_object_store_key: value.revision.transformed_object_store_key,
        })
    }
}

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct ComponentFileRecord {
    pub component_id: Uuid,
    // Note: Set by repo during insert
    pub revision_id: i64,
    pub file_path: String,
    pub hash: SqlBlake3Hash, // NOTE: expected to be set by service-layer
    #[sqlx(flatten)]
    pub audit: RevisionAuditFields,
    pub file_key: String,
    pub file_permissions: SqlComponentFilePermissions,
}

impl ComponentFileRecord {
    pub fn ensure_component(self, component_id: Uuid, revision_id: i64, created_by: Uuid) -> Self {
        Self {
            component_id,
            revision_id,
            audit: RevisionAuditFields {
                created_by,
                ..self.audit
            },
            ..self
        }
    }

    fn from_model(file: InitialComponentFile, component_id: Uuid, actor: &AccountId) -> Self {
        Self {
            component_id,
            revision_id: 0,
            file_path: file.path.to_abs_string(),
            file_key: file.key.0.clone(),
            file_permissions: file.permissions.into(),
            audit: RevisionAuditFields::new(actor.0),
            // TODO: The key is the content hash currently, reuse it here
            hash: SqlBlake3Hash::empty(),
        }
    }
}

impl TryFrom<ComponentFileRecord> for InitialComponentFile {
    type Error = RepoError;
    fn try_from(value: ComponentFileRecord) -> Result<Self, Self::Error> {
        Ok(Self {
            key: InitialComponentFileKey(value.file_key),
            path: ComponentFilePath::from_abs_str(&value.file_path)
                .map_err(|e| anyhow!("Failed converting component file record to model: {e}"))?,
            permissions: value.file_permissions.into(),
        })
    }
}

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct ComponentPluginInstallationRecord {
    pub component_id: Uuid, // NOTE: set by repo during insert
    pub revision_id: i64,   // NOTE: set by repo during insert
    pub priority: i32,
    #[sqlx(flatten)]
    pub audit: RevisionAuditFields,
    pub plugin_id: Uuid,        // NOTE: required for insert
    pub plugin_name: String,    // NOTE: returned by repo, not required to set
    pub plugin_version: String, // NOTE: returned by repo, not required to set
    pub parameters: Json<BTreeMap<String, String>>,
}

impl ComponentPluginInstallationRecord {
    pub fn ensure_component(self, component_id: Uuid, revision_id: i64, created_by: Uuid) -> Self {
        Self {
            component_id,
            revision_id,
            audit: RevisionAuditFields {
                created_by,
                ..self.audit
            },
            ..self
        }
    }

    fn from_model(
        plugin_installation: InstalledPlugin,
        component_id: Uuid,
        actor: &AccountId,
    ) -> Self {
        Self {
            component_id,
            revision_id: 0,
            plugin_id: plugin_installation.plugin_id.0,
            plugin_name: "".to_string(),
            plugin_version: "".to_string(),
            audit: RevisionAuditFields::new(actor.0),
            priority: plugin_installation.priority.0,
            parameters: Json::from(
                plugin_installation
                    .parameters
                    .into_iter()
                    .collect::<BTreeMap<_, _>>(),
            ),
        }
    }
}

impl From<ComponentPluginInstallationRecord> for InstalledPlugin {
    fn from(value: ComponentPluginInstallationRecord) -> Self {
        Self {
            plugin_id: PluginRegistrationId(value.plugin_id),
            priority: PluginPriority(value.priority),
            parameters: value.parameters.0,
        }
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
            hash: Hash::new(value.hash.into_blake3_hash()),
        }
    }
}
