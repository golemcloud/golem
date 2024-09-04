// Copyright 2024 Golem Cloud
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

use std::fmt::{Debug, Display};
use std::hash::Hash;
use std::sync::Arc;

use crate::api_definition::http::{
    CompiledHttpApiDefinition, ComponentMetadataDictionary, HttpApiDefinition,
    HttpApiDefinitionRequest, RouteCompilationErrors,
};
use async_trait::async_trait;
use chrono::Utc;
use golem_service_base::model::{Component, VersionedComponentId};
use tracing::{error, info};

use crate::api_definition::{ApiDefinitionId, ApiVersion, HasGolemWorkerBindings};
use crate::repo::api_definition::ApiDefinitionRecord;
use crate::repo::api_definition::ApiDefinitionRepo;
use crate::repo::api_deployment::ApiDeploymentRepo;
use crate::repo::RepoError;

use super::api_definition_validator::{ApiDefinitionValidatorService, ValidationErrors};
use super::component::ComponentService;

pub type ApiResult<T, E> = Result<T, ApiDefinitionError<E>>;

#[derive(
    Eq, Hash, PartialEq, Clone, Debug, serde::Deserialize, bincode::Encode, bincode::Decode,
)]
pub struct ApiDefinitionIdWithVersion {
    pub id: ApiDefinitionId,
    pub version: ApiVersion,
}

