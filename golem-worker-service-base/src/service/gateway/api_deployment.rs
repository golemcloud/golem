// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::gateway_api_definition::ApiDefinitionId;
use crate::gateway_api_deployment::*;

use std::collections::{HashMap, HashSet};

use async_trait::async_trait;

use std::sync::Arc;
use tracing::{error, info};

use crate::gateway_api_definition::http::{
    AllPathPatterns, CompiledHttpApiDefinition, HttpApiDefinition, Route,
};

use crate::gateway_binding::GatewayBindingCompiled;
use crate::gateway_execution::router::{Router, RouterPattern};
use crate::repo::api_definition::ApiDefinitionRepo;
use crate::repo::api_deployment::ApiDeploymentRecord;
use crate::repo::api_deployment::ApiDeploymentRepo;
use crate::service::component::ComponentService;
use crate::service::gateway::api_definition::ApiDefinitionIdWithVersion;
use chrono::Utc;
use golem_common::model::component_constraint::FunctionConstraintCollection;
use golem_common::model::ComponentId;
use golem_common::SafeDisplay;
use golem_service_base::repo::RepoError;
use rib::WorkerFunctionsInRib;
use std::fmt::{Debug, Display};

#[async_trait]
pub trait ApiDeploymentService<AuthCtx, Namespace> {
    async fn deploy(
        &self,
        deployment: &ApiDeploymentRequest<Namespace>,
        auth_ctx: &AuthCtx,
    ) -> Result<(), ApiDeploymentError<Namespace>>;

    async fn undeploy(
        &self,
        deployment: &ApiDeploymentRequest<Namespace>,
    ) -> Result<(), ApiDeploymentError<Namespace>>;

    // Example: A newer version of API definition is in dev site, and older version of the same definition-id is in prod site.
    // Therefore, Vec<ApiDeployment>
    async fn get_by_id(
        &self,
        namespace: &Namespace,
        api_definition_id: &ApiDefinitionId,
    ) -> Result<Vec<ApiDeployment<Namespace>>, ApiDeploymentError<Namespace>>;

    async fn get_by_site(
        &self,
        site: &ApiSiteString,
    ) -> Result<Option<ApiDeployment<Namespace>>, ApiDeploymentError<Namespace>>;

    async fn get_definitions_by_site(
        &self,
        site: &ApiSiteString,
    ) -> Result<Vec<CompiledHttpApiDefinition<Namespace>>, ApiDeploymentError<Namespace>>;

    async fn delete(
        &self,
        namespace: &Namespace,
        site: &ApiSiteString,
    ) -> Result<(), ApiDeploymentError<Namespace>>;
}

#[derive(Debug, thiserror::Error)]
pub enum ApiDeploymentError<Namespace> {
    #[error("API definition not found: {1}")]
    ApiDefinitionNotFound(Namespace, ApiDefinitionId),
    #[error("API deployment not found: {1}")]
    ApiDeploymentNotFound(Namespace, ApiSiteString),
    #[error("API deployment conflict error: {0}")]
    ApiDeploymentConflict(ApiSiteString),
    #[error("API deployment definitions conflict error: {0}")]
    ApiDefinitionsConflict(String),
    #[error("Internal repository error: {0}")]
    InternalRepoError(RepoError),
    #[error("Internal error: failed to convert {what}: {error}")]
    InternalConversionError { what: String, error: String },
    #[error("Internal error: failed to create component constraints {0}")]
    ComponentConstraintCreateError(String),
}

impl<T> ApiDeploymentError<T> {
    pub fn conversion_error(what: impl AsRef<str>, error: String) -> Self {
        Self::InternalConversionError {
            what: what.as_ref().to_string(),
            error,
        }
    }
}

impl<Namespace> From<RepoError> for ApiDeploymentError<Namespace> {
    fn from(error: RepoError) -> Self {
        ApiDeploymentError::InternalRepoError(error)
    }
}

