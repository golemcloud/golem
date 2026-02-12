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

use crate::model::api_definition::{BoundCompiledRoute, UnboundRouteSecurity};
use crate::repo::deployment::DeploymentRepo;
use crate::repo::model::deployment::DeployRepoError;
use crate::services::http_api_deployment::HttpApiDeploymentError;
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::EnvironmentId;
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::custom_api::{
    CompiledRoute, CompiledRoutes, RouteSecurity, SecuritySchemeRouteSecurity,
};
use golem_service_base::repo::RepoError;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum DeployedRoutesError {
    #[error("No active routes for domain {0} found")]
    NoActiveRoutesForDomain(Domain),
    #[error("Environment {0} not found")]
    ParentEnvironmentNotFound(EnvironmentId),
    #[error("Deployment revision {0} does not exist")]
    DeploymentRevisionNotFound(DeploymentRevision),
    #[error("Api deployment for domain {0} not found in deployment")]
    DomainNotFoundInDeployment(Domain),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for DeployedRoutesError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::NoActiveRoutesForDomain(_) => self.to_string(),
            Self::ParentEnvironmentNotFound(_) => self.to_string(),
            Self::DeploymentRevisionNotFound(_) => self.to_string(),
            Self::DomainNotFoundInDeployment(_) => self.to_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

error_forwarding!(
    DeployedRoutesError,
    RepoError,
    DeployRepoError,
    HttpApiDeploymentError
);

pub struct DeployedRoutesService {
    deployment_repo: Arc<dyn DeploymentRepo>,
    // http_api_deployment_service: Arc<HttpApiDeploymentService>,
}

impl DeployedRoutesService {
    pub fn new(
        deployment_repo: Arc<dyn DeploymentRepo>,
        // http_api_deployment_service: Arc<HttpApiDeploymentService>,
    ) -> Self {
        Self {
            deployment_repo,
            // http_api_deployment_service,
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

    // pub async fn get_compiled_routes_for_domain(
    //     &self,
    //     environment_id: EnvironmentId,
    //     deployment_revision: DeploymentRevision,
    //     domain: &Domain,
    //     auth: &AuthCtx,
    // ) -> Result<CompiledRoutesForDomain, DeployedRoutesError> {
    //     let _http_api_deployment = self
    //         .http_api_deployment_service
    //         .get_in_deployment_by_domain(environment_id, deployment_revision, domain, auth)
    //         .await
    //         .map_err(|err| match err {
    //             HttpApiDeploymentError::ParentEnvironmentNotFound(environment_id) => {
    //                 DeployedRoutesError::ParentEnvironmentNotFound(environment_id)
    //             }
    //             HttpApiDeploymentError::DeploymentRevisionNotFound(deployment_revision) => {
    //                 DeployedRoutesError::DeploymentRevisionNotFound(deployment_revision)
    //             }
    //             HttpApiDeploymentError::HttpApiDeploymentByDomainNotFound(name) => {
    //                 DeployedRoutesError::DomainNotFoundInDeployment(name)
    //             }
    //             other => other.into(),
    //         })?;

    //     let routes: Vec<BoundCompiledRoute> = self
    //         .deployment_repo
    //         .list_compiled_routes_for_domain_and_deployment(
    //             environment_id.0,
    //             deployment_revision.into(),
    //             &domain.0,
    //         )
    //         .await?
    //         .into_iter()
    //         .map(|r| r.try_into())
    //         .collect::<Result<_, _>>()?;

    //     let mut security_schemes = HashMap::new();
    //     let mut converted_routes = Vec::with_capacity(routes.len());

    //     for route in routes {
    //         let mut security_scheme_id = None;
    //         if let Some(security_scheme) = route.security_scheme {
    //             let _ = security_scheme_id.insert(security_scheme.id);
    //             security_schemes.insert(security_scheme.id, security_scheme);
    //         }
    //         let converted = MaybeDisabledCompiledRoute {
    //             method: route.route.method,
    //             path: route.route.path,
    //             behavior: route.route.behaviour,
    //             body: route.route.body,
    //             security_scheme_missing: route.security_scheme_missing,
    //             security_scheme: security_scheme_id,
    //             cors: route.route.cors,
    //         };
    //         converted_routes.push(converted);
    //     }

    //     Ok(CompiledRoutesForDomain {
    //         security_schemes,
    //         routes: converted_routes,
    //     })
    // }

    pub async fn get_currently_active_compiled_routes(
        &self,
        domain: &Domain,
    ) -> Result<CompiledRoutes, DeployedRoutesError> {
        let routes: Vec<BoundCompiledRoute> = self
            .deployment_repo
            .list_active_compiled_routes_for_domain(&domain.0)
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

            let security = match route.route.security {
                UnboundRouteSecurity::None => RouteSecurity::None,
                UnboundRouteSecurity::SessionFromHeader(inner) => {
                    RouteSecurity::SessionFromHeader(inner)
                }
                UnboundRouteSecurity::SecurityScheme(_) => {
                    // Safe as the repo layer guarantees that security_scheme_missing would be set
                    // if the security scheme for this name could not be found.
                    let security_scheme_id = security_scheme_id.unwrap();
                    RouteSecurity::SecurityScheme(SecuritySchemeRouteSecurity {
                        security_scheme_id,
                    })
                }
            };

            let converted = CompiledRoute {
                route_id: route.route.route_id,
                method: route.route.method,
                path: route.route.path,
                body: route.route.body,
                behavior: route.route.behaviour,
                security,
                cors: route.route.cors,
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