#[derive(Debug, thiserror::Error)]
pub enum ApiDefinitionError<E> {
    #[error(transparent)]
    ValidationError(#[from] ValidationErrors<E>),
    #[error("Unable to fetch component: {}", .0.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(", "))]
    ComponentNotFoundError(Vec<VersionedComponentId>),
    #[error("Rib compilation error: {0}")]
    RibCompilationErrors(String),
    #[error("API definition not found: {0}")]
    ApiDefinitionNotFound(ApiDefinitionId),
    #[error("API definition is not draft: {0}")]
    ApiDefinitionNotDraft(ApiDefinitionId),
    #[error("API definition already exists: {0}")]
    ApiDefinitionAlreadyExists(ApiDefinitionId),
    #[error("API definition deployed: {0}")]
    ApiDefinitionDeployed(String),
    #[error("Internal error: {0}")]
    InternalError(#[from] anyhow::Error),
}

impl<T> ApiDefinitionError<T> {
    fn internal<E, C>(error: E, context: C) -> Self
    where
        E: Display + Debug + Send + Sync + 'static,
        C: Display + Send + Sync + 'static,
    {
        ApiDefinitionError::InternalError(anyhow::Error::msg(error).context(context))
    }
}

impl<E> From<RepoError> for ApiDefinitionError<E> {
    fn from(error: RepoError) -> Self {
        ApiDefinitionError::internal(error, "Repository error")
    }
}

impl<E> From<RouteCompilationErrors> for ApiDefinitionError<E> {
    fn from(error: RouteCompilationErrors) -> Self {
        match error {
            RouteCompilationErrors::RibCompilationError(e) => {
                ApiDefinitionError::RibCompilationErrors(e)
            }
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
pub trait ApiDefinitionService<AuthCtx, Namespace, ValidationError> {
    async fn create(
        &self,
        definition: &HttpApiDefinitionRequest,
        namespace: &Namespace,
        auth_ctx: &AuthCtx,
    ) -> ApiResult<CompiledHttpApiDefinition, ValidationError>;

    async fn update(
        &self,
        definition: &HttpApiDefinitionRequest,
        namespace: &Namespace,
        auth_ctx: &AuthCtx,
    ) -> ApiResult<CompiledHttpApiDefinition, ValidationError>;

    async fn get(
        &self,
        id: &ApiDefinitionId,
        version: &ApiVersion,
        namespace: &Namespace,
        auth_ctx: &AuthCtx,
    ) -> ApiResult<Option<CompiledHttpApiDefinition>, ValidationError>;

    async fn delete(
        &self,
        id: &ApiDefinitionId,
        version: &ApiVersion,
        namespace: &Namespace,
        auth_ctx: &AuthCtx,
    ) -> ApiResult<Option<ApiDefinitionId>, ValidationError>;

    async fn get_all(
        &self,
        namespace: &Namespace,
        auth_ctx: &AuthCtx,
    ) -> ApiResult<Vec<CompiledHttpApiDefinition>, ValidationError>;

    async fn get_all_versions(
        &self,
        id: &ApiDefinitionId,
        namespace: &Namespace,
        auth_ctx: &AuthCtx,
    ) -> ApiResult<Vec<CompiledHttpApiDefinition>, ValidationError>;
}

pub struct ApiDefinitionServiceDefault<AuthCtx, ValidationError> {
    pub component_service: Arc<dyn ComponentService<AuthCtx> + Send + Sync>,
    pub definition_repo: Arc<dyn ApiDefinitionRepo + Sync + Send>,
    pub deployment_repo: Arc<dyn ApiDeploymentRepo + Sync + Send>,
    pub api_definition_validator:
        Arc<dyn ApiDefinitionValidatorService<HttpApiDefinition, ValidationError> + Sync + Send>,
}

impl<AuthCtx, ValidationError> ApiDefinitionServiceDefault<AuthCtx, ValidationError> {
    pub fn new(
        component_service: Arc<dyn ComponentService<AuthCtx> + Send + Sync>,
        definition_repo: Arc<dyn ApiDefinitionRepo + Sync + Send>,
        deployment_repo: Arc<dyn ApiDeploymentRepo + Sync + Send>,
        api_definition_validator: Arc<
            dyn ApiDefinitionValidatorService<HttpApiDefinition, ValidationError> + Sync + Send,
        >,
    ) -> Self {
        Self {
            component_service,
            definition_repo,
            deployment_repo,
            api_definition_validator,
        }
    }

    async fn get_all_components(
        &self,
        definition: &HttpApiDefinition,
        auth_ctx: &AuthCtx,
    ) -> Result<Vec<Component>, ApiDefinitionError<ValidationError>> {
        let get_components = definition
            .get_golem_worker_bindings()
            .iter()
            .cloned()
            .map(|binding| async move {
                let id = &binding.component_id;
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
impl<AuthCtx, Namespace, ValidationError> ApiDefinitionService<AuthCtx, Namespace, ValidationError>
    for ApiDefinitionServiceDefault<AuthCtx, ValidationError>
where
    AuthCtx: Send + Sync,
    Namespace: Display + Clone + Send + Sync,
{
    async fn create(
        &self,
        definition: &HttpApiDefinitionRequest,
        namespace: &Namespace,
        auth_ctx: &AuthCtx,
    ) -> ApiResult<CompiledHttpApiDefinition, ValidationError> {
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
            ));
        }

        let definition = HttpApiDefinition::new(definition.clone(), created_at);

        let components = self.get_all_components(&definition, auth_ctx).await?;

        self.api_definition_validator
            .validate(&definition, components.as_slice())?;

        let component_metadata_dictionary =
            ComponentMetadataDictionary::from_components(&components);

        let compiled_http_api_definition = CompiledHttpApiDefinition::from_http_api_definition(
            &definition,
            &component_metadata_dictionary,
        )?;

        let record = ApiDefinitionRecord::new(
            namespace.clone(),
            compiled_http_api_definition.clone(),
            created_at,
        )
        .map_err(|e| ApiDefinitionError::internal(e, "Failed to convert record"))?;

        self.definition_repo.create(&record).await?;

        Ok(compiled_http_api_definition)
    }

    async fn update(
        &self,
        definition: &HttpApiDefinitionRequest,
        namespace: &Namespace,
        auth_ctx: &AuthCtx,
    ) -> ApiResult<CompiledHttpApiDefinition, ValidationError> {
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
        let definition = HttpApiDefinition::new(definition.clone(), created_at);

        let components = self.get_all_components(&definition, auth_ctx).await?;

        self.api_definition_validator
            .validate(&definition, components.as_slice())?;

        let component_metadata_dictionary =
            ComponentMetadataDictionary::from_components(&components);

        let compiled_http_api_definition = CompiledHttpApiDefinition::from_http_api_definition(
            &definition,
            &component_metadata_dictionary,
        )?;

        let record = ApiDefinitionRecord::new(
            namespace.clone(),
            compiled_http_api_definition.clone(),
            created_at,
        )
        .map_err(|e| ApiDefinitionError::internal(e, "Failed to convert record"))?;

        self.definition_repo.update(&record).await?;

        Ok(compiled_http_api_definition)
    }

    async fn get(
        &self,
        id: &ApiDefinitionId,
        version: &ApiVersion,
        namespace: &Namespace,
        _auth_ctx: &AuthCtx,
    ) -> ApiResult<Option<CompiledHttpApiDefinition>, ValidationError> {
        info!(namespace = %namespace, "Get API definition");
        let value = self
            .definition_repo
            .get(&namespace.to_string(), id.0.as_str(), version.0.as_str())
            .await?;

        match value {
            Some(v) => {
                let definition = v
                    .try_into()
                    .map_err(|e| ApiDefinitionError::internal(e, "Failed to convert record"))?;
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
    ) -> ApiResult<Option<ApiDefinitionId>, ValidationError> {
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

            let value = if deleted { Some(id.clone()) } else { None };

            Ok(value)
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
    ) -> ApiResult<Vec<CompiledHttpApiDefinition>, ValidationError> {
        info!(namespace = %namespace, "Get all API definitions");
        let records = self.definition_repo.get_all(&namespace.to_string()).await?;

        let values: Vec<CompiledHttpApiDefinition> = records
            .iter()
            .map(|d| d.clone().try_into())
            .collect::<Result<Vec<CompiledHttpApiDefinition>, _>>()
            .map_err(|e| ApiDefinitionError::internal(e, "Failed to convert record"))?;

        Ok(values)
    }

    async fn get_all_versions(
        &self,
        id: &ApiDefinitionId,
        namespace: &Namespace,
        _auth_ctx: &AuthCtx,
    ) -> ApiResult<Vec<CompiledHttpApiDefinition>, ValidationError> {
        info!(namespace = %namespace, "Get all API definitions versions");

        let records = self
            .definition_repo
            .get_all_versions(&namespace.to_string(), id.0.as_str())
            .await?;

        let values: Vec<CompiledHttpApiDefinition> = records
            .iter()
            .map(|d| d.clone().try_into())
            .collect::<Result<Vec<CompiledHttpApiDefinition>, _>>()
            .map_err(|e| ApiDefinitionError::internal(e, "Failed to convert record"))?;

        Ok(values)
    }
}

#[derive(Default)]
pub struct ApiDefinitionServiceNoop {}

#[async_trait]
impl<AuthCtx, Namespace, ValidationError> ApiDefinitionService<AuthCtx, Namespace, ValidationError>
    for ApiDefinitionServiceNoop
where
    AuthCtx: Send + Sync,
    Namespace: Display + Clone + Send + Sync,
{
    async fn create(
        &self,
        definition: &HttpApiDefinitionRequest,
        _namespace: &Namespace,
        _auth_ctx: &AuthCtx,
    ) -> ApiResult<CompiledHttpApiDefinition, ValidationError> {
        Ok(CompiledHttpApiDefinition {
            id: definition.id.clone(),
            version: definition.version.clone(),
            routes: vec![],
            draft: definition.draft,
            created_at: Utc::now(),
        })
    }

    async fn update(
        &self,
        definition: &HttpApiDefinitionRequest,
        _namespace: &Namespace,
        _auth_ctx: &AuthCtx,
    ) -> ApiResult<CompiledHttpApiDefinition, ValidationError> {
        Ok(CompiledHttpApiDefinition {
            id: definition.id.clone(),
            version: definition.version.clone(),
            routes: vec![],
            draft: definition.draft,
            created_at: Utc::now(),
        })
    }

    async fn get(
        &self,
        _id: &ApiDefinitionId,
        _version: &ApiVersion,
        _namespace: &Namespace,
        _auth_ctx: &AuthCtx,
    ) -> ApiResult<Option<CompiledHttpApiDefinition>, ValidationError> {
        Ok(None)
    }

    async fn delete(
        &self,
        _id: &ApiDefinitionId,
        _version: &ApiVersion,
        _namespace: &Namespace,
        _auth_ctx: &AuthCtx,
    ) -> ApiResult<Option<ApiDefinitionId>, ValidationError> {
        Ok(None)
    }

    async fn get_all(
        &self,
        _namespace: &Namespace,
        _auth_ctx: &AuthCtx,
    ) -> ApiResult<Vec<CompiledHttpApiDefinition>, ValidationError> {
        Ok(vec![])
    }

    async fn get_all_versions(
        &self,
        _id: &ApiDefinitionId,
        _namespace: &Namespace,
        _auth_ctx: &AuthCtx,
    ) -> ApiResult<Vec<CompiledHttpApiDefinition>, ValidationError> {
        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    use crate::repo::RepoError;
    use crate::service::api_definition::ApiDefinitionError;

    #[test]
    pub fn test_repo_error_to_service_error() {
        let repo_err = RepoError::Internal("some sql error".to_string());
        let service_err: ApiDefinitionError<String> = repo_err.into();
        assert_eq!(
            service_err.to_string(),
            "Internal error: Repository error".to_string()
        );
    }
}