impl<Namespace: Display> SafeDisplay for ApiDeploymentError<Namespace> {
    fn to_safe_string(&self) -> String {
        match self {
            ApiDeploymentError::ApiDefinitionNotFound(_, _) => self.to_string(),
            ApiDeploymentError::ApiDeploymentNotFound(_, _) => self.to_string(),
            ApiDeploymentError::ApiDeploymentConflict(_) => self.to_string(),
            ApiDeploymentError::ApiDefinitionsConflict(_) => self.to_string(),
            ApiDeploymentError::InternalRepoError(inner) => inner.to_safe_string(),
            ApiDeploymentError::InternalConversionError { .. } => self.to_string(),
            ApiDeploymentError::ComponentConstraintCreateError(_) => self.to_string(),
        }
    }
}

pub trait ConflictChecker {
    type Entity: Display + Send;
    fn find_conflicts(input: &[Self]) -> Vec<Self::Entity>
    where
        Self: Sized;
}

impl ConflictChecker for HttpApiDefinition {
    type Entity = AllPathPatterns;
    fn find_conflicts(definitions: &[Self]) -> Vec<Self::Entity> {
        let routes = definitions
            .iter()
            .flat_map(|def| def.routes.clone())
            .collect::<Vec<_>>();

        let mut router = Router::<Route>::new();

        let mut conflicting_path_patterns = vec![];

        for route in routes {
            let method: hyper::Method = route.clone().method.into();
            let path = route
                .clone()
                .path
                .path_patterns
                .iter()
                .map(|pattern| RouterPattern::from(pattern.clone()))
                .collect::<Vec<_>>();

            if !router.add_route(method.clone(), path.clone(), route) {
                let current_route = router.get_route(&method, &path).unwrap();

                conflicting_path_patterns.push(current_route.path.clone());
            }
        }

        conflicting_path_patterns
    }
}

pub struct ApiDeploymentServiceDefault<AuthCtx> {
    pub deployment_repo: Arc<dyn ApiDeploymentRepo + Sync + Send>,
    pub definition_repo: Arc<dyn ApiDefinitionRepo + Sync + Send>,
    pub component_service: Arc<dyn ComponentService<AuthCtx> + Send + Sync>,
}

impl<AuthCtx> ApiDeploymentServiceDefault<AuthCtx> {
    pub fn new(
        deployment_repo: Arc<dyn ApiDeploymentRepo + Sync + Send>,
        definition_repo: Arc<dyn ApiDefinitionRepo + Sync + Send>,
        component_service: Arc<dyn ComponentService<AuthCtx> + Send + Sync>,
    ) -> Self {
        Self {
            deployment_repo,
            definition_repo,
            component_service,
        }
    }

    async fn set_undeployed_as_draft<Namespace>(
        &self,
        deployments: Vec<ApiDeploymentRecord>,
    ) -> Result<(), ApiDeploymentError<Namespace>> {
        for deployment in deployments {
            let existing_deployments = self
                .deployment_repo
                .get_by_id_and_version(
                    deployment.namespace.as_str(),
                    deployment.definition_id.as_str(),
                    deployment.definition_version.as_str(),
                )
                .await?;

            if existing_deployments.is_empty() {
                self.definition_repo
                    .set_draft(
                        deployment.namespace.as_str(),
                        deployment.definition_id.as_str(),
                        deployment.definition_version.as_str(),
                        true,
                    )
                    .await?;
            }
        }

        Ok(())
    }

    fn get_worker_functions_in_api_definitions<Namespace>(
        definitions: Vec<CompiledHttpApiDefinition<Namespace>>,
    ) -> Result<HashMap<ComponentId, FunctionConstraintCollection>, ApiDeploymentError<Namespace>>
    {
        let mut worker_functions_in_rib = HashMap::new();

        for definition in definitions {
            for route in definition.routes {
                if let GatewayBindingCompiled::Worker(worker_binding) = route.binding {
                    let component_id = worker_binding.component_id;
                    let worker_calls = worker_binding.response_compiled.worker_calls;
                    if let Some(worker_calls) = worker_calls {
                        worker_functions_in_rib
                            .entry(component_id.component_id)
                            .or_insert_with(Vec::new)
                            .push(worker_calls)
                    }
                }
            }
        }

        Self::merge_worker_functions_in_rib(worker_functions_in_rib)
    }

