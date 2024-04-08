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
    async fn put(&self, path: PathBufOrStdin) -> Result<HttpApiDefinition, GolemError>;
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

    async fn put(&self, path: PathBufOrStdin) -> Result<HttpApiDefinition, GolemError> {
        info!("Creating api definition from {path:?}");

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

        let value: serde_json::Value = serde_json::from_str(definition_str.as_str())
            .map_err(|e| GolemError(format!("Failed to parse json: {e:?}")))?;

        Ok(self.client.oas_put(&value).await?)
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
