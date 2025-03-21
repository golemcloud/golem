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

use crate::gateway_api_definition::http::{
    CompiledHttpApiDefinition, ComponentMetadataDictionary, HttpApiDefinition,
    HttpApiDefinitionRequest, OpenApiHttpApiDefinition, RouteCompilationErrors,
};
use crate::gateway_api_definition::{ApiDefinitionId, ApiVersion, HasGolemBindings};
use crate::gateway_security::IdentityProviderError;
use crate::repo::api_definition::ApiDefinitionRecord;
use crate::repo::api_definition::ApiDefinitionRepo;
use crate::repo::api_deployment::ApiDeploymentRepo;
use crate::service::component::{ComponentService, ComponentServiceError};
use crate::service::gateway::api_definition_validator::{
    ApiDefinitionValidatorService, ValidationErrors,
};
use crate::service::gateway::security_scheme::{SecuritySchemeService, SecuritySchemeServiceError};
use async_trait::async_trait;
use chrono::Utc;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::model::ComponentId;
use golem_common::SafeDisplay;
use golem_service_base::model::{Component, ComponentName, VersionedComponentId};
use golem_service_base::repo::RepoError;
use rib::RibError;
use std::fmt::{Debug, Display};
use std::hash::Hash;
use std::sync::Arc;
use tracing::{error, info};

use super::{BoxConversionContext, ConversionContext};

pub type ApiResult<T> = Result<T, ApiDefinitionError>;

#[derive(
    Eq, Hash, PartialEq, Clone, Debug, serde::Deserialize, bincode::Encode, bincode::Decode,
)]
pub struct ApiDefinitionIdWithVersion {
    pub id: ApiDefinitionId,
    pub version: ApiVersion,
}

#[derive(Debug, thiserror::Error)]
pub enum ApiDefinitionError {
    #[error(transparent)]
    ValidationError(#[from] ValidationErrors),
    #[error("Unable to fetch component: {}", .0.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(", "))]
    ComponentNotFoundError(Vec<VersionedComponentId>),
    #[error("Rib compilation error: {0}")]
    RibCompilationErrors(String),
    #[error("Rib internal error: {0}")]
    RibInternal(String),
    #[error("Invalid rib script: {0}")]
    InvalidRibScript(String),
    #[error("Security Scheme Error: {0}")]
    SecuritySchemeError(SecuritySchemeServiceError),
    #[error("Identity Provider Error: {0}")]
    IdentityProviderError(IdentityProviderError),
    #[error("API definition not found: {0}")]
    ApiDefinitionNotFound(ApiDefinitionId),
    #[error("API definition is not draft: {0}")]
    ApiDefinitionNotDraft(ApiDefinitionId),
    #[error("API definition {0} already exists with the same version: {1}")]
    ApiDefinitionAlreadyExists(ApiDefinitionId, ApiVersion),
    #[error("API definition deployed: {0}")]
    ApiDefinitionDeployed(String),
    #[error("Internal repository error: {0}")]
    InternalRepoError(RepoError),
    #[error("Internal error: {0}")]
    Internal(String),
    #[error("Invalid openapi api definition: {0}")]
    InvalidOasDefinition(String),
}

impl ApiDefinitionError {}

impl From<RepoError> for ApiDefinitionError {
    fn from(error: RepoError) -> Self {
        ApiDefinitionError::InternalRepoError(error)
    }
}

impl SafeDisplay for ApiDefinitionError {
    fn to_safe_string(&self) -> String {
        match self {
            ApiDefinitionError::ValidationError(inner) => inner.to_safe_string(),
            ApiDefinitionError::ComponentNotFoundError(_) => self.to_string(),
            ApiDefinitionError::RibCompilationErrors(_) => self.to_string(),
            ApiDefinitionError::ApiDefinitionNotFound(_) => self.to_string(),
            ApiDefinitionError::ApiDefinitionNotDraft(_) => self.to_string(),
            ApiDefinitionError::ApiDefinitionAlreadyExists(_, _) => self.to_string(),
            ApiDefinitionError::IdentityProviderError(inner) => inner.to_safe_string(),
            ApiDefinitionError::ApiDefinitionDeployed(_) => self.to_string(),
            ApiDefinitionError::InternalRepoError(inner) => inner.to_safe_string(),
            ApiDefinitionError::Internal(_) => self.to_string(),
            ApiDefinitionError::SecuritySchemeError(inner) => inner.to_safe_string(),
            ApiDefinitionError::RibInternal(_) => self.to_string(),
            ApiDefinitionError::InvalidRibScript(_) => self.to_string(),
            ApiDefinitionError::InvalidOasDefinition(_) => self.to_string(),
        }
    }
}

impl From<RouteCompilationErrors> for ApiDefinitionError {
    fn from(error: RouteCompilationErrors) -> Self {
        match error {
            RouteCompilationErrors::RibError(e) => match e {
                RibError::RibCompilationError(e) => {
                    ApiDefinitionError::RibCompilationErrors(e.to_string())
                }
                RibError::InternalError(e) => ApiDefinitionError::RibInternal(e),
                RibError::InvalidRibScript(e) => ApiDefinitionError::InvalidRibScript(e),
            },
            RouteCompilationErrors::MetadataNotFoundError(e) => {
                ApiDefinitionError::RibCompilationErrors(format!(
                    "Failed to find the metadata of the component {}",
                    e
                ))
            }
        }
    }
}

// A namespace here can be example: (account, project) etc.
// Ideally a repo service and its implementation with a different service impl that takes care of
// validations, authorisations etc is the right approach. However we are keeping it simple for now.
#[async_trait]
pub trait ApiDefinitionService<AuthCtx, Namespace> {
    async fn create(
        &self,
        definition: &HttpApiDefinitionRequest,
        namespace: &Namespace,
        auth_ctx: &AuthCtx,
    ) -> ApiResult<CompiledHttpApiDefinition<Namespace>>;

