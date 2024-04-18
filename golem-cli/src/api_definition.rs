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

use async_trait::async_trait;

use crate::clients::api_definition::ApiDefinitionClient;
use crate::model::text::{ApiDefinitionGetRes, ApiDefinitionPostRes};
use crate::model::{
    ApiDefinitionId, ApiDefinitionVersion, GolemError, GolemResult, PathBufOrStdin,
};
use clap::Subcommand;

#[derive(Subcommand, Debug)]
#[command()]
pub enum ApiDefinitionSubcommand {
    /// Lists all api definitions
    #[command()]
    List {},

    /// Creates an api definition
    #[command()]
    Put {
        /// The OAuth file to be used as the api definition
        #[arg(value_hint = clap::ValueHint::FilePath)]
        definition: PathBufOrStdin, // TODO: validate exists
    },

    /// Retrieves metadata about an existing api definition
    #[command()]
    Get {
        /// Api definition id
        #[arg(short, long)]
        id: ApiDefinitionId,

        /// Version of the api definition
        #[arg(short = 'V', long)]
        version: ApiDefinitionVersion,
    },

    /// Deletes an existing api definition
    #[command()]
    Delete {
        /// Api definition id
        #[arg(short, long)]
        id: ApiDefinitionId,

        /// Version of the api definition
        #[arg(short = 'V', long)]
        version: ApiDefinitionVersion,
    },
}

#[async_trait]
pub trait ApiDefinitionHandler {
    async fn handle(&self, subcommand: ApiDefinitionSubcommand) -> Result<GolemResult, GolemError>;
}

pub struct ApiDefinitionHandlerLive<C: ApiDefinitionClient + Send + Sync> {
    pub client: C,
}

#[async_trait]
impl<C: ApiDefinitionClient + Send + Sync> ApiDefinitionHandler for ApiDefinitionHandlerLive<C> {
    async fn handle(&self, subcommand: ApiDefinitionSubcommand) -> Result<GolemResult, GolemError> {
        match subcommand {
            ApiDefinitionSubcommand::Get { id, version } => {
                let definition = self.client.get(id, version).await?;
                Ok(GolemResult::Ok(Box::new(ApiDefinitionGetRes(definition))))
            }
            ApiDefinitionSubcommand::Put { definition } => {
                let definition = self.client.put(definition).await?;
                Ok(GolemResult::Ok(Box::new(ApiDefinitionPostRes(definition))))
            }
            ApiDefinitionSubcommand::List { .. } => {
                let definitions = self.client.all_get().await?;
                Ok(GolemResult::Ok(Box::new(definitions)))
            }
            ApiDefinitionSubcommand::Delete { id, version } => {
                let result = self.client.delete(id, version).await?;
                Ok(GolemResult::Str(result))
            }
        }
    }
}

#[cfg(test)]
mod tests {

    use std::sync::{Arc, Mutex};

    use golem_client::{api::ApiDefinitionError, model::HttpApiDefinition};
    use tonic::async_trait;

    use crate::{
        api_definition::ApiDefinitionSubcommand,
        clients::api_definition::ApiDefinitionClientLive,
        model::{ApiDefinitionId, ApiDefinitionVersion, GolemError},
    };
    use golem_client::Error;

    use super::{ApiDefinitionHandler, ApiDefinitionHandlerLive};

    pub struct ApiDefinitionClientTest {
        calls: Arc<Mutex<String>>,
    }

    #[async_trait]
    impl golem_client::api::ApiDefinitionClient for ApiDefinitionClientTest {
        async fn oas_put(
            &self,
            _: &serde_json::Value,
        ) -> Result<HttpApiDefinition, Error<ApiDefinitionError>> {
            let mut calls = self.calls.lock().unwrap();
            calls.push_str("oas_put");

            Ok(HttpApiDefinition {
                id: "".to_string(),
                version: "".to_string(),
                routes: vec![],
            })
        }

        async fn get(
            &self,
            api_definition_id: &str,
            version: &str,
        ) -> Result<Vec<HttpApiDefinition>, Error<ApiDefinitionError>> {
            let mut calls = self.calls.lock().unwrap();
            calls.push_str(format!("get: {}/{}", api_definition_id, version).as_str());
            Ok(vec![])
        }

        async fn put(
            &self,
            value: &HttpApiDefinition,
        ) -> Result<HttpApiDefinition, Error<ApiDefinitionError>> {
            let mut calls = self.calls.lock().unwrap();
            calls.push_str(format!("put: {:?}", value).as_str());
            Ok(HttpApiDefinition {
                id: "".to_string(),
                version: "".to_string(),
                routes: vec![],
            })
        }

        async fn post(
            &self,
            value: &HttpApiDefinition,
        ) -> Result<HttpApiDefinition, Error<ApiDefinitionError>> {
            let mut calls = self.calls.lock().unwrap();
            calls.push_str(format!("post: {:?}", value).as_str());
            Ok(HttpApiDefinition {
                id: "".to_string(),
                version: "".to_string(),
                routes: vec![],
            })
        }

        async fn delete(
            &self,
            api_definition_id: &str,
            version: &str,
        ) -> Result<String, Error<ApiDefinitionError>> {
            let mut calls = self.calls.lock().unwrap();
            calls.push_str(format!("delete: {}/{}", api_definition_id, version).as_str());
            Ok("deleted".to_string())
        }

        async fn all_get(&self) -> Result<Vec<HttpApiDefinition>, Error<ApiDefinitionError>> {
            let mut calls = self.calls.lock().unwrap();
            calls.push_str("all");
            Ok(vec![])
        }
    }

    async fn handle(subcommand: ApiDefinitionSubcommand) -> Result<String, GolemError> {
        let api_definition_client = ApiDefinitionClientLive {
            client: ApiDefinitionClientTest {
                calls: Arc::new(Mutex::new(String::new())),
            },
        };

        let api_definition_srv = ApiDefinitionHandlerLive {
            client: api_definition_client,
        };
        api_definition_srv.handle(subcommand).await.map(|_| {
            api_definition_srv
                .client
                .client
                .calls
                .lock()
                .unwrap()
                .to_string()
        })
    }

    #[tokio::test]
    pub async fn list() {
        let checked = handle(ApiDefinitionSubcommand::List {}).await;
        if let Ok(calls) = checked {
            assert_eq!(calls, "all");
        }
    }

    #[tokio::test]
    pub async fn get() {
        let subcommand = ApiDefinitionSubcommand::Get {
            id: ApiDefinitionId("id".to_string()),
            version: ApiDefinitionVersion("version".to_string()),
        };
        let checked = handle(subcommand).await;
        if let Ok(calls) = checked {
            assert_eq!(calls, "get: id/version");
        }
    }

    #[tokio::test]
    pub async fn delete() {
        let subcommand = ApiDefinitionSubcommand::Delete {
            id: ApiDefinitionId("id".to_string()),
            version: ApiDefinitionVersion("version".to_string()),
        };
        let checked = handle(subcommand).await;
        if let Ok(calls) = checked {
            assert_eq!(calls, "delete: id/version");
        }
    }
}
