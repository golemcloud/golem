use std::fmt::Display;

use std::io::Read;

use async_trait::async_trait;

use golem_cloud_client::model::{GolemWorkerBinding, HttpApiDefinition, MethodPattern, Route};

use golem_cli::clients::api_definition::ApiDefinitionClient;
use golem_cli::cloud::ProjectId;
use tokio::fs::read_to_string;
use tracing::info;

use crate::cloud::clients::errors::CloudGolemError;
use golem_cli::model::{ApiDefinitionId, ApiDefinitionVersion, GolemError, PathBufOrStdin};

#[derive(Clone)]
pub struct ApiDefinitionClientLive<C: golem_cloud_client::api::ApiDefinitionClient + Sync + Send> {
    pub client: C,
}

#[derive(Debug, Copy, Clone)]
enum Action {
    Create,
    Update,
    Import,
}

impl Display for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let str = match self {
            Action::Create => "Creating",
            Action::Update => "Updating",
            Action::Import => "Importing",
        };
        write!(f, "{}", str)
    }
}

async fn create_or_update_api_definition<
    C: golem_cloud_client::api::ApiDefinitionClient + Sync + Send,
>(
    action: Action,
    client: &C,
    path: PathBufOrStdin,
    project_id: &ProjectId,
) -> Result<golem_client::model::HttpApiDefinition, GolemError> {
    info!("{action} api definition from {path:?}");

    let definition_str: String = match path {
        PathBufOrStdin::Path(path) => read_to_string(path)
            .await
            .map_err(|e| GolemError(format!("Failed to read from file: {e:?}")))?,
        PathBufOrStdin::Stdin => {
            let mut content = String::new();

            let _ = std::io::stdin()
                .read_to_string(&mut content)
                .map_err(|e| GolemError(format!("Failed to read stdin: {e:?}")))?;

            content
        }
    };

    let res = match action {
        Action::Import => {
            let value: serde_json::value::Value = serde_json::from_str(definition_str.as_str())
                .map_err(|e| GolemError(format!("Failed to parse json: {e:?}")))?;

            client
                .import_open_api(&project_id.0, &value)
                .await
                .map_err(CloudGolemError::from)?
        }
        Action::Create => {
            let value: HttpApiDefinition = serde_json::from_str(definition_str.as_str())
                .map_err(|e| GolemError(format!("Failed to parse HttpApiDefinition: {e:?}")))?;

            client
                .create_definition(&project_id.0, &value)
                .await
                .map_err(CloudGolemError::from)?
        }
        Action::Update => {
            let value: HttpApiDefinition = serde_json::from_str(definition_str.as_str())
                .map_err(|e| GolemError(format!("Failed to parse HttpApiDefinition: {e:?}")))?;

            client
                .update_definition(&project_id.0, &value.id, &value.version, &value)
                .await
                .map_err(CloudGolemError::from)?
        }
    };

    Ok(to_oss_http_api_definition(res))
}

fn to_oss_method_pattern(p: MethodPattern) -> golem_client::model::MethodPattern {
    match p {
        MethodPattern::Get => golem_client::model::MethodPattern::Get,
        MethodPattern::Connect => golem_client::model::MethodPattern::Connect,
        MethodPattern::Post => golem_client::model::MethodPattern::Post,
        MethodPattern::Delete => golem_client::model::MethodPattern::Delete,
        MethodPattern::Put => golem_client::model::MethodPattern::Put,
        MethodPattern::Patch => golem_client::model::MethodPattern::Patch,
        MethodPattern::Options => golem_client::model::MethodPattern::Options,
        MethodPattern::Trace => golem_client::model::MethodPattern::Trace,
        MethodPattern::Head => golem_client::model::MethodPattern::Head,
    }
}

fn to_oss_golem_worker_binding(b: GolemWorkerBinding) -> golem_client::model::GolemWorkerBinding {
    let GolemWorkerBinding {
        component_id,
        worker_name,
        idempotency_key,
        response,
    } = b;

    golem_client::model::GolemWorkerBinding {
        component_id: golem_client::model::VersionedComponentId {
            component_id: component_id.component_id,
            version: component_id.version,
        },
        worker_name,
        idempotency_key,
        response,
    }
}

fn to_oss_route(r: Route) -> golem_client::model::Route {
    let Route {
        method,
        path,
        binding,
    } = r;

    golem_client::model::Route {
        method: to_oss_method_pattern(method),
        path,
        binding: to_oss_golem_worker_binding(binding),
    }
}

fn to_oss_http_api_definition(d: HttpApiDefinition) -> golem_client::model::HttpApiDefinition {
    let HttpApiDefinition {
        id,
        version,
        routes,
        draft,
    } = d;

    golem_client::model::HttpApiDefinition {
        id,
        version,
        routes: routes.into_iter().map(to_oss_route).collect(),
        draft,
    }
}

#[async_trait]
impl<C: golem_cloud_client::api::ApiDefinitionClient + Sync + Send> ApiDefinitionClient
    for ApiDefinitionClientLive<C>
{
    type ProjectContext = ProjectId;

    async fn list(
        &self,
        id: Option<&ApiDefinitionId>,
        project: &Self::ProjectContext,
    ) -> Result<Vec<golem_client::model::HttpApiDefinition>, GolemError> {
        info!("Getting api definitions");

        let definitions = self
            .client
            .list_definitions(&project.0, id.map(|id| id.0.as_str()))
            .await
            .map_err(CloudGolemError::from)?;

        Ok(definitions
            .into_iter()
            .map(to_oss_http_api_definition)
            .collect())
    }

    async fn get(
        &self,
        id: ApiDefinitionId,
        version: ApiDefinitionVersion,
        project: &Self::ProjectContext,
    ) -> Result<golem_client::model::HttpApiDefinition, GolemError> {
        info!("Getting api definition for {}/{}", id.0, version.0);

        let definition = self
            .client
            .get_definition(&project.0, id.0.as_str(), version.0.as_str())
            .await
            .map_err(CloudGolemError::from)?;

        Ok(to_oss_http_api_definition(definition))
    }

    async fn create(
        &self,
        path: PathBufOrStdin,
        project: &Self::ProjectContext,
    ) -> Result<golem_client::model::HttpApiDefinition, GolemError> {
        create_or_update_api_definition(Action::Create, &self.client, path, project).await
    }

    async fn update(
        &self,
        path: PathBufOrStdin,
        project: &Self::ProjectContext,
    ) -> Result<golem_client::model::HttpApiDefinition, GolemError> {
        create_or_update_api_definition(Action::Update, &self.client, path, project).await
    }

    async fn import(
        &self,
        path: PathBufOrStdin,
        project: &Self::ProjectContext,
    ) -> Result<golem_client::model::HttpApiDefinition, GolemError> {
        create_or_update_api_definition(Action::Import, &self.client, path, project).await
    }

    async fn delete(
        &self,
        id: ApiDefinitionId,
        version: ApiDefinitionVersion,
        project: &Self::ProjectContext,
    ) -> Result<String, GolemError> {
        info!("Deleting api definition for {}/{}", id.0, version.0);
        Ok(self
            .client
            .delete_definition(&project.0, id.0.as_str(), version.0.as_str())
            .await
            .map_err(CloudGolemError::from)?)
    }
}
