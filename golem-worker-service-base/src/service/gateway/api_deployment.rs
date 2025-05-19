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

use crate::gateway_api_definition::{ApiDefinitionId, ApiVersion};
use crate::gateway_api_deployment::*;

use std::collections::{HashMap, HashSet};

use async_trait::async_trait;
use golem_service_base::auth::{GolemAuthCtx, GolemNamespace};

use std::sync::Arc;
use tracing::{error, info};

use crate::gateway_api_definition::http::{
    AllPathPatterns, CompiledAuthCallBackRoute, CompiledHttpApiDefinition, HttpApiDefinition, Route,
};

use crate::gateway_api_deployment::{ApiDeployment, ApiDeploymentRequest, ApiSite};
use crate::gateway_binding::GatewayBindingCompiled;
use crate::gateway_execution::router::{Router, RouterPattern};
use crate::repo::api_definition::ApiDefinitionRepo;
use crate::repo::api_deployment::ApiDeploymentRecord;
use crate::repo::api_deployment::ApiDeploymentRepo;
use crate::service::component::ComponentService;
use crate::service::gateway::api_definition::ApiDefinitionIdWithVersion;
use chrono::Utc;
use golem_common::model::component_constraint::FunctionConstraints;
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

    // New undeploy function takes ApiSiteString and ApiDefinitionIdWithVersion
    async fn undeploy(
        &self,
        namespace: &Namespace,
        site: ApiSiteString,
        api_definition_key: ApiDefinitionIdWithVersion,
        auth_ctx: &AuthCtx,
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
        namespace: &Namespace,
        site: &ApiSiteString,
    ) -> Result<Option<ApiDeployment<Namespace>>, ApiDeploymentError<Namespace>>;

    async fn get_definitions_by_site(
        &self,
        namespace: &Namespace,
        site: &ApiSiteString,
    ) -> Result<Vec<CompiledHttpApiDefinition<Namespace>>, ApiDeploymentError<Namespace>>;

    /// Get all API definitions deployed in a site
    /// regardless of the namespace, mainly to serve
    /// the http requests to API gateway
    async fn get_all_definitions_by_site(
        &self,
        site: &ApiSiteString,
    ) -> Result<Vec<CompiledHttpApiDefinition<Namespace>>, ApiDeploymentError<Namespace>>;

    async fn delete(
        &self,
        namespace: &Namespace,
        auth_ctx: &AuthCtx,
        site: &ApiSiteString,
    ) -> Result<(), ApiDeploymentError<Namespace>>;
}

