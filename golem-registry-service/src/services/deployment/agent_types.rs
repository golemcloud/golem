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

use crate::model::api_definition::{
    CompiledRouteWithSecuritySchemeDetails, CompiledRoutesForHttpApiDefinition,
    MaybeDisabledCompiledRoute,
};
use crate::repo::deployment::DeploymentRepo;
use crate::repo::model::deployment::DeployRepoError;
use crate::services::http_api_definition::{HttpApiDefinitionError, HttpApiDefinitionService};
use anyhow::anyhow;
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::http_api_definition::HttpApiDefinitionName;
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::custom_api::openapi::HttpApiDefinitionOpenApiSpec;
use golem_service_base::custom_api::{CompiledRoute, CompiledRoutes};
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::repo::RepoError;
use std::collections::HashMap;
use std::sync::Arc;
use golem_common::model::agent::RegisteredAgentType;

#[derive(Debug, thiserror::Error)]
pub enum DeployedAgentTypesError {
    #[error("No active routes for domain {0} found")]
    NoActiveRoutesForDomain(Domain),
    #[error("Http api definition for name {0} not found")]
    HttpApiDefinitionNotFound(HttpApiDefinitionName),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for DeployedAgentTypesError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::NoActiveRoutesForDomain(_) => self.to_string(),
            Self::HttpApiDefinitionNotFound(_) => self.to_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

error_forwarding!(
    DeployedAgentTypesError,
    RepoError,
    DeployRepoError,
    HttpApiDefinitionError
);

pub struct DeployedAgentTypesService {
    deployment_repo: Arc<dyn DeploymentRepo>,
}

impl DeployedAgentTypesService {
    pub fn new(
        deployment_repo: Arc<dyn DeploymentRepo>,
    ) -> Self {
        Self {
            deployment_repo,
        }
    }

    pub async fn get_deployed_agent_type(
        &self,
        environment_id: &EnvironmentId,
        agent_type_name: String
    ) -> Result<Option<RegisteredAgentType>, DeployedAgentTypesError> {
        todo!()
    }

    pub async fn list_deployed_agent_type(
        &self,
        environment_id: &EnvironmentId,
    ) -> Result<Vec<RegisteredAgentType>, DeployedAgentTypesError> {
        let agent_types = self
            .deployment_repo
            .list_deployed_agent_types(&environment_id.0)
            .await?
            .into_iter()
            .map(|r| r.into())
            .collect();

        Ok(agent_types)
    }

    pub async fn list_deployment_agent_types(
        &self,
        environment_id: &EnvironmentId,
        deployment_revision: DeploymentRevision,
        agent_type_name: String
    ) -> Result<Vec<RegisteredAgentType>, DeployedAgentTypesError> {
        todo!()
    }
}
