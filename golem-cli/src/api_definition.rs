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
use crate::model::text::{
    ApiDefinitionAddRes, ApiDefinitionGetRes, ApiDefinitionImportRes, ApiDefinitionUpdateRes,
};
use crate::model::{
    ApiDefinitionId, ApiDefinitionVersion, GolemError, GolemResult, PathBufOrStdin,
};
use clap::Subcommand;

#[derive(Subcommand, Debug)]
#[command()]
pub enum ApiDefinitionSubcommand {
    /// Lists all api definitions
    #[command()]
    List {
        /// Api definition id to get all versions. Optional.
        #[arg(short, long)]
        id: Option<ApiDefinitionId>,
    },

    /// Creates an api definition
    ///
    /// Golem API definition file format expected
    #[command()]
    Add {
        /// The Golem API definition file
        #[arg(value_hint = clap::ValueHint::FilePath)]
        definition: PathBufOrStdin, // TODO: validate exists
    },

    /// Updates an api definition
    ///
    /// Golem API definition file format expected
    #[command()]
    Update {
        /// The Golem API definition file
        #[arg(value_hint = clap::ValueHint::FilePath)]
        definition: PathBufOrStdin, // TODO: validate exists
    },

    /// Import OpenAPI file as api definition
    #[command()]
    Import {
        /// The OpenAPI json or yaml file to be used as the api definition
        ///
        /// Json format expected unless file name ends up in `.yaml`
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
            ApiDefinitionSubcommand::Add { definition } => {
                let definition = self.client.create(definition).await?;
                Ok(GolemResult::Ok(Box::new(ApiDefinitionAddRes(definition))))
            }
            ApiDefinitionSubcommand::Update { definition } => {
                let definition = self.client.update(definition).await?;
                Ok(GolemResult::Ok(Box::new(ApiDefinitionUpdateRes(
                    definition,
                ))))
            }
            ApiDefinitionSubcommand::Import { definition } => {
                let definition = self.client.import(definition).await?;
                Ok(GolemResult::Ok(Box::new(ApiDefinitionImportRes(
                    definition,
                ))))
            }
            ApiDefinitionSubcommand::List { id } => {
                let definitions = self.client.list(id.as_ref()).await?;
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
        async fn import_open_api(
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

        async fn get_definition(
            &self,
            api_definition_id: &str,
            version: &str,
        ) -> Result<HttpApiDefinition, Error<ApiDefinitionError>> {
            let mut calls = self.calls.lock().unwrap();
            calls.push_str(format!("get: {}/{}", api_definition_id, version).as_str());
            Ok(HttpApiDefinition {
                id: "".to_string(),
                version: "".to_string(),
                routes: vec![],
            })
        }

        async fn update_definition(
            &self,
            _id: &str,
            _version: &str,
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

        async fn create_definition(
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

        async fn delete_definition(
            &self,
            api_definition_id: &str,
            version: &str,
        ) -> Result<String, Error<ApiDefinitionError>> {
            let mut calls = self.calls.lock().unwrap();
            calls.push_str(format!("delete: {}/{}", api_definition_id, version).as_str());
            Ok("deleted".to_string())
        }

        async fn list_definitions(
            &self,
            _id: Option<&str>,
        ) -> Result<Vec<HttpApiDefinition>, Error<ApiDefinitionError>> {
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
        let checked = handle(ApiDefinitionSubcommand::List { id: None }).await;
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
