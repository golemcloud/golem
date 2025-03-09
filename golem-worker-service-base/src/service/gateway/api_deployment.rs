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
    AllPathPatterns, CompiledAuthCallBackRoute, CompiledHttpApiDefinition, HttpApiDefinition, Route,
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
        api_definition_id: Option<ApiDefinitionId>,
    ) -> Result<Vec<ApiDeployment<Namespace>>, ApiDeploymentError<Namespace>>;

    async fn get_by_site(
        &self,
        site: &ApiSiteString,
    ) -> Result<Option<ApiDeployment<Namespace>>, ApiDeploymentError<Namespace>>;

    async fn get_definitions_by_site(
        &self,
        namespace: &Namespace,
        site: &ApiSiteString,
    ) -> Result<Vec<CompiledHttpApiDefinition<Namespace>>, ApiDeploymentError<Namespace>>;

    async fn get_all_definitions_by_site(
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
    #[error("Unknown API: {1}")]
    ApiDefinitionNotFound(Namespace, ApiDefinitionId),
    #[error("Unknown authority or domain: {1}")]
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

impl<AuthCtx: Send + Sync> ApiDeploymentServiceDefault<AuthCtx> {
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

    async fn fetch_existing_deployments<Namespace>(
        &self,
        site: &ApiSite,
    ) -> Result<Vec<ApiDeploymentRecord>, ApiDeploymentError<Namespace>> {
        let deployments = self.deployment_repo.get_by_site(&site.to_string()).await?;

        Ok(deployments)
    }

    /// Ensures that the site is not already used by another namespace.
    fn ensure_no_namespace_conflict<Namespace: Display>(
        &self,
        deployment: &ApiDeploymentRequest<Namespace>,
        existing_records: &[ApiDeploymentRecord],
    ) -> Result<(), ApiDeploymentError<Namespace>> {
        for record in existing_records {
            if record.namespace != deployment.namespace.to_string()
                || record.subdomain != deployment.site.subdomain
                || record.host != deployment.site.host
            {
                info!(namespace = %deployment.namespace,
                    "Deploying API definition - failed, site used by another API (under another namespace/API)",
                );
                return Err(ApiDeploymentError::ApiDeploymentConflict(
                    ApiSiteString::from(&ApiSite {
                        host: record.host.clone(),
                        subdomain: record.subdomain.clone(),
                    }),
                ));
            }
        }
        Ok(())
    }

    /// Checks for conflicts among API definitions.
    fn check_for_conflicts<Namespace: Display + Clone>(
        &self,
        namespace: &Namespace,
        all_definitions: &[CompiledHttpApiDefinition<Namespace>],
    ) -> Result<(), ApiDeploymentError<Namespace>> {
        let conflicts = HttpApiDefinition::find_conflicts(
            &all_definitions
                .iter()
                .map(|x| HttpApiDefinition::from((*x).clone()))
                .collect::<Vec<_>>(),
        );

        if conflicts.is_empty() {
            Ok(())
        } else {
            let conflicts_str = conflicts
                .iter()
                .map(|def| def.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            info!(namespace = %namespace, "Deploy API definition - failed, conflicting definitions: {}", conflicts_str);
            Err(ApiDeploymentError::ApiDefinitionsConflict(conflicts_str))
        }
    }

    /// Finalizes the deployment by marking drafts, updating constraints, and saving records.
    async fn finalize_deployment<Namespace>(
        &self,
        deployment: &ApiDeploymentRequest<Namespace>,
        auth_ctx: &AuthCtx,
        new_deployment: NewDeployment<Namespace>,
    ) -> Result<(), ApiDeploymentError<Namespace>>
    where
        Namespace: Display + TryFrom<String> + Eq + Clone + Send + Sync,
        <Namespace as TryFrom<String>>::Error: Display + Debug + Send + Sync + 'static,
    {
        // Find conflicts
        let deployed_defs = self
            .get_definitions_by_site(&deployment.namespace, &(&deployment.site.clone()).into())
            .await?;

        let mut deployed_auth_call_back_routes = vec![];

        for api_def in &deployed_defs {
            for route in &api_def.routes {
                if let Some(auth_callback_route) = route.as_auth_callback_route() {
                    deployed_auth_call_back_routes.push(auth_callback_route);
                }
            }
        }

        let new_auth_call_back_routes = new_deployment.auth_call_back_routes.clone();

        let already_deployed_call_back_routes = new_auth_call_back_routes
            .iter()
            .filter(|new_auth_call_back_route| {
                !deployed_auth_call_back_routes
                    .iter()
                    .any(|deployed_auth_call_back_route| {
                        new_auth_call_back_route == &deployed_auth_call_back_route
                    })
            })
            .collect::<Vec<_>>();

        let all_definitions = new_deployment
            .api_defs_to_deploy
            .iter()
            .map(|def| {
                def.remove_auth_call_back_routes(already_deployed_call_back_routes.as_slice())
            })
            .chain(deployed_defs)
            .collect::<Vec<_>>();

        self.check_for_conflicts(&deployment.namespace, &all_definitions)?;

        // If there is nothing to deploy return Ok
        if new_deployment.is_empty() {
            return Ok(());
        }

        // Setting draft to true for all definitions that were never deployed
        for api_key in new_deployment.never_deployed_api_defs() {
            info!(namespace = %deployment.namespace,
                "Set API definition as not draft - definition id: {}, definition version: {}",
                api_key.id, api_key.version
            );

            // TODO; setting draft false should be transactional with the actual deployment
            self.definition_repo
                .set_draft(
                    &deployment.namespace.to_string(),
                    &api_key.id.0,
                    &api_key.version.0,
                    false,
                )
                .await?;
        }

        // Find component constraints and update
        let constraints = ComponentConstraints::from_new_deployment(&new_deployment)?;

        for (component_id, constraints) in constraints.constraints {
            self.component_service
                .create_or_update_constraints(&component_id, constraints, auth_ctx)
                .await
                .map_err(|err| {
                    ApiDeploymentError::ComponentConstraintCreateError(err.to_safe_string())
                })?;
        }

        self.deployment_repo
            .create(new_deployment.deployment_records())
            .await?;

        Ok(())
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
        deployment_request: &ApiDeploymentRequest<Namespace>,
        auth_ctx: &AuthCtx,
    ) -> Result<(), ApiDeploymentError<Namespace>> {
        info!(namespace = %deployment_request.namespace, "Deploy API definitions");

        let existing_deployment_records = self
            .fetch_existing_deployments(&deployment_request.site)
            .await?;

        self.ensure_no_namespace_conflict(deployment_request, &existing_deployment_records)?;

        let new_deployment = NewDeployment::from_deployment_request(
            deployment_request,
            &self.deployment_repo,
            &self.definition_repo,
        )
        .await?;

        self.finalize_deployment(deployment_request, auth_ctx, new_deployment)
            .await
    }

    async fn undeploy(
        &self,
        deployment: &ApiDeploymentRequest<Namespace>,
    ) -> Result<(), ApiDeploymentError<Namespace>> {
        info!(namespace = %deployment.namespace, "Undeploying API definitions");

        // Existing deployment
        let existing_deployment_records = self
            .deployment_repo
            .get_by_site(&deployment.site.to_string())
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
        definition_id: Option<ApiDefinitionId>,
    ) -> Result<Vec<ApiDeployment<Namespace>>, ApiDeploymentError<Namespace>> {
        info!(namespace = %namespace, "Get API deployment");

        let existing_deployment_records = match definition_id {
            Some(definition_id) => {
                self.deployment_repo
                    .get_by_id(namespace.to_string().as_str(), definition_id.0.as_str())
                    .await?
            }
            None => {
                self.deployment_repo
                    .get_all(namespace.to_string().as_str())
                    .await?
            }
        };

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
        let existing_deployment_records =
            self.deployment_repo.get_by_site(&site.to_string()).await?;

        let mut api_definition_keys: Vec<ApiDefinitionIdWithVersion> = vec![];
        let mut namespace: Option<Namespace> = None;
        let mut site: Option<ApiSite> = None;
        let mut created_at: Option<chrono::DateTime<Utc>> = None;

        for deployment_record in existing_deployment_records {
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

            if site.is_none() {
                site = Some(ApiSite {
                    host: deployment_record.host,
                    subdomain: deployment_record.subdomain,
                });
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
        namespace: &Namespace,
        site: &ApiSiteString,
    ) -> Result<Vec<CompiledHttpApiDefinition<Namespace>>, ApiDeploymentError<Namespace>> {
        info!(namespace = %namespace, "Get API definitions");
        let records = self
            .deployment_repo
            .get_definitions_by_site(&namespace.to_string(), &site.to_string())
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

    async fn get_all_definitions_by_site(
        &self,
        site: &ApiSiteString,
    ) -> Result<Vec<CompiledHttpApiDefinition<Namespace>>, ApiDeploymentError<Namespace>> {
        info!("Get all API definitions");
        let records = self
            .deployment_repo
            .get_all_definitions_by_site(&site.to_string())
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
        let existing_deployment_records =
            self.deployment_repo.get_by_site(&site.to_string()).await?;

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

// A structure representing the new deployments to be created
// and it can only be created from a deployment request
struct NewDeployment<Namespace> {
    namespace: Namespace,
    site: ApiSite,
    api_defs_to_deploy: Vec<CompiledHttpApiDefinition<Namespace>>,
    auth_call_back_routes: Vec<CompiledAuthCallBackRoute>,
}

impl<Namespace: Display + Clone> NewDeployment<Namespace>
where
    Namespace: TryFrom<String>,
    <Namespace as TryFrom<String>>::Error: Display,
{
    pub async fn from_deployment_request(
        deployment_request: &ApiDeploymentRequest<Namespace>,
        deployment_repo: &Arc<dyn ApiDeploymentRepo + Sync + Send>,
        definition_repo: &Arc<dyn ApiDefinitionRepo + Sync + Send>,
    ) -> Result<NewDeployment<Namespace>, ApiDeploymentError<Namespace>> {
        let mut new_definitions_to_deploy = Vec::new();

        let existing_deployed_api_def_keys = deployment_repo
            .get_by_site(&deployment_request.site.to_string())
            .await?
            .into_iter()
            .map(|record| ApiDefinitionIdWithVersion {
                id: record.definition_id.into(),
                version: record.definition_version.into(),
            })
            .collect::<HashSet<_>>();

        let mut auth_call_back_routes = vec![];

        for api_key_to_deploy in &deployment_request.api_definition_keys {
            if existing_deployed_api_def_keys.contains(api_key_to_deploy) {
                continue;
            }

            match Self::get_api_definition_details(
                &deployment_request.namespace,
                api_key_to_deploy,
                definition_repo,
            )
            .await?
            {
                Some(api_def) => {
                    for route in &api_def.routes {
                        if let Some(auth_callback_route) = route.as_auth_callback_route() {
                            auth_call_back_routes.push(auth_callback_route);
                        }
                    }
                    new_definitions_to_deploy.push(api_def);
                }
                None => {
                    return Err(ApiDeploymentError::ApiDefinitionNotFound(
                        deployment_request.namespace.clone(),
                        api_key_to_deploy.id.clone(),
                    ));
                }
            }
        }

        Ok(NewDeployment {
            namespace: deployment_request.namespace.clone(),
            site: deployment_request.site.clone(),
            api_defs_to_deploy: new_definitions_to_deploy,
            auth_call_back_routes,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.api_defs_to_deploy.is_empty()
    }

    pub fn never_deployed_api_defs(&self) -> Vec<&CompiledHttpApiDefinition<Namespace>> {
        self.api_defs_to_deploy
            .iter()
            .filter(|def| def.draft)
            .collect::<Vec<_>>()
    }

    pub fn deployment_records(&self) -> Vec<ApiDeploymentRecord> {
        let created_at = Utc::now();

        self.api_defs_to_deploy
            .iter()
            .map(|def| {
                ApiDeploymentRecord::new(
                    self.namespace.to_string(),
                    self.site.clone(),
                    ApiDefinitionIdWithVersion {
                        id: def.id.clone(),
                        version: def.version.clone(),
                    },
                    created_at,
                )
            })
            .collect()
    }

    async fn get_api_definition_details(
        namespace: &Namespace,
        api_key: &ApiDefinitionIdWithVersion,
        definition_repo: &Arc<dyn ApiDefinitionRepo + Sync + Send>,
    ) -> Result<Option<CompiledHttpApiDefinition<Namespace>>, ApiDeploymentError<Namespace>>
    where
        Namespace: TryFrom<String>,
        <Namespace as TryFrom<String>>::Error: Display,
    {
        let result = definition_repo
            .get(&namespace.to_string(), &api_key.id.0, &api_key.version.0)
            .await?;

        match result {
            Some(api_def_record) => Ok(Some(
                CompiledHttpApiDefinition::try_from(api_def_record).map_err(|e| {
                    ApiDeploymentError::conversion_error("API definition record", e)
                })?,
            )),
            None => Ok(None),
        }
    }
}

struct ComponentConstraints {
    constraints: HashMap<ComponentId, FunctionConstraintCollection>,
}

impl ComponentConstraints {
    fn from_new_deployment<Namespace>(
        new_deployment: &NewDeployment<Namespace>,
    ) -> Result<Self, ApiDeploymentError<Namespace>> {
        let mut worker_functions_in_rib = HashMap::new();

        for definition in &new_deployment.api_defs_to_deploy {
            for route in definition.routes.iter() {
                if let GatewayBindingCompiled::Worker(worker_binding) = route.binding.clone() {
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

        let constraints = Self::merge_worker_functions_in_rib(worker_functions_in_rib)?;

        Ok(Self { constraints })
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