    async fn create_with_oas(
        &self,
        definition: &OpenApiHttpApiDefinition,
        namespace: &Namespace,
        auth_ctx: &AuthCtx,
    ) -> ApiResult<CompiledHttpApiDefinition<Namespace>>;

    async fn update(
        &self,
        definition: &HttpApiDefinitionRequest,
        namespace: &Namespace,
        auth_ctx: &AuthCtx,
    ) -> ApiResult<CompiledHttpApiDefinition<Namespace>>;

    async fn update_with_oas(
        &self,
        definition: &OpenApiHttpApiDefinition,
        namespace: &Namespace,
        auth_ctx: &AuthCtx,
    ) -> ApiResult<CompiledHttpApiDefinition<Namespace>>;

    async fn get(
        &self,
        id: &ApiDefinitionId,
        version: &ApiVersion,
        namespace: &Namespace,
        auth_ctx: &AuthCtx,
    ) -> ApiResult<Option<CompiledHttpApiDefinition<Namespace>>>;

    async fn delete(
        &self,
        id: &ApiDefinitionId,
        version: &ApiVersion,
        namespace: &Namespace,
        auth_ctx: &AuthCtx,
    ) -> ApiResult<()>;

    async fn get_all(
        &self,
        namespace: &Namespace,
        auth_ctx: &AuthCtx,
    ) -> ApiResult<Vec<CompiledHttpApiDefinition<Namespace>>>;

    async fn get_all_versions(
        &self,
        id: &ApiDefinitionId,
        namespace: &Namespace,
        auth_ctx: &AuthCtx,
    ) -> ApiResult<Vec<CompiledHttpApiDefinition<Namespace>>>;

    fn conversion_context<'a>(&'a self, auth_ctx: &'a AuthCtx) -> BoxConversionContext<'a>
    where
        AuthCtx: 'a;
}

type ComponentNameCache = Cache<
    ComponentId,
    (),
    String,
    String,
>;

type ComponentIdCache = Cache<
    String,
    (),
    ComponentId,
    String,
>;

#[derive(Clone)]
pub struct ApiDefinitionServiceConfig {
    component_name_cache_size: usize,
    component_id_cache_size: usize
}

impl Default for ApiDefinitionServiceConfig {
    fn default() -> Self {
        Self {
            component_name_cache_size: 1024,
            component_id_cache_size: 1024,
        }
    }
}

// TODO: cache mappings
struct ConversionContextImpl<'a, AuthCtx> {
    component_service: &'a Arc<dyn ComponentService<AuthCtx>>,
    auth_ctx: &'a AuthCtx,
    component_name_cache: &'a ComponentNameCache,
    component_id_cache: &'a ComponentIdCache
}

