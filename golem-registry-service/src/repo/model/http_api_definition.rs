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

use super::datetime::SqlDateTime;
use crate::repo::model::audit::{AuditFields, DeletableRevisionAuditFields};
use crate::repo::model::hash::SqlBlake3Hash;
use desert_rust::BinaryCodec;
use golem_common::error_forwarding;
use golem_common::model::account::AccountId;
use golem_common::model::deployment::DeploymentPlanHttpApiDefintionEntry;
use golem_common::model::diff;
use golem_common::model::diff::Hashable;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::http_api_definition::{
    GatewayBinding, GatewayBindingType, HttpApiDefinition, HttpApiDefinitionId,
    HttpApiDefinitionName, HttpApiDefinitionRevision, HttpApiDefinitionVersion, HttpApiRoute,
};
use golem_service_base::repo::RepoError;
use golem_service_base::repo::blob::Blob;
use sqlx::FromRow;
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum HttpApiDefinitionRepoError {
    #[error("There is already an api definition with this name in the environment")]
    ApiDefinitionViolatesUniqueness,
    #[error("Concurrent modification")]
    ConcurrentModification,
    #[error("Version already exists: {version}")]
    VersionAlreadyExists { version: String },
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

error_forwarding!(HttpApiDefinitionRepoError, RepoError);

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct HttpApiDefinitionRecord {
    pub http_api_definition_id: Uuid,
    pub name: String,
    pub environment_id: Uuid,
    #[sqlx(flatten)]
    pub audit: AuditFields,
    pub current_revision_id: i64,
}

// Definition field of the HttpApiDefinitionRevisionRecord record. Must be kept backwards compatible
#[derive(Debug, Clone, BinaryCodec, PartialEq)]
#[desert(evolution())]
pub struct HttpApiDefinitionDefinitionBlob {
    pub routes: Vec<HttpApiRoute>,
}

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct HttpApiDefinitionRevisionRecord {
    pub http_api_definition_id: Uuid,
    pub revision_id: i64,
    pub version: String,
    pub hash: SqlBlake3Hash, // NOTE: set by repo during insert
    #[sqlx(flatten)]
    pub audit: DeletableRevisionAuditFields,
    pub definition: Blob<HttpApiDefinitionDefinitionBlob>,
}

impl HttpApiDefinitionRevisionRecord {
    pub fn for_recreation(
        mut self,
        http_api_definition_id: Uuid,
        revision_id: i64,
    ) -> Result<Self, HttpApiDefinitionRepoError> {
        let revision: HttpApiDefinitionRevision = revision_id.try_into()?;
        let next_revision_id = revision.next()?.into();

        self.http_api_definition_id = http_api_definition_id;
        self.revision_id = next_revision_id;

        Ok(self)
    }

    pub fn creation(
        http_api_definition_id: HttpApiDefinitionId,
        version: HttpApiDefinitionVersion,
        routes: Vec<HttpApiRoute>,
        actor: AccountId,
    ) -> Self {
        let mut value = Self {
            http_api_definition_id: http_api_definition_id.0,
            revision_id: HttpApiDefinitionRevision::INITIAL.into(),
            version: version.0,
            hash: SqlBlake3Hash::empty(),
            audit: DeletableRevisionAuditFields::new(actor.0),
            definition: Blob::new(HttpApiDefinitionDefinitionBlob { routes }),
        };
        value.update_hash();
        value
    }

    pub fn from_model(value: HttpApiDefinition, audit: DeletableRevisionAuditFields) -> Self {
        let mut value = Self {
            http_api_definition_id: value.id.0,
            revision_id: value.revision.into(),
            version: value.version.0,
            hash: SqlBlake3Hash::empty(),
            audit,
            definition: Blob::new(HttpApiDefinitionDefinitionBlob {
                routes: value.routes,
            }),
        };
        value.update_hash();
        value
    }

    pub fn deletion(
        created_by: Uuid,
        http_api_definition_id: Uuid,
        current_revision_id: i64,
    ) -> Self {
        let mut value = Self {
            http_api_definition_id,
            revision_id: current_revision_id,
            version: "".to_string(),
            hash: SqlBlake3Hash::empty(),
            audit: DeletableRevisionAuditFields::deletion(created_by),
            definition: Blob::new(HttpApiDefinitionDefinitionBlob { routes: Vec::new() }),
        };
        value.update_hash();
        value
    }

    pub fn to_diffable(&self) -> diff::HttpApiDefinition {
        let mut converted_routes = BTreeMap::new();
        for route in &self.definition.value().routes {
            let http_api_method_and_path = diff::HttpApiMethodAndPath {
                method: route.method.to_string(),
                path: route.path.clone(),
            };
            let binding = match &route.binding {
                GatewayBinding::Worker(inner) => diff::HttpApiDefinitionBinding {
                    binding_type: GatewayBindingType::Worker,
                    component_name: Some(inner.component_name.0.clone()),
                    worker_name: None,
                    idempotency_key: inner.idempotency_key.clone(),
                    invocation_context: inner.invocation_context.clone(),
                    response: Some(inner.response.clone()),
                },
                GatewayBinding::FileServer(inner) => diff::HttpApiDefinitionBinding {
                    binding_type: GatewayBindingType::FileServer,
                    component_name: Some(inner.component_name.0.clone()),
                    worker_name: Some(inner.worker_name.clone()),
                    idempotency_key: None,
                    invocation_context: None,
                    response: Some(inner.response.clone()),
                },
                GatewayBinding::HttpHandler(inner) => diff::HttpApiDefinitionBinding {
                    binding_type: GatewayBindingType::HttpHandler,
                    component_name: Some(inner.component_name.0.clone()),
                    worker_name: Some(inner.worker_name.clone()),
                    idempotency_key: inner.idempotency_key.clone(),
                    invocation_context: inner.invocation_context.clone(),
                    response: Some(inner.response.clone()),
                },
                GatewayBinding::CorsPreflight(inner) => diff::HttpApiDefinitionBinding {
                    binding_type: GatewayBindingType::CorsPreflight,
                    component_name: None,
                    worker_name: None,
                    idempotency_key: None,
                    invocation_context: None,
                    response: inner.response.clone(),
                },
                GatewayBinding::SwaggerUi(_) => diff::HttpApiDefinitionBinding {
                    binding_type: GatewayBindingType::SwaggerUi,
                    component_name: None,
                    worker_name: None,
                    idempotency_key: None,
                    invocation_context: None,
                    response: None,
                },
            };
            let http_api_route = diff::HttpApiRoute {
                binding,
                security: route.security.as_ref().map(|s| s.0.clone()),
            };

            converted_routes.insert(http_api_method_and_path, http_api_route);
        }

        diff::HttpApiDefinition {
            routes: converted_routes,
            version: self.version.clone(),
        }
    }

    pub fn update_hash(&mut self) {
        self.hash = self.to_diffable().hash().into();
    }

    pub fn with_updated_hash(mut self) -> Self {
        self.update_hash();
        self
    }
}

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct HttpApiDefinitionExtRevisionRecord {
    pub name: String,
    pub environment_id: Uuid,

    pub entity_created_at: SqlDateTime,

    #[sqlx(flatten)]
    pub revision: HttpApiDefinitionRevisionRecord,
}

impl HttpApiDefinitionExtRevisionRecord {
    pub fn to_identity(self) -> HttpApiDefinitionRevisionIdentityRecord {
        HttpApiDefinitionRevisionIdentityRecord {
            http_api_definition_id: self.revision.http_api_definition_id,
            name: self.name,
            revision_id: self.revision.revision_id,
            version: self.revision.version,
            hash: self.revision.hash,
        }
    }
}

impl TryFrom<HttpApiDefinitionExtRevisionRecord> for HttpApiDefinition {
    type Error = HttpApiDefinitionRepoError;
    fn try_from(value: HttpApiDefinitionExtRevisionRecord) -> Result<Self, Self::Error> {
        Ok(Self {
            id: HttpApiDefinitionId(value.revision.http_api_definition_id),
            revision: value.revision.revision_id.try_into()?,
            environment_id: EnvironmentId(value.environment_id),
            name: HttpApiDefinitionName(value.name),
            hash: value.revision.hash.into(),
            version: HttpApiDefinitionVersion(value.revision.version),
            routes: value.revision.definition.into_value().routes,
            created_at: value.entity_created_at.into(),
            updated_at: value.revision.audit.created_at.into(),
        })
    }
}

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct HttpApiDefinitionRevisionIdentityRecord {
    pub http_api_definition_id: Uuid,
    pub name: String,
    pub revision_id: i64,
    pub version: String,
    pub hash: SqlBlake3Hash,
}

impl TryFrom<HttpApiDefinitionRevisionIdentityRecord> for DeploymentPlanHttpApiDefintionEntry {
    type Error = RepoError;
    fn try_from(value: HttpApiDefinitionRevisionIdentityRecord) -> Result<Self, Self::Error> {
        Ok(Self {
            id: HttpApiDefinitionId(value.http_api_definition_id),
            revision: value.revision_id.try_into()?,
            name: HttpApiDefinitionName(value.name),
            hash: value.hash.into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::HttpApiDefinitionDefinitionBlob;
    use desert_rust::BinaryCodec;
    use goldenfile::Mint;
    use golem_common::model::component::ComponentName;
    use golem_common::model::http_api_definition::{
        GatewayBinding, HttpApiRoute, RouteMethod, WorkerGatewayBinding,
    };
    use golem_service_base::repo::blob::Blob;
    use std::fmt::Debug;
    use std::io::Write;
    use test_r::test;

    #[allow(clippy::type_complexity)]
    fn assert_old_decodes_as<T: BinaryCodec + PartialEq + Debug + 'static>(
        expected: T,
    ) -> Box<dyn Fn(&std::path::Path, &std::path::Path)> {
        Box::new(move |old, _new| {
            let old_bytes = std::fs::read(old).unwrap();

            let old_decoded: T =
                desert_rust::deserialize(&old_bytes).expect("Failed to decode old version");

            assert_eq!(
                old_decoded, expected,
                "Decoded value from old file does not match expected"
            );
        })
    }

    #[test]
    fn blob_version_1_serialization() -> anyhow::Result<()> {
        let blob = Blob::new(HttpApiDefinitionDefinitionBlob {
            routes: vec![HttpApiRoute {
                method: RouteMethod::Post,
                path: "/{user-id}/test-path-1".to_string(),
                binding: GatewayBinding::Worker(WorkerGatewayBinding {
                    component_name: ComponentName("test-component".to_string()),
                    idempotency_key: None,
                    invocation_context: None,
                    response: r#"
                                let user-id = request.path.user-id;
                                let worker = "shopping-cart-${user-id}";
                                let inst = instance(worker);
                                let res = inst.cart(user-id);
                                let contents = res.get-cart-contents();
                                {
                                    headers: {ContentType: "json", userid: "foo"},
                                    body: contents,
                                    status: 201
                                }
                            "#
                    .to_string(),
                }),
                security: None,
            }],
        });

        let serialized = blob.serialize()?.clone();

        let mut mint = Mint::new("tests/goldenfiles");

        let mut file = mint.new_goldenfile_with_differ(
            "http_api_definition_repo_blob_v1.bin",
            assert_old_decodes_as(blob.into_value()),
        )?;

        file.write_all(&serialized)?;

        Ok(())
    }
}