#[derive(Debug, thiserror::Error)]
pub enum ApiDeploymentError<Namespace> {
    #[error("Unknown API {1}/{2}")]
    ApiDefinitionNotFound(Namespace, ApiDefinitionId, ApiVersion),
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
            ApiDeploymentError::ApiDefinitionNotFound(_, _, _) => self.to_string(),
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

pub struct ApiDeploymentServiceDefault<Namespace, AuthCtx> {
    pub deployment_repo: Arc<dyn ApiDeploymentRepo + Sync + Send>,
    pub definition_repo: Arc<dyn ApiDefinitionRepo + Sync + Send>,
    pub component_service: Arc<dyn ComponentService<Namespace, AuthCtx> + Send + Sync>,
}

impl<Namespace: GolemNamespace, AuthCtx: GolemAuthCtx>
    ApiDeploymentServiceDefault<Namespace, AuthCtx>
{
    pub fn new(
        deployment_repo: Arc<dyn ApiDeploymentRepo + Sync + Send>,
        definition_repo: Arc<dyn ApiDefinitionRepo + Sync + Send>,
        component_service: Arc<dyn ComponentService<Namespace, AuthCtx> + Send + Sync>,
    ) -> Self {
        Self {
            deployment_repo,
            definition_repo,
            component_service,
        }
    }

    // A site is owned by a namespace (i.e, host and subdomain
    // Only the authorised namespace is allowed to fetch the existing deployments in that site
    async fn fetch_existing_deployments(
        &self,
        namespace: &Namespace,
        site: &ApiSite,
    ) -> Result<Vec<ApiDeploymentRecord>, ApiDeploymentError<Namespace>> {
        let deployments = self
            .deployment_repo
            .get_by_site(&namespace.to_string(), &site.to_string())
            .await?;

        Ok(deployments)
    }

    /// Ensures that the site is not already used by another namespace.
    fn ensure_no_namespace_conflict(
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
    fn check_for_conflicts(
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
    async fn finalize_deployment(
        &self,
        deployment: &ApiDeploymentRequest<Namespace>,
        auth_ctx: &AuthCtx,
        deployment_plan: ApiDeploymentPlan<Namespace>,
    ) -> Result<(), ApiDeploymentError<Namespace>>
    where
        Namespace: Display + TryFrom<String> + Eq + Clone + Send + Sync,
        <Namespace as TryFrom<String>>::Error: Display + Debug + Send + Sync + 'static,
    {
        let existing_deployed_apis = self
            .get_definitions_by_site(&deployment.namespace, &(&deployment.site.clone()).into())
            .await?;

        let mut deployed_auth_call_back_routes = vec![];

        for api_def in &existing_deployed_apis {
            for route in &api_def.routes {
                if let Some(auth_callback_route) = route.as_auth_callback_route() {
                    deployed_auth_call_back_routes.push(auth_callback_route);
                }
            }
        }

        let new_and_old_apis_merged = deployment_plan
            .remove_existing_deployed_auth_call_backs(deployed_auth_call_back_routes.as_slice())
            .into_iter()
            .chain(existing_deployed_apis)
            .collect::<Vec<_>>();

        self.check_for_conflicts(&deployment.namespace, &new_and_old_apis_merged)?;

        if deployment_plan.is_empty() {
            return Ok(());
        }

        // Setting draft to true for all definitions that were never deployed to any site
        for draft_api in deployment_plan.draft_api_defs() {
            info!(namespace = %deployment.namespace,
                "Set API definition as not draft - definition id: {}, definition version: {}",
                draft_api.id, draft_api.version
            );

            self.definition_repo
                .set_draft(
                    &deployment.namespace.to_string(),
                    &draft_api.id.0,
                    &draft_api.version.0,
                    false,
                )
                .await?;
        }

        // Find component constraints and update
        let constraints =
            ComponentConstraints::from_api_definitions(&deployment_plan.apis_to_deploy)?;

        for (component_id, constraints) in constraints.constraints {
            self.component_service
                .create_or_update_constraints(&component_id, constraints, auth_ctx)
                .await
                .map_err(|err| {
                    ApiDeploymentError::ComponentConstraintCreateError(err.to_safe_string())
                })?;
        }

        self.deployment_repo
            .create(
                &deployment_plan.namespace.to_string(),
                deployment_plan.deployment_records(),
            )
            .await?;

        Ok(())
    }

    async fn remove_component_constraints(
        &self,
        existing_api_definitions: Vec<CompiledHttpApiDefinition<Namespace>>,
        auth_ctx: &AuthCtx,
    ) -> Result<(), ApiDeploymentError<Namespace>>
    where
        Namespace: Display + TryFrom<String> + Eq + Clone + Send + Sync,
        <Namespace as TryFrom<String>>::Error: Display + Debug + Send + Sync + 'static,
    {
        let constraints = ComponentConstraints::from_api_definitions(&existing_api_definitions)?;

        for (component_id, constraints) in &constraints.constraints {
            let signatures_to_be_removed = constraints
                .constraints
                .iter()
                .map(|x| x.function_signature.clone())
                .collect::<Vec<_>>();

            self.component_service
                .delete_constraints(component_id, &signatures_to_be_removed, auth_ctx)
                .await
                .map_err(|err| {
                    ApiDeploymentError::ComponentConstraintCreateError(err.to_safe_string())
                })?;
        }

        Ok(())
    }

    async fn set_undeployed_as_draft(
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
impl<Namespace: GolemNamespace, AuthCtx: GolemAuthCtx> ApiDeploymentService<AuthCtx, Namespace>
    for ApiDeploymentServiceDefault<Namespace, AuthCtx>
{
    async fn deploy(
        &self,
        deployment_request: &ApiDeploymentRequest<Namespace>,
        auth_ctx: &AuthCtx,
    ) -> Result<(), ApiDeploymentError<Namespace>> {
        info!(namespace = %deployment_request.namespace, "Deploy API definitions");

        let existing_deployment_records = self
            .fetch_existing_deployments(&deployment_request.namespace, &deployment_request.site)
            .await?;

        self.ensure_no_namespace_conflict(deployment_request, &existing_deployment_records)?;

        let new_deployment = ApiDeploymentPlan::create(
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
        namespace: &Namespace,
        site: ApiSiteString,
        api_definition_key: ApiDefinitionIdWithVersion,
        auth_ctx: &AuthCtx,
    ) -> Result<(), ApiDeploymentError<Namespace>> {
        info!(namespace = %namespace, "Undeploying API definition");

        // 1. Check if the site exists
        let site_exists = self.get_by_site(namespace, &site).await?.is_some();

        if !site_exists {
            return Err(ApiDeploymentError::ApiDeploymentNotFound(
                namespace.clone(),
                site.clone(),
            ));
        }

        // 2. Check if the API definition exists in the site
        let api_definition_exists = self
            .get_by_id(namespace, Some(api_definition_key.id.clone()))
            .await?
            .iter()
            .any(|deployment| {
                deployment.api_definition_keys.iter().any(|key| {
                    key.id == api_definition_key.id && key.version == api_definition_key.version
                })
            });

        if !api_definition_exists {
            return Err(ApiDeploymentError::ApiDefinitionNotFound(
                namespace.clone(),
                api_definition_key.id,
                api_definition_key.version,
            ));
        }

        // 3. Get existing deployment records for the site
        let existing_deployment_records = self
            .deployment_repo
            .get_by_site(&namespace.to_string(), &site.to_string())
            .await?;

        // 4. Filter records that match the API definition key to undeploy
        let mut remove_deployment_records: Vec<ApiDeploymentRecord> = vec![];
        for deployment_record in existing_deployment_records {
            if deployment_record.definition_id == api_definition_key.id.0
                && deployment_record.definition_version == api_definition_key.version.0
            {
                remove_deployment_records.push(deployment_record);
            }
        }

        if !remove_deployment_records.is_empty() {
            // 5. Get the specific API definition being undeployed
            let definition_to_undeploy = self
                .definition_repo
                .get(
                    &namespace.to_string(),
                    &api_definition_key.id.0,
                    &api_definition_key.version.0,
                )
                .await?;

            if let Some(definition) = definition_to_undeploy {
                let compiled_definition =
                    CompiledHttpApiDefinition::try_from(definition).map_err(|e| {
                        ApiDeploymentError::conversion_error("API definition record", e)
                    })?;

                // 6. Remove component constraints
                self.remove_component_constraints(vec![compiled_definition], auth_ctx)
                    .await?;

                // 7. Delete deployment records
                self.deployment_repo
                    .delete(&namespace.to_string(), remove_deployment_records.clone())
                    .await?;

                // 8. Set undeployed as draft
                self.set_undeployed_as_draft(remove_deployment_records)
                    .await?;
            }
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
        namespace: &Namespace,
        site: &ApiSiteString,
    ) -> Result<Option<ApiDeployment<Namespace>>, ApiDeploymentError<Namespace>> {
        info!("Get API deployment");
        let existing_deployment_records = self
            .deployment_repo
            .get_by_site(&namespace.to_string(), &site.to_string())
            .await?;

        let mut api_definition_keys: Vec<ApiDefinitionIdWithVersion> = vec![];
        let mut site: Option<ApiSite> = None;
        let mut created_at: Option<chrono::DateTime<Utc>> = None;

        for deployment_record in existing_deployment_records {
            // Retrieving the original domain and subdomain from the deployment record
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

        match (site, created_at) {
            (Some(site), Some(created_at)) => Ok(Some(ApiDeployment {
                namespace: namespace.clone(),
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
        auth_ctx: &AuthCtx,
        site: &ApiSiteString,
    ) -> Result<(), ApiDeploymentError<Namespace>> {
        info!(namespace = %namespace, "Get API deployment");

        // Not sure of the purpose of retrieving records at repo level to delete API deployment
        // https://github.com/golemcloud/golem/issues/1443
        let existing_deployment_records = self
            .deployment_repo
            .get_by_site(&namespace.to_string(), &site.to_string())
            .await?;

        if existing_deployment_records.is_empty() {
            Err(ApiDeploymentError::ApiDeploymentNotFound(
                namespace.clone(),
                site.clone(),
            ))
        } else {
            // API definitions corresponding to the deployment
            let existing_api_definitions = self.get_definitions_by_site(namespace, site).await?;

            self.deployment_repo
                .delete(&namespace.to_string(), existing_deployment_records.clone())
                .await?;

            self.set_undeployed_as_draft(existing_deployment_records)
                .await?;

            self.remove_component_constraints(existing_api_definitions, auth_ctx)
                .await?;

            Ok(())
        }
    }
}
// A structure representing the new deployments to be created
// by comparing the deployments that already exist with the new request.
struct ApiDeploymentPlan<Namespace> {
    namespace: Namespace,
    site: ApiSite,
    apis_to_deploy: Vec<CompiledHttpApiDefinition<Namespace>>,
}

impl<Namespace: Display + Clone> ApiDeploymentPlan<Namespace>
where
    Namespace: TryFrom<String>,
    <Namespace as TryFrom<String>>::Error: Display,
{
    pub async fn create(
        deployment_request: &ApiDeploymentRequest<Namespace>,
        deployment_repo: &Arc<dyn ApiDeploymentRepo + Sync + Send>,
        definition_repo: &Arc<dyn ApiDefinitionRepo + Sync + Send>,
    ) -> Result<ApiDeploymentPlan<Namespace>, ApiDeploymentError<Namespace>> {
        let mut new_definitions_to_deploy = Vec::new();

        let existing_deployed_api_def_keys = deployment_repo
            .get_by_site(
                &deployment_request.namespace.to_string(),
                &deployment_request.site.to_string(),
            )
            .await?
            .into_iter()
            .map(|record| ApiDefinitionIdWithVersion {
                id: record.definition_id.into(),
                version: record.definition_version.into(),
            })
            .collect::<HashSet<_>>();

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
                    new_definitions_to_deploy.push(api_def);
                }
                None => {
                    return Err(ApiDeploymentError::ApiDefinitionNotFound(
                        deployment_request.namespace.clone(),
                        api_key_to_deploy.id.clone(),
                        api_key_to_deploy.version.clone(),
                    ));
                }
            }
        }

        Ok(ApiDeploymentPlan {
            namespace: deployment_request.namespace.clone(),
            site: deployment_request.site.clone(),
            apis_to_deploy: new_definitions_to_deploy,
        })
    }

    pub fn remove_existing_deployed_auth_call_backs(
        &self,
        deployed_auth_call_back_routes: &[CompiledAuthCallBackRoute],
    ) -> Vec<CompiledHttpApiDefinition<Namespace>> {
        self.apis_to_deploy
            .iter()
            .map(|def| def.remove_auth_call_back_routes(deployed_auth_call_back_routes))
            .collect::<Vec<_>>()
    }

    pub fn is_empty(&self) -> bool {
        self.apis_to_deploy.is_empty()
    }

    // All the new API definitions (in the plan) to be deployed in this site
    // may not be draft API defnitions as some of them may have been already
    // deployed in other sites.
    // This function retrieves all the api definitions to be deployed that are still draft
    pub fn draft_api_defs(&self) -> Vec<&CompiledHttpApiDefinition<Namespace>> {
        self.apis_to_deploy
            .iter()
            .filter(|def| def.draft)
            .collect::<Vec<_>>()
    }

    pub fn deployment_records(&self) -> Vec<ApiDeploymentRecord> {
        let created_at = Utc::now();

        self.apis_to_deploy
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

#[derive(Debug)]
struct ComponentConstraints {
    constraints: HashMap<ComponentId, FunctionConstraints>,
}

impl ComponentConstraints {
    fn from_api_definitions<Namespace>(
        definitions: &Vec<CompiledHttpApiDefinition<Namespace>>,
    ) -> Result<Self, ApiDeploymentError<Namespace>> {
        let mut worker_functions_in_rib = HashMap::new();

        for definition in definitions {
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
    ) -> Result<HashMap<ComponentId, FunctionConstraints>, ApiDeploymentError<Namespace>> {
        let mut merged_worker_functions: HashMap<ComponentId, FunctionConstraints> = HashMap::new();

        for (component_id, worker_functions_in_rib) in worker_functions {
            let function_constraints = worker_functions_in_rib
                .iter()
                .map(FunctionConstraints::from_worker_functions_in_rib)
                .collect::<Vec<_>>();

            let merged_calls = FunctionConstraints::try_merge(function_constraints)
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