#[async_trait]
impl<AuthCtx: Send + Sync> ConversionContext for ConversionContextImpl<'_, AuthCtx> {
    async fn resolve_component_id(&self, name: &str) -> Result<ComponentId, String> {
        let result = self.component_id_cache.get_or_insert_simple(
            &name.to_string(),
            || Box::pin(async {
                self
                    .component_service
                    .get_by_name(name, self.auth_ctx)
                    .await
                    .map_err(|e| format!("Failed to lookup component by name: {e}"))
            })
        );
        let component = self
            .component_service
            .get_by_name(name, self.auth_ctx)
            .await
            .map_err(|e| format!("Failed to lookup component by name: {e}"))?;
        Ok(component.versioned_component_id.component_id)
    }
    async fn get_component_name(
        &self,
        component_id: &ComponentId,
    ) -> Result<ComponentName, String> {
        let component = self
            .component_service
            .get_latest(component_id, self.auth_ctx)
            .await
            .map_err(|e| format!("Failed to lookup component by id: {e}"))?;
        Ok(component.component_name)
    }
}

pub struct ApiDefinitionServiceDefault<AuthCtx, Namespace> {
    component_service: Arc<dyn ComponentService<AuthCtx>>,
    definition_repo: Arc<dyn ApiDefinitionRepo + Sync + Send>,
    deployment_repo: Arc<dyn ApiDeploymentRepo + Sync + Send>,
    security_scheme_service: Arc<dyn SecuritySchemeService<Namespace> + Sync + Send>,
    api_definition_validator:
        Arc<dyn ApiDefinitionValidatorService<HttpApiDefinition> + Sync + Send>,
    component_name_cache: ComponentNameCache,
    component_id_cache: ComponentIdCache
}

impl<AuthCtx, Namespace> ApiDefinitionServiceDefault<AuthCtx, Namespace> {
    pub fn new(
        component_service: Arc<dyn ComponentService<AuthCtx> + Send + Sync>,
        definition_repo: Arc<dyn ApiDefinitionRepo + Sync + Send>,
        deployment_repo: Arc<dyn ApiDeploymentRepo + Sync + Send>,
        security_scheme_service: Arc<dyn SecuritySchemeService<Namespace> + Sync + Send>,
        api_definition_validator: Arc<
            dyn ApiDefinitionValidatorService<HttpApiDefinition> + Sync + Send,
        >,
        config: ApiDefinitionServiceConfig
    ) -> Self {
        Self {
            component_service,
            definition_repo,
            security_scheme_service,
            deployment_repo,
            api_definition_validator,
            component_name_cache: Cache::new(Some(config.component_name_cache_size), FullCacheEvictionMode::None, BackgroundEvictionMode::None, "component_name"),
            component_id_cache: Cache::new(Some(config.component_id_cache_size), FullCacheEvictionMode::None, BackgroundEvictionMode::None, "component_id")
        }
    }

    async fn get_all_components(
        &self,
        definition: &HttpApiDefinition,
        auth_ctx: &AuthCtx,
    ) -> Result<Vec<Component>, ApiDefinitionError> {
        let get_components = definition
            .get_bindings()
            .iter()
            .cloned()
            .filter_map(|binding| binding.get_component_id())
            .map(|id| async move {
                self.component_service
                    .get_by_version(&id.component_id, id.version, auth_ctx)
                    .await
                    .map_err(|e| {
                        error!(
                            error = e.to_string(),
                            component_id = id.to_string(),
                            "Error getting latest component"
                        );
                        id.clone()
                    })
            })
            .collect::<Vec<_>>();

        let components: Vec<Component> = {
            let results = futures::future::join_all(get_components).await;
            let (successes, errors) = results
                .into_iter()
                .partition::<Vec<_>, _>(|result| result.is_ok());

            // Ensure that all components were retrieved.
            if !errors.is_empty() {
                let errors: Vec<VersionedComponentId> =
                    errors.into_iter().map(|r| r.unwrap_err()).collect();
                return Err(ApiDefinitionError::ComponentNotFoundError(errors));
            }

            successes.into_iter().map(|r| r.unwrap()).collect()
        };

        Ok(components)
    }
}

#[async_trait]
impl<AuthCtx, Namespace> ApiDefinitionService<AuthCtx, Namespace>
    for ApiDefinitionServiceDefault<AuthCtx, Namespace>
