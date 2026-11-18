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

use crate::repo::model::audit::DeletableRevisionAuditFields;
use desert_rust::BinaryCodec;
use golem_common::error_forwarding;
use golem_common::model::account::AccountId;
use golem_common::model::agent_config::{SharedConfigId, SharedConfigRevision};
use golem_common::model::environment::EnvironmentId;
use golem_common::schema::config::{
    ConfigFieldKind, ConfigFieldSchema, ConfigOrigin, ConfigSchema,
};
use golem_common::schema::validation::{is_equivalent_cross_graph, validate_value};
use golem_common::schema::{SchemaGraph, SchemaType, SchemaValue};
use golem_service_base::repo::{Blob, RepoError};
use sqlx::FromRow;
use sqlx::types::Json;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum SharedConfigRepoError {
    #[error("A Shared config value already exists at this catalog path")]
    PathAlreadyExists,
    #[error("Shared config value not found")]
    NotFound,
    #[error("config-value-removed")]
    Removed,
    #[error("Concurrent modification")]
    ConcurrentModification,
    #[error("Shared config schema is incompatible with the stored schema")]
    IncompatibleSchema,
    #[error("Invalid Shared config: {0}")]
    InvalidConfig(String),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

error_forwarding!(SharedConfigRepoError, RepoError);

pub struct SharedConfigCreationRecord {
    pub id: SharedConfigId,
    pub environment_id: EnvironmentId,
    pub path: Vec<String>,
    pub schema: SchemaGraph,
    pub value: Option<SchemaValue>,
    pub actor: AccountId,
}

pub struct SharedConfigUpdateRecord {
    pub id: SharedConfigId,
    pub current_revision: SharedConfigRevision,
    pub schema: SchemaGraph,
    pub value: Option<SchemaValue>,
    pub actor: AccountId,
}

#[derive(Debug, Clone, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub struct SharedConfigRevisionData {
    /// Secret leaves are UUID values, never Secret payloads or private revisions.
    /// None is uninitialized; Some(Option { inner: None }) is configured absence.
    pub value: Option<SchemaValue>,
}

#[derive(Debug, Clone, PartialEq, FromRow)]
pub struct SharedConfigRecord {
    pub shared_config_id: Uuid,
    pub environment_id: Uuid,
    pub path: Json<Vec<String>>,
    pub schema: Blob<SchemaGraph>,
    pub revision_id: i64,
    pub revision_data: Blob<SharedConfigRevisionData>,
    #[sqlx(flatten)]
    pub audit: DeletableRevisionAuditFields,
}

impl SharedConfigRecord {
    pub fn check_schema(&self, schema: &SchemaGraph) -> Result<(), SharedConfigRepoError> {
        if !is_equivalent_cross_graph(
            self.schema.value(),
            &self.schema.value().root,
            schema,
            &schema.root,
        ) {
            return Err(SharedConfigRepoError::IncompatibleSchema);
        }
        Ok(())
    }
}

/// The storage schema uses UUID leaves in place of Secret capabilities. The original
/// graph remains fixed on the identity and supplies the Secret payload types.
fn storage_schema(schema: &SchemaGraph) -> Result<SchemaGraph, SharedConfigRepoError> {
    ConfigSchema {
        graph: schema.clone(),
        descriptors: Default::default(),
        fields: vec![ConfigFieldSchema {
            path: vec!["value".into()],
            kind: ConfigFieldKind::Shared,
            value_type: schema.root.clone(),
            default: None,
        }],
    }
    .validate(&ConfigOrigin {
        file: "<shared-config>".into(),
        path: vec![],
        layer: "storage".into(),
    })
    .map_err(|error| SharedConfigRepoError::InvalidConfig(error.to_string()))?;

    let mut storage = schema.clone();
    secret_id_type(&mut storage.root);
    for def in &mut storage.defs {
        secret_id_type(&mut def.body);
    }
    Ok(storage)
}

pub(crate) fn stored_value(
    schema: &SchemaGraph,
    value: Option<SchemaValue>,
) -> Result<SharedConfigRevisionData, SharedConfigRepoError> {
    let storage = storage_schema(schema)?;
    if let Some(value) = &value {
        validate_value(&storage, &storage.root, value)
            .map_err(|errors| SharedConfigRepoError::InvalidConfig(format!("{errors:?}")))?;
    }
    Ok(SharedConfigRevisionData { value })
}

fn secret_id_type(ty: &mut SchemaType) {
    match ty {
        SchemaType::Secret { .. } => *ty = SchemaType::uuid(),
        SchemaType::Record { fields, .. } => fields
            .iter_mut()
            .for_each(|field| secret_id_type(&mut field.body)),
        SchemaType::Tuple { elements, .. } => elements.iter_mut().for_each(secret_id_type),
        SchemaType::Option { inner, .. } => secret_id_type(inner),
        SchemaType::List { element, .. } | SchemaType::FixedList { element, .. } => {
            secret_id_type(element)
        }
        SchemaType::Map { key, value, .. } => {
            secret_id_type(key);
            secret_id_type(value);
        }
        SchemaType::Variant { cases, .. } => cases
            .iter_mut()
            .filter_map(|case| case.payload.as_mut())
            .for_each(secret_id_type),
        SchemaType::Result { spec, .. } => spec
            .ok
            .iter_mut()
            .chain(&mut spec.err)
            .for_each(|ty| secret_id_type(ty)),
        SchemaType::Union { spec, .. } => spec
            .branches
            .iter_mut()
            .for_each(|branch| secret_id_type(&mut branch.body)),
        _ => {}
    }
}
