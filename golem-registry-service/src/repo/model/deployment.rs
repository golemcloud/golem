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

use crate::model::api_definition::{BoundCompiledRoute, UnboundCompiledRoute};
use crate::model::component::Component;
use crate::repo::model::audit::RevisionAuditFields;
use crate::repo::model::component::ComponentRevisionIdentityRecord;
use crate::repo::model::hash::SqlBlake3Hash;
use crate::repo::model::http_api_deployment::HttpApiDeploymentRevisionIdentityRecord;
use anyhow::anyhow;
use desert_rust::BinaryCodec;
use golem_common::base_model::agent::AgentTypeName;
use golem_common::base_model::domain_registration::Domain;
use golem_common::error_forwarding;
use golem_common::model::account::AccountId;
use golem_common::model::agent::DeployedRegisteredAgentType;
use golem_common::model::agent::{AgentType, RegisteredAgentTypeImplementer};
use golem_common::model::deployment::{
    CurrentDeployment, CurrentDeploymentRevision, Deployment, DeploymentPlan, DeploymentRevision,
    DeploymentSummary, DeploymentVersion,
};
use golem_common::model::diff::{self, Hash, Hashable};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::http_api_deployment::HttpApiDeployment;
use golem_common::model::security_scheme::{Provider, SecuritySchemeId, SecuritySchemeName};
use golem_service_base::custom_api::SecuritySchemeDetails;
use golem_service_base::mcp::CompiledMcp;
use golem_service_base::repo::RepoError;
use golem_service_base::repo::blob::Blob;
use sqlx::FromRow;
use std::str::FromStr;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum DeployRepoError {
    #[error("Concurrent modification")]
    ConcurrentModification,
    #[error("Version already exists: {version}")]
    VersionAlreadyExists { version: String },
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

error_forwarding!(DeployRepoError, RepoError);

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct CurrentDeploymentRevisionRecord {
    pub environment_id: Uuid,
    pub revision_id: i64,
    #[sqlx(flatten)]
    pub audit: RevisionAuditFields,
    pub deployment_revision_id: i64,
}

impl CurrentDeploymentRevisionRecord {
    pub fn into_model(
        self,
        version: DeploymentVersion,
        deployment_hash: Hash,
    ) -> Result<CurrentDeployment, DeployRepoError> {
        Ok(CurrentDeployment {
            environment_id: EnvironmentId(self.environment_id),
            revision: self.deployment_revision_id.try_into()?,
            version,
            deployment_hash,
            current_revision: self.revision_id.try_into()?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct CurrentDeploymentExtRevisionRecord {
    #[sqlx(flatten)]
    pub revision: CurrentDeploymentRevisionRecord,

    pub deployment_version: String,
    pub deployment_hash: SqlBlake3Hash,
}

impl TryFrom<CurrentDeploymentExtRevisionRecord> for CurrentDeployment {
    type Error = DeployRepoError;
    fn try_from(value: CurrentDeploymentExtRevisionRecord) -> Result<Self, Self::Error> {
        value.revision.into_model(
            DeploymentVersion(value.deployment_version),
            Hash::new(value.deployment_hash.into_blake3_hash()),
        )
    }
}

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct CurrentDeploymentRecord {
    pub environment_id: Uuid,
    pub current_revision_id: i64,
}

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct DeploymentRevisionRecord {
    pub environment_id: Uuid,
    pub revision_id: i64,
    pub version: String,
    pub hash: SqlBlake3Hash,
    #[sqlx(flatten)]
    pub audit: RevisionAuditFields,
}

impl TryFrom<DeploymentRevisionRecord> for Deployment {
    type Error = DeployRepoError;
    fn try_from(value: DeploymentRevisionRecord) -> Result<Self, Self::Error> {
        Ok(Self {
            environment_id: EnvironmentId(value.environment_id),
            revision: value.revision_id.try_into()?,
            version: DeploymentVersion(value.version),
            deployment_hash: Hash::new(value.hash.into_blake3_hash()),
        })
    }
}

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct DeploymentComponentRevisionRecord {
    pub environment_id: Uuid,
    pub deployment_revision_id: i64,
    pub component_id: Uuid,
    pub component_revision_id: i64,
}

impl DeploymentComponentRevisionRecord {
    pub fn from_model(
        environment_id: EnvironmentId,
        deployment_revision: DeploymentRevision,
        component: Component,
    ) -> Self {
        Self {
            environment_id: environment_id.0,
            deployment_revision_id: deployment_revision.into(),
            component_id: component.id.0,
            component_revision_id: component.revision.into(),
        }
    }
}

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct DeploymentHttpApiDeploymentRevisionRecord {
    pub environment_id: Uuid,
    pub deployment_revision_id: i64,
    pub http_api_deployment_id: Uuid,
    pub http_api_deployment_revision_id: i64,
}

impl DeploymentHttpApiDeploymentRevisionRecord {
    pub fn from_model(
        environment_id: EnvironmentId,
        deployment_revision: DeploymentRevision,
        http_api_deployment: HttpApiDeployment,
    ) -> Self {
        Self {
            environment_id: environment_id.0,
            deployment_revision_id: deployment_revision.into(),
            http_api_deployment_id: http_api_deployment.id.0,
            http_api_deployment_revision_id: http_api_deployment.revision.into(),
        }
    }
}

pub struct DeploymentHashes {
    pub env_hash: SqlBlake3Hash,
    pub deployment_hash: SqlBlake3Hash,
}

pub struct DeploymentIdentity {
    pub components: Vec<ComponentRevisionIdentityRecord>,
    pub http_api_deployments: Vec<HttpApiDeploymentRevisionIdentityRecord>,
}

impl DeploymentIdentity {
    pub fn into_plan(
        self,
        current_revision: Option<CurrentDeploymentRevision>,
    ) -> Result<DeploymentPlan, DeployRepoError> {
        Ok(DeploymentPlan {
            current_revision,
            deployment_hash: self.to_diffable().hash(),
            components: self
                .components
                .into_iter()
                .map(|c| c.try_into())
                .collect::<Result<Vec<_>, _>>()?,
            http_api_deployments: self
                .http_api_deployments
                .into_iter()
                .map(|had| had.try_into())
                .collect::<Result<Vec<_>, _>>()?,
        })
    }
}

impl DeploymentIdentity {
    pub fn to_diffable(&self) -> diff::Deployment {
        diff::Deployment {
            components: self
                .components
                .iter()
                .map(|component| {
                    (
                        component.name.clone(),
                        diff::HashOf::from_blake3_hash(component.hash.into()),
                    )
                })
                .collect(),
            http_api_deployments: self
                .http_api_deployments
                .iter()
                .map(|deployment| {
                    (
                        (&deployment.domain).into(),
                        diff::HashOf::from_blake3_hash(deployment.hash.into()),
                    )
                })
                .collect(),
        }
    }
}

pub struct DeployedDeploymentIdentity {
    pub deployment_revision: DeploymentRevisionRecord,
    pub identity: DeploymentIdentity,
}

impl TryFrom<DeployedDeploymentIdentity> for DeploymentSummary {
    type Error = DeployRepoError;
    fn try_from(value: DeployedDeploymentIdentity) -> Result<Self, Self::Error> {
        Ok(Self {
            deployment_revision: value.deployment_revision.revision_id.try_into()?,
            deployment_hash: value.deployment_revision.hash.into(),
            components: value
                .identity
                .components
                .into_iter()
                .map(|c| c.try_into())
                .collect::<Result<Vec<_>, _>>()?,
            http_api_deployments: value
                .identity
                .http_api_deployments
                .into_iter()
                .map(|had| had.try_into())
                .collect::<Result<Vec<_>, _>>()?,
        })
    }
}

#[derive(FromRow)]
pub struct DeploymentCompiledRouteRecord {
    pub environment_id: Uuid,
    pub deployment_revision_id: i64,
    pub domain: String,
    pub route_id: i32,

    pub security_scheme: Option<String>,
    // Potential performance optimization:
    // We could extract security scheme and domain here and reconstruct during reads.
    pub compiled_route: Blob<UnboundCompiledRoute>,
}

impl DeploymentCompiledRouteRecord {
    pub fn from_model(
        environment_id: EnvironmentId,
        deployment_revision: DeploymentRevision,
        compiled_route: UnboundCompiledRoute,
    ) -> Self {
        Self {
            environment_id: environment_id.0,
            deployment_revision_id: deployment_revision.into(),
            domain: compiled_route.domain.0.clone(),
            route_id: compiled_route.route_id,
            security_scheme: compiled_route.security_scheme().map(|sn| sn.0),
            compiled_route: Blob::new(compiled_route),
        }
    }
}

#[derive(Debug, Clone, PartialEq, FromRow)]
pub struct DeploymentRegisteredAgentTypeRecord {
    pub environment_id: Uuid,
    pub deployment_revision_id: i64,
    pub agent_type_name: String,
    pub agent_wrapper_type_name: String,

    pub component_id: Uuid,
    pub component_revision_id: i64,
    pub webhook_prefix_authority_and_path: Option<String>,
    pub agent_type: Blob<AgentType>,
}

impl DeploymentRegisteredAgentTypeRecord {
    pub fn from_model(
        environment_id: EnvironmentId,
        deployment_revision: DeploymentRevision,
        registered_agent_type: DeployedRegisteredAgentType,
    ) -> Self {
        Self {
            environment_id: environment_id.0,
            deployment_revision_id: deployment_revision.into(),
            agent_type_name: registered_agent_type.agent_type.type_name.to_string(),
            agent_wrapper_type_name: registered_agent_type.agent_type.wrapper_type_name(),
            component_id: registered_agent_type.implemented_by.component_id.0,
            component_revision_id: registered_agent_type
                .implemented_by
                .component_revision
                .into(),
            webhook_prefix_authority_and_path: registered_agent_type
                .webhook_prefix_authority_and_path,
            agent_type: Blob::new(registered_agent_type.agent_type),
        }
    }
}

impl TryFrom<DeploymentRegisteredAgentTypeRecord> for DeployedRegisteredAgentType {
    type Error = DeployRepoError;
    fn try_from(value: DeploymentRegisteredAgentTypeRecord) -> Result<Self, Self::Error> {
        Ok(Self {
            agent_type: value.agent_type.into_value(),
            implemented_by: RegisteredAgentTypeImplementer {
                component_id: value.component_id.into(),
                component_revision: value.component_revision_id.try_into()?,
            },
            webhook_prefix_authority_and_path: value.webhook_prefix_authority_and_path,
        })
    }
}

pub struct DeploymentRevisionCreationRecord {
    pub environment_id: Uuid,
    pub deployment_revision_id: i64,

    pub version: String,
    pub hash: SqlBlake3Hash,

    pub components: Vec<DeploymentComponentRevisionRecord>,
    pub http_api_deployments: Vec<DeploymentHttpApiDeploymentRevisionRecord>,
    pub compiled_routes: Vec<DeploymentCompiledRouteRecord>,
    pub compiled_mcp: DeploymentMcpCapabilityRecord,
    pub registered_agent_types: Vec<DeploymentRegisteredAgentTypeRecord>,
}

impl DeploymentRevisionCreationRecord {
    pub fn from_model(
        environment_id: EnvironmentId,
        deployment_revision: DeploymentRevision,
        version: DeploymentVersion,
        hash: diff::Hash,
        components: Vec<Component>,
        http_api_deployments: Vec<HttpApiDeployment>,
        compiled_routes: Vec<UnboundCompiledRoute>,
        compiled_mcp: CompiledMcp,
        registered_agent_types: Vec<DeployedRegisteredAgentType>,
    ) -> Self {
        Self {
            environment_id: environment_id.0,
            deployment_revision_id: deployment_revision.into(),
            version: version.0,
            hash: hash.into(),
            components: components
                .into_iter()
                .map(|c| {
                    DeploymentComponentRevisionRecord::from_model(
                        environment_id,
                        deployment_revision,
                        c,
                    )
                })
                .collect(),
            http_api_deployments: http_api_deployments
                .into_iter()
                .map(|had| {
                    DeploymentHttpApiDeploymentRevisionRecord::from_model(
                        environment_id,
                        deployment_revision,
                        had,
                    )
                })
                .collect(),
            compiled_routes: compiled_routes
                .into_iter()
                .map(|r| {
                    DeploymentCompiledRouteRecord::from_model(
                        environment_id,
                        deployment_revision,
                        r,
                    )
                })
                .collect(),
            compiled_mcp: DeploymentMcpCapabilityRecord::from_model(compiled_mcp),
            registered_agent_types: registered_agent_types
                .into_iter()
                .map(|r| {
                    DeploymentRegisteredAgentTypeRecord::from_model(
                        environment_id,
                        deployment_revision,
                        r,
                    )
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, BinaryCodec)]
pub struct CompiledMcpData {
    pub implementers: golem_service_base::mcp::AgentTypeImplementers,
}

#[derive(FromRow)]
pub struct DeploymentMcpCapabilityRecord {
    pub account_id: Uuid,
    pub environment_id: Uuid,
    pub deployment_revision_id: i64,
    pub domain: String,
    pub mcp_data: Blob<CompiledMcpData>,
}

impl DeploymentMcpCapabilityRecord {
    pub fn from_model(compiled_mcp: CompiledMcp) -> Self {
        Self {
            account_id: compiled_mcp.account_id.0,
            environment_id: compiled_mcp.environment_id.0,
            deployment_revision_id: compiled_mcp.deployment_revision.into(),
            domain: compiled_mcp.domain.0.clone(),
            mcp_data: Blob::new(CompiledMcpData {
                implementers: compiled_mcp.agent_type_implementers,
            }),
        }
    }
}

impl TryFrom<DeploymentMcpCapabilityRecord> for CompiledMcp {
    type Error = DeployRepoError;

    fn try_from(value: DeploymentMcpCapabilityRecord) -> Result<Self, Self::Error> {
        let mcp_data = value.mcp_data.into_value();

        Ok(Self {
            account_id: AccountId(value.account_id),
            environment_id: EnvironmentId(value.environment_id),
            deployment_revision: value.deployment_revision_id.try_into()?,
            domain: Domain(value.domain),
            agent_type_implementers: mcp_data.implementers,
        })
    }
}

#[derive(FromRow)]
pub struct DeploymentCompiledRouteWithSecuritySchemeRecord {
    pub account_id: Uuid,
    pub environment_id: Uuid,
    pub deployment_revision_id: i64,
    pub domain: String,
    pub route_id: i32,

    pub security_scheme_missing: bool,

    pub security_scheme_id: Option<Uuid>,
    pub security_scheme_name: Option<String>,
    pub security_scheme_provider_type: Option<String>,
    pub security_scheme_client_id: Option<String>,
    pub security_scheme_client_secret: Option<String>,
    pub security_scheme_redirect_url: Option<String>,
    pub security_scheme_scopes: Option<String>,

    pub compiled_route: Blob<UnboundCompiledRoute>,
}

impl TryFrom<DeploymentCompiledRouteWithSecuritySchemeRecord> for BoundCompiledRoute {
    type Error = DeployRepoError;

    fn try_from(
        value: DeploymentCompiledRouteWithSecuritySchemeRecord,
    ) -> Result<Self, Self::Error> {
        use openidconnect::{ClientId, ClientSecret, RedirectUrl, Scope};

        let security_scheme = match (
            value.security_scheme_id,
            value.security_scheme_name,
            value.security_scheme_provider_type,
            value.security_scheme_client_id,
            value.security_scheme_client_secret,
            value.security_scheme_redirect_url,
            value.security_scheme_scopes,
        ) {
            (
                Some(security_scheme_id),
                Some(security_scheme_name),
                Some(provider_type),
                Some(client_id),
                Some(client_secret),
                Some(redirect_url),
                Some(scopes),
            ) => {
                let id = SecuritySchemeId(security_scheme_id);
                let name = SecuritySchemeName(security_scheme_name);
                let scopes: Vec<Scope> = serde_json::from_str(&scopes)
                    .map_err(|e| anyhow::Error::from(e).context("Failed parsing scopes"))?;
                let redirect_url: RedirectUrl = serde_json::from_str(&redirect_url)
                    .map_err(|e| anyhow::Error::from(e).context("Failed parsing redirect_url"))?;
                let provider_type = Provider::from_str(&provider_type)
                    .map_err(|e| anyhow!("Failed parsing provider type: {e}"))?;
                let client_id = ClientId::new(client_id);
                let client_secret = ClientSecret::new(client_secret);

                Some(SecuritySchemeDetails {
                    id,
                    name,
                    scopes,
                    redirect_url,
                    provider_type,
                    client_id,
                    client_secret,
                })
            }
            _ => None,
        };

        Ok(Self {
            account_id: AccountId(value.account_id),
            environment_id: EnvironmentId(value.environment_id),
            deployment_revision: value.deployment_revision_id.try_into()?,
            security_scheme_missing: value.security_scheme_missing,
            security_scheme,
            route: value.compiled_route.into_value(),
        })
    }
}