    fn merge_worker_functions_in_rib<Namespace>(
        worker_functions: HashMap<ComponentId, Vec<WorkerFunctionsInRib>>,
    ) -> Result<HashMap<ComponentId, FunctionConstraintCollection>, ApiDeploymentError<Namespace>>
    {
        let mut merged_worker_functions: HashMap<ComponentId, FunctionConstraintCollection> =
            HashMap::new();

        for (component_id, worker_functions_in_rib) in worker_functions {
            let function_constraints = worker_functions_in_rib
                .iter()
                .map(FunctionConstraintCollection::from_worker_functions_in_rib)
                .collect::<Vec<_>>();

            let merged_calls = FunctionConstraintCollection::try_merge(function_constraints)
                .map_err(|err| ApiDeploymentError::ApiDefinitionsConflict(err))?;

            merged_worker_functions.insert(component_id, merged_calls);
        }

        Ok(merged_worker_functions)
    }
}

#[async_trait]
impl<AuthCtx, Namespace> ApiDeploymentService<AuthCtx, Namespace>
    for ApiDeploymentServiceDefault<AuthCtx>
where
    AuthCtx: Send + Sync,
    Namespace: Display + TryFrom<String> + Eq + Clone + Send + Sync,
    <Namespace as TryFrom<String>>::Error: Display + Debug + Send + Sync + 'static,
{
    async fn deploy(
        &self,
        deployment: &ApiDeploymentRequest<Namespace>,
        auth_ctx: &AuthCtx,
    ) -> Result<(), ApiDeploymentError<Namespace>> {
        info!(namespace = %deployment.namespace, "Deploy API definitions");

        let created_at = Utc::now();

        // Existing deployment
        let existing_deployment_records = self
            .deployment_repo
            .get_by_site(deployment.site.to_string().as_str())
            .await?;

        let mut existing_api_definition_keys: HashSet<ApiDefinitionIdWithVersion> = HashSet::new();

        for deployment_record in existing_deployment_records {
            if deployment_record.namespace != deployment.namespace.to_string()
                || deployment_record.subdomain != deployment.site.subdomain
                || deployment_record.host != deployment.site.host
            {
                info!(namespace = %deployment.namespace,
                    "Deploying API definition - failed, site used by another API (under another namespace/API)",
                );
                return Err(ApiDeploymentError::ApiDeploymentConflict(
                    ApiSiteString::from(&ApiSite {
                        host: deployment_record.host,
                        subdomain: deployment_record.subdomain,
                    }),
                ));
            }

            existing_api_definition_keys.insert(ApiDefinitionIdWithVersion {
                id: deployment_record.definition_id.into(),
                version: deployment_record.definition_version.into(),
            });
        }

        let mut new_deployment_records: Vec<ApiDeploymentRecord> = vec![];

        let mut set_not_draft: Vec<ApiDefinitionIdWithVersion> = vec![];

        let mut new_definitions: Vec<CompiledHttpApiDefinition<Namespace>> = vec![];

        for api_definition_key in deployment.api_definition_keys.clone() {
            // If definition is not present in existing deployment
            if !existing_api_definition_keys.contains(&api_definition_key) {
                let record = self
                    .definition_repo
                    .get(
                        deployment.namespace.to_string().as_str(),
                        api_definition_key.id.0.as_str(),
                        api_definition_key.version.0.as_str(),
                    )
                    .await?;

                match record {
                    None => {
                        return Err(ApiDeploymentError::ApiDefinitionNotFound(
                            deployment.namespace.clone(),
                            api_definition_key.id.clone(),
                        ));
                    }
                    Some(record) => {
                        if record.draft {
                            set_not_draft.push(api_definition_key.clone());
                        }
                        let definition = record.try_into().map_err(|e| {
                            ApiDeploymentError::conversion_error("API definition record", e)
                        })?;
                        new_definitions.push(definition);
                    }
                }

                new_deployment_records.push(ApiDeploymentRecord::new(
                    deployment.namespace.clone(),
                    deployment.site.clone(),
                    api_definition_key,
                    created_at,
                ));
            }
        }

        let existing_definitions = self
            .get_definitions_by_site(&(&deployment.site.clone()).into())
            .await?;

        new_definitions.extend(existing_definitions);

        let conflicting_definitions = HttpApiDefinition::find_conflicts(
            new_definitions
                .clone()
                .into_iter()
                .map(|x| x.into())
                .collect::<Vec<HttpApiDefinition>>()
                .as_slice(),
        );

        if !conflicting_definitions.is_empty() {
            let conflicting_definitions = conflicting_definitions
                .iter()
                .map(|def| format!("{}", def))
                .collect::<Vec<_>>()
                .join(", ");

            info!(namespace = %deployment.namespace,
                "Deploy API definition - failed, conflicting definitions: {}",
                conflicting_definitions
            );
            Err(ApiDeploymentError::ApiDefinitionsConflict(
                conflicting_definitions,
            ))
        } else if !new_deployment_records.is_empty() {
            for api_definition_key in set_not_draft {
                info!(namespace = %deployment.namespace,
                    "Set API definition as not draft - definition id: {}, definition version: {}",
                    api_definition_key.id, api_definition_key.version
                );

                self.definition_repo
                    .set_draft(
                        deployment.namespace.to_string().as_str(),
                        api_definition_key.id.0.as_str(),
                        api_definition_key.version.0.as_str(),
                        false,
                    )
                    .await?;
            }

            let constraints =
                Self::get_worker_functions_in_api_definitions(new_definitions.clone())?;

            for (component_id, constraints) in constraints {
                self.component_service
                    .create_or_update_constraints(&component_id, constraints, auth_ctx)
                    .await
                    .map_err(|err| {
                        ApiDeploymentError::ComponentConstraintCreateError(err.to_safe_string())
                    })?;
            }

            self.deployment_repo.create(new_deployment_records).await?;
            Ok(())
        } else {
            Ok(())
        }
    }

    async fn undeploy(
        &self,
        deployment: &ApiDeploymentRequest<Namespace>,
    ) -> Result<(), ApiDeploymentError<Namespace>> {
        info!(namespace = %deployment.namespace, "Undeploying API definitions");

        // Existing deployment
        let existing_deployment_records = self
            .deployment_repo
            .get_by_site(deployment.site.to_string().as_str())
            .await?;

        let mut remove_deployment_records: Vec<ApiDeploymentRecord> = vec![];

        for deployment_record in existing_deployment_records {
            if deployment_record.namespace != deployment.namespace.to_string()
                || deployment_record.subdomain != deployment.site.subdomain
                || deployment_record.host != deployment.site.host
            {
                error!("Undeploying API definition - failed, site used by another API (under another namespace/API)");
                return Err(ApiDeploymentError::ApiDeploymentConflict(
                    ApiSiteString::from(&ApiSite {
                        host: deployment_record.host,
                        subdomain: deployment_record.subdomain,
                    }),
                ));
            }

            if deployment
                .api_definition_keys
                .clone()
                .into_iter()
                .any(|key| {
                    deployment_record.definition_id == key.id.0
                        && deployment_record.definition_version == key.version.0
                })
            {
                remove_deployment_records.push(deployment_record);
            }
        }

        if !remove_deployment_records.is_empty() {
            self.deployment_repo
                .delete(remove_deployment_records.clone())
                .await?;

            self.set_undeployed_as_draft(remove_deployment_records)
                .await?;
        }

        Ok(())
    }

    async fn get_by_id(
        &self,
        namespace: &Namespace,
        definition_id: &ApiDefinitionId,
    ) -> Result<Vec<ApiDeployment<Namespace>>, ApiDeploymentError<Namespace>> {
        info!(namespace = %namespace, "Get API deployment");

        let existing_deployment_records = self
            .deployment_repo
            .get_by_id(namespace.to_string().as_str(), definition_id.0.as_str())
            .await?;

        let mut values: Vec<ApiDeployment<Namespace>> = vec![];

        for deployment_record in existing_deployment_records {
            let site = ApiSite {
                host: deployment_record.host,
                subdomain: deployment_record.subdomain,
            };

            let namespace: Namespace = deployment_record.namespace.try_into().map_err(
                |e: <Namespace as TryFrom<String>>::Error| {
                    ApiDeploymentError::conversion_error("API deployment namespace", e.to_string())
                },
            )?;

            let api_definition_key = ApiDefinitionIdWithVersion {
                id: deployment_record.definition_id.into(),
                version: deployment_record.definition_version.into(),
            };

            match values
                .iter_mut()
                .find(|val| val.site == site && val.namespace == namespace)
            {
                Some(val) => {
                    val.api_definition_keys.push(api_definition_key);
                }
                None => {
                    values.push(ApiDeployment {
                        site,
                        namespace,
                        api_definition_keys: vec![api_definition_key],
                        created_at: deployment_record.created_at,
                    });
                }
            }
        }

        Ok(values)
    }

    async fn get_by_site(
        &self,
        site: &ApiSiteString,
    ) -> Result<Option<ApiDeployment<Namespace>>, ApiDeploymentError<Namespace>> {
        info!("Get API deployment");
        let existing_deployment_records = self
            .deployment_repo
            .get_by_site(site.to_string().as_str())
            .await?;

        let mut api_definition_keys: Vec<ApiDefinitionIdWithVersion> = vec![];

        let mut site: Option<ApiSite> = None;

        let mut namespace: Option<Namespace> = None;

        let mut created_at: Option<chrono::DateTime<Utc>> = None;

        for deployment_record in existing_deployment_records {
            if site.is_none() {
                site = Some(ApiSite {
                    host: deployment_record.host,
                    subdomain: deployment_record.subdomain,
                });
            }

            if namespace.is_none() {
                namespace = Some(deployment_record.namespace.try_into().map_err(
                    |e: <Namespace as TryFrom<std::string::String>>::Error| {
                        ApiDeploymentError::conversion_error(
                            "API deployment namespace",
                            e.to_string(),
                        )
                    },
                )?);
            }

            if created_at.is_none() || created_at.is_some_and(|t| t > deployment_record.created_at)
            {
                created_at = Some(deployment_record.created_at);
            }

            api_definition_keys.push(ApiDefinitionIdWithVersion {
                id: deployment_record.definition_id.into(),
                version: deployment_record.definition_version.into(),
            });
        }

        match (site, namespace, created_at) {
            (Some(site), Some(namespace), Some(created_at)) => Ok(Some(ApiDeployment {
                namespace,
                site,
                api_definition_keys,
                created_at,
            })),
            _ => Ok(None),
        }
    }

    async fn get_definitions_by_site(
        &self,
        site: &ApiSiteString,
    ) -> Result<Vec<CompiledHttpApiDefinition<Namespace>>, ApiDeploymentError<Namespace>> {
        info!("Get API definitions");
        let records = self
            .deployment_repo
            .get_definitions_by_site(site.to_string().as_str())
            .await?;

        let mut values: Vec<CompiledHttpApiDefinition<Namespace>> = vec![];

        for record in records {
            values.push(
                record.try_into().map_err(|e| {
                    ApiDeploymentError::conversion_error("API definition record", e)
                })?,
            );
        }

        Ok(values)
    }

    async fn delete(
        &self,
        namespace: &Namespace,
        site: &ApiSiteString,
    ) -> Result<(), ApiDeploymentError<Namespace>> {
        info!(namespace = %namespace, "Get API deployment");
        let existing_deployment_records = self
            .deployment_repo
            .get_by_site(site.to_string().as_str())
            .await?;

        if existing_deployment_records.is_empty() {
            Err(ApiDeploymentError::ApiDeploymentNotFound(
                namespace.clone(),
                site.clone(),
            ))
        } else if existing_deployment_records
            .iter()
            .any(|value| value.namespace != namespace.to_string())
        {
            error!(
                "Failed to delete API deployment - site used by another API (under another namespace/API)"
            );

            Err(ApiDeploymentError::ApiDeploymentConflict(site.clone()))
        } else {
            self.deployment_repo
                .delete(existing_deployment_records.clone())
                .await?;

            self.set_undeployed_as_draft(existing_deployment_records)
                .await?;

            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use crate::service::gateway::api_deployment::ApiDeploymentError;
    use golem_common::SafeDisplay;
    use golem_service_base::repo::RepoError;

    #[test]
    pub fn test_repo_error_to_service_error() {
        let repo_err = RepoError::Internal("some sql error".to_string());
        let service_err: ApiDeploymentError<String> = repo_err.into();
        assert_eq!(
            service_err.to_safe_string(),
            "Internal repository error".to_string()
        );
    }
}