where
    AuthCtx: Send + Sync,
    Namespace: Display + Clone + Send + Sync + TryFrom<String>,
    <Namespace as TryFrom<String>>::Error: Display,
{
    async fn create(
        &self,
        definition: &HttpApiDefinitionRequest,
        namespace: &Namespace,
        auth_ctx: &AuthCtx,
    ) -> ApiResult<CompiledHttpApiDefinition<Namespace>> {
        info!(namespace = %namespace, "Create API definition");
        let created_at = Utc::now();

        let exists = self
            .definition_repo
            .get_draft(
                namespace.to_string().as_str(),
                definition.id.0.as_str(),
                definition.version.0.as_str(),
            )
            .await?;

        if exists.is_some() {
            return Err(ApiDefinitionError::ApiDefinitionAlreadyExists(
                definition.id.clone(),
                definition.version.clone(),
            ));
        }

        let definition = HttpApiDefinition::from_http_api_definition_request::<Namespace>(
            namespace,
            definition.clone(),
            created_at,
            &self.security_scheme_service,
        )
        .await?;

        let components = self.get_all_components(&definition, auth_ctx).await?;

        self.api_definition_validator
            .validate_name(&definition.id)?;

        self.api_definition_validator
            .validate(&definition, components.as_slice())?;

        let component_metadata_dictionary =
            ComponentMetadataDictionary::from_components(&components);

        let compiled_http_api_definition = CompiledHttpApiDefinition::from_http_api_definition(
            &definition,
            &component_metadata_dictionary,
            namespace,
        )?;

        let record = ApiDefinitionRecord::new(compiled_http_api_definition.clone(), created_at)
            .map_err(|e| {
                ApiDefinitionError::Internal(format!("Failed to create API definition record: {e}"))
            })?;

        self.definition_repo.create(&record).await?;

        Ok(compiled_http_api_definition)
    }

    async fn create_with_oas(
        &self,
        definition: &OpenApiHttpApiDefinition,
        namespace: &Namespace,
        auth_ctx: &AuthCtx,
    ) -> ApiResult<CompiledHttpApiDefinition<Namespace>> {
        let conversion_ctx = self.conversion_context(auth_ctx);
        let converted = definition
            .to_http_api_definition_request(&conversion_ctx)
            .await
            .map_err(ApiDefinitionError::InvalidOasDefinition)?;
        self.create(&converted, namespace, auth_ctx).await
    }

    async fn update(
        &self,
        definition: &HttpApiDefinitionRequest,
        namespace: &Namespace,
        auth_ctx: &AuthCtx,
    ) -> ApiResult<CompiledHttpApiDefinition<Namespace>> {
        info!(namespace = %namespace, "Update API definition");

        let existing_record = self
            .definition_repo
            .get(
                namespace.to_string().as_str(),
                definition.id.0.as_str(),
                definition.version.0.as_str(),
            )
            .await?;

        let created_at = match existing_record {
            None => Err(ApiDefinitionError::ApiDefinitionNotFound(
                definition.id.clone(),
            )),
            Some(record) if !record.draft => Err(ApiDefinitionError::ApiDefinitionNotDraft(
                definition.id.clone(),
            )),
            Some(record) => Ok(record.created_at),
        }?;
        let definition = HttpApiDefinition::from_http_api_definition_request(
            namespace,
            definition.clone(),
            created_at,
            &self.security_scheme_service,
        )
        .await?;

        let components = self.get_all_components(&definition, auth_ctx).await?;

        self.api_definition_validator
            .validate(&definition, components.as_slice())?;

        let component_metadata_dictionary =
            ComponentMetadataDictionary::from_components(&components);

        let compiled_http_api_definition = CompiledHttpApiDefinition::from_http_api_definition(
            &definition,
            &component_metadata_dictionary,
            namespace,
        )?;

        let record = ApiDefinitionRecord::new(compiled_http_api_definition.clone(), created_at)
            .map_err(|e| {
                ApiDefinitionError::Internal(format!("Failed to create API definition record: {e}"))
            })?;

        self.definition_repo.update(&record).await?;

        Ok(compiled_http_api_definition)
    }

    async fn update_with_oas(
        &self,
        definition: &OpenApiHttpApiDefinition,
        namespace: &Namespace,
        auth_ctx: &AuthCtx,
    ) -> ApiResult<CompiledHttpApiDefinition<Namespace>> {
        let conversion_ctx = self.conversion_context(auth_ctx);
        let converted = definition
            .to_http_api_definition_request(&conversion_ctx)
            .await
            .map_err(ApiDefinitionError::InvalidOasDefinition)?;
        self.update(&converted, namespace, auth_ctx).await
    }

    async fn get(
        &self,
        id: &ApiDefinitionId,
        version: &ApiVersion,
        namespace: &Namespace,
        _auth_ctx: &AuthCtx,
    ) -> ApiResult<Option<CompiledHttpApiDefinition<Namespace>>> {
        info!(namespace = %namespace, "Get API definition");
        let value = self
            .definition_repo
            .get(&namespace.to_string(), id.0.as_str(), version.0.as_str())
            .await?;

        match value {
            Some(v) => {
                let definition = v.try_into().map_err(|e| {
                    ApiDefinitionError::Internal(format!(
                        "Failed to convert API definition record: {e}"
                    ))
                })?;
                Ok(Some(definition))
            }
            None => Ok(None),
        }
    }

    async fn delete(
        &self,
        id: &ApiDefinitionId,
        version: &ApiVersion,
        namespace: &Namespace,
        _auth_ctx: &AuthCtx,
    ) -> ApiResult<()> {
        info!(namespace = %namespace, "Delete API definition");

        let deployments = self
            .deployment_repo
            .get_by_id_and_version(&namespace.to_string(), id.0.as_str(), version.0.as_str())
            .await?;

        if deployments.is_empty() {
            let deleted = self
                .definition_repo
                .delete(&namespace.to_string(), id.0.as_str(), version.0.as_str())
                .await?;

            if deleted {
                Ok(())
            } else {
                Err(ApiDefinitionError::ApiDefinitionNotFound(id.clone()))
            }
        } else {
            Err(ApiDefinitionError::ApiDefinitionDeployed(
                deployments
                    .into_iter()
                    .map(|d| d.site)
                    .collect::<Vec<String>>()
                    .join(", "),
            ))
        }
    }

    async fn get_all(
        &self,
        namespace: &Namespace,
        _auth_ctx: &AuthCtx,
    ) -> ApiResult<Vec<CompiledHttpApiDefinition<Namespace>>> {
        info!(namespace = %namespace, "Get all API definitions");
        let records = self.definition_repo.get_all(&namespace.to_string()).await?;

        let values: Vec<CompiledHttpApiDefinition<Namespace>> = records
            .iter()
            .map(|d| d.clone().try_into())
            .collect::<Result<Vec<CompiledHttpApiDefinition<Namespace>>, _>>()
            .map_err(|e| {
                ApiDefinitionError::Internal(format!(
                    "Failed to convert API definition record: {e}"
                ))
            })?;

        Ok(values)
    }

    async fn get_all_versions(
        &self,
        id: &ApiDefinitionId,
        namespace: &Namespace,
        _auth_ctx: &AuthCtx,
    ) -> ApiResult<Vec<CompiledHttpApiDefinition<Namespace>>> {
        info!(namespace = %namespace, "Get all API definitions versions");

        let records = self
            .definition_repo
            .get_all_versions(&namespace.to_string(), id.0.as_str())
            .await?;

        let values: Vec<CompiledHttpApiDefinition<Namespace>> = records
            .iter()
            .map(|d| d.clone().try_into())
            .collect::<Result<Vec<CompiledHttpApiDefinition<Namespace>>, _>>()
            .map_err(|e| {
                ApiDefinitionError::Internal(format!(
                    "Failed to convert API definition record: {e}"
                ))
            })?;

        Ok(values)
    }

    fn conversion_context<'a>(&'a self, auth_ctx: &'a AuthCtx) -> BoxConversionContext<'a>
    where
        AuthCtx: 'a,
    {
        ConversionContextImpl {
            component_service: &self.component_service,
            auth_ctx,
        }
        .boxed()
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use crate::service::gateway::api_definition::ApiDefinitionError;
    use golem_common::SafeDisplay;
    use golem_service_base::repo::RepoError;

    #[test]
    pub fn test_repo_error_to_service_error() {
        let repo_err = RepoError::Internal("some sql error".to_string());
        let service_err: ApiDefinitionError = repo_err.into();
        assert_eq!(
            service_err.to_safe_string(),
            "Internal repository error".to_string()
        );
    }
}
