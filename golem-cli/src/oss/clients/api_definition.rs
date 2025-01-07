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

use std::fmt::Display;

use std::io::Read;

use async_trait::async_trait;
use golem_client::model::{HttpApiDefinitionRequest, HttpApiDefinitionResponseData};
use golem_client::api::ApiDefinitionClient as GolemApiDefinitionClient;
use tokio::fs::read_to_string;
use tracing::info;
use golem_worker_service_base::gateway_api_definition::http::rib_converter::RibConverter;
use poem_openapi::registry::Registry;

use crate::model::{
    decode_api_definition, ApiDefinitionFileFormat, ApiDefinitionId, ApiDefinitionVersion,
    GolemError, PathBufOrStdin,
};
use crate::oss::model::OssContext;

#[derive(Clone)]
pub struct ApiDefinitionClientLive<C: GolemApiDefinitionClient + Sync + Send> {
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
    C: GolemApiDefinitionClient + Sync + Send,
>(
    action: Action,
    client: &C,
    path: PathBufOrStdin,
    format: &ApiDefinitionFileFormat,
) -> Result<HttpApiDefinitionResponseData, GolemError> {
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
            let value = decode_api_definition(definition_str.as_str(), format)?;
            Ok(client.import_open_api_json(&value).await?)
        }
        Action::Create => {
            let value: HttpApiDefinitionRequest =
                decode_api_definition(definition_str.as_str(), format)?;
            Ok(client.create_definition_json(&value).await?)
        }
        Action::Update => {
            let value: HttpApiDefinitionRequest =
                decode_api_definition(definition_str.as_str(), format)?;
            Ok(client
                .update_definition_json(&value.id, &value.version, &value)
                .await?)
        }
    }
}

#[async_trait]
impl crate::clients::api_definition::ApiDefinitionClient for ApiDefinitionClientLive<golem_client::api::ApiDefinitionClientLive> {
    type ProjectContext = OssContext;

    async fn get_swagger_url(
        &self,
        id: ApiDefinitionId,
        version: ApiDefinitionVersion,
        _project: &Self::ProjectContext,
    ) -> Result<String, GolemError> {
        let id_str = id.0.as_str();
        let version_str = version.0.as_str();
        // Get the definition first to ensure it exists
        let _definition = self.client.get_definition(id_str, version_str).await?;
        // Construct the Swagger UI URL from the worker service base URL
        let base_url = self.client.context.base_url.clone();
        Ok(format!("{}/swagger-ui/api-definitions/{}/{}", base_url, id_str, version_str))
    }

