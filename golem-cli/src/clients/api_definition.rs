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

use std::fmt::Display;

use std::io::Read;

use async_trait::async_trait;

use golem_client::model::HttpApiDefinition;

use tokio::fs::read_to_string;
use tracing::info;

use crate::model::{ApiDefinitionId, ApiDefinitionVersion, GolemError, PathBufOrStdin};

#[async_trait]
pub trait ApiDefinitionClient {
    async fn all_get(&self) -> Result<Vec<HttpApiDefinition>, GolemError>;
    async fn get(
        &self,
        id: ApiDefinitionId,
        version: ApiDefinitionVersion,
    ) -> Result<Vec<HttpApiDefinition>, GolemError>;
    async fn create(&self, path: PathBufOrStdin) -> Result<HttpApiDefinition, GolemError>;
    async fn update(&self, path: PathBufOrStdin) -> Result<HttpApiDefinition, GolemError>;
    async fn import(&self, path: PathBufOrStdin) -> Result<HttpApiDefinition, GolemError>;
    async fn delete(
        &self,
        id: ApiDefinitionId,
        version: ApiDefinitionVersion,
    ) -> Result<String, GolemError>;
}

#[derive(Clone)]
pub struct ApiDefinitionClientLive<C: golem_client::api::ApiDefinitionClient + Sync + Send> {
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
    C: golem_client::api::ApiDefinitionClient + Sync + Send,
>(
    action: Action,
    client: &C,
    path: PathBufOrStdin,
) -> Result<HttpApiDefinition, GolemError> {
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

    match action {
        Action::Import => {
            let value: serde_json::value::Value = serde_json::from_str(definition_str.as_str())
                .map_err(|e| GolemError(format!("Failed to parse json: {e:?}")))?;

            Ok(client.oas_put(&value).await?)
        }
        Action::Create => {
            let value: HttpApiDefinition = serde_json::from_str(definition_str.as_str())
                .map_err(|e| GolemError(format!("Failed to parse HttpApiDefinition: {e:?}")))?;

            Ok(client.post(&value).await?)
        }
        Action::Update => {
            let value: HttpApiDefinition = serde_json::from_str(definition_str.as_str())
                .map_err(|e| GolemError(format!("Failed to parse HttpApiDefinition: {e:?}")))?;

            Ok(client.put(&value).await?)
        }
    }
}

#[async_trait]
impl<C: golem_client::api::ApiDefinitionClient + Sync + Send> ApiDefinitionClient
    for ApiDefinitionClientLive<C>
{
    async fn all_get(&self) -> Result<Vec<HttpApiDefinition>, GolemError> {
        info!("Getting api definitions");

        Ok(self.client.all_get().await?)
    }

    async fn get(
        &self,
        id: ApiDefinitionId,
        version: ApiDefinitionVersion,
    ) -> Result<Vec<HttpApiDefinition>, GolemError> {
        info!("Getting api definition for {}/{}", id.0, version.0);

        Ok(self.client.get(id.0.as_str(), version.0.as_str()).await?)
    }

    async fn create(&self, path: PathBufOrStdin) -> Result<HttpApiDefinition, GolemError> {
        create_or_update_api_definition(Action::Create, &self.client, path).await
    }

    async fn update(&self, path: PathBufOrStdin) -> Result<HttpApiDefinition, GolemError> {
        create_or_update_api_definition(Action::Update, &self.client, path).await
    }

    async fn import(&self, path: PathBufOrStdin) -> Result<HttpApiDefinition, GolemError> {
        create_or_update_api_definition(Action::Import, &self.client, path).await
    }

    async fn delete(
        &self,
        id: ApiDefinitionId,
        version: ApiDefinitionVersion,
    ) -> Result<String, GolemError> {
        info!("Deleting api definition for {}/{}", id.0, version.0);
        Ok(self
            .client
            .delete(id.0.as_str(), version.0.as_str())
            .await?)
    }
}
