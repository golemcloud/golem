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
// use golem_service_base::custom_api::openapi::HttpApiDefinitionOpenApiSpec;
use golem_service_base::custom_api::{CompiledRoute, CompiledRoutes};
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::repo::RepoError;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum DeployedRoutesError {
    #[error("No active routes for domain {0} found")]
    NoActiveRoutesForDomain(Domain),
    #[error("Http api definition for name {0} not found")]
    HttpApiDefinitionNotFound(HttpApiDefinitionName),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for DeployedRoutesError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::NoActiveRoutesForDomain(_) => self.to_string(),
            Self::HttpApiDefinitionNotFound(_) => self.to_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

error_forwarding!(
    DeployedRoutesError,
    RepoError,
    DeployRepoError,
    HttpApiDefinitionError
);

pub struct DeployedRoutesService {
    deployment_repo: Arc<dyn DeploymentRepo>,
    http_api_definition_service: Arc<HttpApiDefinitionService>,
}

impl DeployedRoutesService {
    pub fn new(
        deployment_repo: Arc<dyn DeploymentRepo>,
        http_api_definition_service: Arc<HttpApiDefinitionService>,
    ) -> Self {
        Self {
            deployment_repo,
            http_api_definition_service,
        }
    }

    // pub async fn get_openapi_spec_for_http_api_definition(
    //     &self,
    //     environment_id: EnvironmentId,
    //     deployment_revision: DeploymentRevision,
    //     http_api_definition_name: &HttpApiDefinitionName,
    //     auth: &AuthCtx,
    // ) -> Result<HttpApiDefinitionOpenApiSpec, DeployedRoutesError> {
    //     let compiled_routes = self
    //         .get_compiled_routes_for_http_api_definition(
    //             environment_id,
    //             deployment_revision,
    //             http_api_definition_name,
    //             auth,
    //         )
    //         .await?;
    //     let openapi_spec = HttpApiDefinitionOpenApiSpec::from_routes(
    //         &compiled_routes.http_api_definition_name,
    //         &compiled_routes.http_api_definition_version,
    //         &compiled_routes.routes,
    //         &compiled_routes.security_schemes,
    //     )
    //     .await
    //     .map_err(|e| anyhow!("Failed building openapi spec: {e}"))?;

    //     Ok(openapi_spec)
    // }

    pub async fn get_compiled_routes_for_http_api_definition(
        &self,
        environment_id: EnvironmentId,
        deployment_revision: DeploymentRevision,
        http_api_definition_name: &HttpApiDefinitionName,
        auth: &AuthCtx,
    ) -> Result<CompiledRoutesForHttpApiDefinition, DeployedRoutesError> {
        let http_api_definition = self
            .http_api_definition_service
            .get_in_deployment_by_name(
                environment_id,
                deployment_revision,
                http_api_definition_name,
                auth,
            )
            .await
            .map_err(|err| match err {
                HttpApiDefinitionError::HttpApiDefinitionByNameNotFound(name) => {
                    DeployedRoutesError::HttpApiDefinitionNotFound(name)
                }
                other => other.into(),
            })?;

        let routes: Vec<CompiledRouteWithSecuritySchemeDetails> = self
            .deployment_repo
            .list_compiled_http_api_routes_for_http_api_definition(
                environment_id.0,
                deployment_revision.into(),
                &http_api_definition_name.0,
            )
            .await?
            .into_iter()
            .map(|r| r.try_into())
            .collect::<Result<_, _>>()?;

        let mut security_schemes = HashMap::new();
        let mut converted_routes = Vec::with_capacity(routes.len());

        for route in routes {
            let mut security_scheme_id = None;
            if let Some(security_scheme) = route.security_scheme {
                let _ = security_scheme_id.insert(security_scheme.id);
                security_schemes.insert(security_scheme.id, security_scheme);
            }
            let converted = MaybeDisabledCompiledRoute {
                security_scheme_missing: route.security_scheme_missing,
                security_scheme: security_scheme_id,
                method: route.route.method,
                path: route.route.path,
                binding: route.route.binding,
            };
            converted_routes.push(converted);
        }

        Ok(CompiledRoutesForHttpApiDefinition {
            http_api_definition_id: http_api_definition.id,
            http_api_definition_name: http_api_definition.name,
            http_api_definition_version: http_api_definition.version,
            routes: converted_routes,
            security_schemes,
        })
    }

    pub async fn get_currently_active_compiled_routes(
        &self,
        domain: &Domain,
    ) -> Result<CompiledRoutes, DeployedRoutesError> {
        let routes: Vec<CompiledRouteWithSecuritySchemeDetails> = self
            .deployment_repo
            .list_active_compiled_http_api_routes_for_domain(&domain.0)
            .await?
            .into_iter()
            .map(|r| r.try_into())
            .collect::<Result<_, _>>()?;

        let mut account_id = None;
        let mut environment_id = None;
        let mut deployment_revision = None;
        let mut security_schemes = HashMap::new();
        let mut converted_routes = Vec::with_capacity(routes.len());

        for route in routes {
            // we only care about active routes here
            if route.security_scheme_missing {
                continue;
            };

            let _ = account_id.insert(route.account_id);
            let _ = environment_id.insert(route.environment_id);
            let _ = deployment_revision.insert(route.deployment_revision);

            let mut security_scheme_id = None;
            if let Some(security_scheme) = route.security_scheme {
                let _ = security_scheme_id.insert(security_scheme.id);
                security_schemes.insert(security_scheme.id, security_scheme);
            }
            let converted = CompiledRoute {
                http_api_definition_id: route.http_api_definition_id,
                method: route.route.method,
                path: route.route.path,
                binding: route.route.binding,
                security_scheme: security_scheme_id,
            };
            converted_routes.push(converted);
        }

        let account_id =
            account_id.ok_or(DeployedRoutesError::NoActiveRoutesForDomain(domain.clone()))?;

        let environment_id =
            environment_id.ok_or(DeployedRoutesError::NoActiveRoutesForDomain(domain.clone()))?;

        let deployment_revision = deployment_revision
            .ok_or(DeployedRoutesError::NoActiveRoutesForDomain(domain.clone()))?;

        Ok(CompiledRoutes {
            account_id,
            environment_id,
            deployment_revision,
            routes: converted_routes,
            security_schemes,
        })
    }
}