    async fn export_schema(
        &self,
        id: ApiDefinitionId,
        version: ApiDefinitionVersion,
        _project: &Self::ProjectContext,
        format: ApiDefinitionFileFormat,
    ) -> Result<String, GolemError> {
        let id_str = id.0.as_str();
        let version_str = version.0.as_str();
        // Get the definition first
        let definition = self.client.get_definition(id_str, version_str).await?;
        
        // Create a new RibConverter in OpenAPI mode
        let mut converter = RibConverter::new_openapi();
        let registry = Registry::new();

        // Convert each route to OpenAPI schema
        let mut openapi = serde_json::json!({
            "openapi": "3.0.0",
            "info": {
                "title": format!("API Definition {}", id_str),
                "version": version_str,
            },
            "paths": {}
        });

        let paths = openapi.as_object_mut().unwrap().get_mut("paths").unwrap().as_object_mut().unwrap();

        // Convert each route to an OpenAPI path
        for route in definition.routes {
            let mut path_obj = serde_json::Map::new();
            let method = route.method.to_string().to_lowercase();

            // Convert request and response types using RibConverter
            let mut operation = serde_json::Map::new();
            operation.insert("summary".to_string(), serde_json::Value::String(route.path.clone()));
            operation.insert("operationId".to_string(), serde_json::Value::String(format!("{}_{}", method, route.path.replace("/", "_"))));

            // Add responses
            let mut responses = serde_json::Map::new();
            let mut ok_response = serde_json::Map::new();
            ok_response.insert("description".to_string(), serde_json::Value::String("Successful response".to_string()));

            // If we have response types in the binding, convert them
            if let Some(response_mapping_output) = &route.binding.response_mapping_output {
                if let Ok(schema_ref) = converter.convert_type(&response_mapping_output.analysed_type, &registry) {
                    ok_response.insert("content".to_string(), serde_json::json!({
                        "application/json": {
                            "schema": schema_ref
                        }
                    }));
                }
            }

            responses.insert("200".to_string(), serde_json::Value::Object(ok_response));
            operation.insert("responses".to_string(), serde_json::Value::Object(responses));

            // Add parameters if we have them in the binding
            if let Some(params) = &route.binding.response_mapping_input {
                if let Some(request_type) = params.types.get("request") {
                    if let Ok(schema_ref) = converter.convert_type(request_type, &registry) {
                        operation.insert("parameters".to_string(), serde_json::json!([{
                            "in": "query",
                            "name": "params",
                            "schema": schema_ref
                        }]));
                    }
                }
            }

            path_obj.insert(method, serde_json::Value::Object(operation));
            paths.insert(route.path, serde_json::Value::Object(path_obj));
        }

        // Fix any additionalProperties in the schema
        golem_worker_service_base::gateway_api_definition::http::rib_converter::fix_additional_properties(&mut openapi);

        // Convert to the requested format
        match format {
            ApiDefinitionFileFormat::Json => {
                serde_json::to_string_pretty(&openapi)
                    .map_err(|e| GolemError(format!("Failed to serialize schema to JSON: {}", e)))
            }
            ApiDefinitionFileFormat::Yaml => {
                serde_yaml::to_string(&openapi)
                    .map_err(|e| GolemError(format!("Failed to serialize schema to YAML: {}", e)))
            }
        }
    }

    async fn list(
        &self,
        id: Option<&ApiDefinitionId>,
        _project: &Self::ProjectContext,
    ) -> Result<Vec<HttpApiDefinitionResponseData>, GolemError> {
        Ok(self.client.list_definitions(id.map(|id| id.0.as_str())).await?)
    }

    async fn get(
        &self,
        id: ApiDefinitionId,
        version: ApiDefinitionVersion,
        _project: &Self::ProjectContext,
    ) -> Result<HttpApiDefinitionResponseData, GolemError> {
        let id_str = id.0.as_str();
        let version_str = version.0.as_str();
        Ok(self.client.get_definition(id_str, version_str).await?)
    }

    async fn create(
        &self,
        path: PathBufOrStdin,
        _project: &Self::ProjectContext,
        format: &ApiDefinitionFileFormat,
    ) -> Result<HttpApiDefinitionResponseData, GolemError> {
        create_or_update_api_definition(Action::Create, &self.client, path, format).await
    }

    async fn update(
        &self,
        path: PathBufOrStdin,
        _project: &Self::ProjectContext,
        format: &ApiDefinitionFileFormat,
    ) -> Result<HttpApiDefinitionResponseData, GolemError> {
        create_or_update_api_definition(Action::Update, &self.client, path, format).await
    }

    async fn import(
        &self,
        path: PathBufOrStdin,
        _project: &Self::ProjectContext,
        format: &ApiDefinitionFileFormat,
    ) -> Result<HttpApiDefinitionResponseData, GolemError> {
        create_or_update_api_definition(Action::Import, &self.client, path, format).await
    }

    async fn delete(
        &self,
        id: ApiDefinitionId,
        version: ApiDefinitionVersion,
        _project: &Self::ProjectContext,
    ) -> Result<String, GolemError> {
        let id_str = id.0.as_str();
        let version_str = version.0.as_str();
        Ok(self.client.delete_definition(id_str, version_str).await?)
    }
}
