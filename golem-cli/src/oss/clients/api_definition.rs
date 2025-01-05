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
use std::net::SocketAddr;

use async_trait::async_trait;
use golem_client::model::{HttpApiDefinitionRequest, HttpApiDefinitionResponseData};
use golem_worker_service_base::gateway_api_definition::http::{
    openapi_export::{OpenApiExporter, OpenApiFormat},
    swagger_ui::{create_api_route, SwaggerUiConfig},
};
use poem::listener::TcpListener;
use poem_openapi::{
    OpenApi,
    Tags,
    payload::Json,
};
use serde_json::Value;
use tokio::fs::{read_to_string, write};
use tracing::info;

use crate::clients::api_definition::ApiDefinitionClient;
use crate::model::{
    decode_api_definition, ApiDefinitionFileFormat, ApiDefinitionId, ApiDefinitionVersion,
    GolemError, PathBufOrStdin,
};
use crate::oss::model::OssContext;

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

#[derive(Tags)]
enum ApiTags {
    /// API Definition operations
    ApiDefinition,
}

#[derive(Clone)]
struct ApiSpec(Value);

#[OpenApi]
impl ApiSpec {
    /// Get OpenAPI specification
    #[oai(path = "/openapi", method = "get", tag = "ApiTags::ApiDefinition")]
    async fn get_openapi(&self) -> Json<Value> {
        Json(self.0.clone())
    }
}

impl<C: golem_client::api::ApiDefinitionClient + Sync + Send> ApiDefinitionClientLive<C> {
    async fn export_openapi(
        &self,
        api_def: &HttpApiDefinitionResponseData,
        format: &ApiDefinitionFileFormat,
    ) -> Result<String, GolemError> {
        // First convert to JSON Value
        let api_value = serde_json::to_value(api_def)
            .map_err(|e| GolemError(format!("Failed to convert API definition to JSON: {}", e)))?;

        // Create OpenAPI exporter
        let exporter = OpenApiExporter;
        let openapi_format = OpenApiFormat {
            json: matches!(format, ApiDefinitionFileFormat::Json),
        };

        // Export using the exporter - pass ApiSpec directly, not as a reference
        Ok(exporter.export_openapi(ApiSpec(api_value), &openapi_format))
    }
}

#[async_trait]
impl<C: golem_client::api::ApiDefinitionClient + Sync + Send> ApiDefinitionClient
    for ApiDefinitionClientLive<C>
{
    type ProjectContext = OssContext;

    async fn list(
        &self,
        id: Option<&ApiDefinitionId>,
        _project: &Self::ProjectContext,
    ) -> Result<Vec<HttpApiDefinitionResponseData>, GolemError> {
        info!("Getting api definitions");

        Ok(self
            .client
            .list_definitions(id.map(|id| id.0.as_str()))
            .await?)
    }

    async fn get(
        &self,
        id: ApiDefinitionId,
        version: ApiDefinitionVersion,
        _project: &Self::ProjectContext,
    ) -> Result<HttpApiDefinitionResponseData, GolemError> {
        info!("Getting api definition for {}/{}", id.0, version.0);

        Ok(self
            .client
            .get_definition(id.0.as_str(), version.0.as_str())
            .await?)
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
        info!("Deleting api definition for {}/{}", id.0, version.0);
        Ok(self
            .client
            .delete_definition(id.0.as_str(), version.0.as_str())
            .await?)
    }

    async fn export(
        &self,
        id: ApiDefinitionId,
        version: ApiDefinitionVersion,
        _project: &Self::ProjectContext,
        format: &ApiDefinitionFileFormat,
    ) -> Result<String, GolemError> {
        info!("Exporting OpenAPI spec for {}/{}", id.0, version.0);

        // Get the API definition
        let api_def = self.client.get_definition(id.0.as_str(), version.0.as_str()).await?;

        // Export to OpenAPI format
        let spec = self.export_openapi(&api_def, format).await?;

        // Save to file
        let filename = format!("api_definition_{}_{}.{}", id.0, version.0, 
            if matches!(format, ApiDefinitionFileFormat::Json) { "json" } else { "yaml" });
        write(&filename, &spec).await
            .map_err(|e| GolemError(format!("Failed to write OpenAPI spec to file: {}", e)))?;

        Ok(format!("OpenAPI specification exported to {}", filename))
    }

    async fn ui(
        &self,
        id: ApiDefinitionId,
        version: ApiDefinitionVersion,
        _project: &Self::ProjectContext,
        port: u16,
    ) -> Result<String, GolemError> {
        info!("Starting SwaggerUI for {}/{} on port {}", id.0, version.0, port);

        // Get the API definition
        let api_def = self.client.get_definition(id.0.as_str(), version.0.as_str()).await?;

        // Export to OpenAPI format (always JSON for SwaggerUI)
        let spec = self.export_openapi(&api_def, &ApiDefinitionFileFormat::Json).await?;

        // Parse the spec into a JSON Value
        let spec_value: Value = serde_json::from_str(&spec)
            .map_err(|e| GolemError(format!("Failed to parse OpenAPI spec: {}", e)))?;

        // Configure SwaggerUI
        let config = SwaggerUiConfig {
            enabled: true,
            title: Some(format!("API Definition: {} ({})", id.0, version.0)),
            version: Some(version.0.clone()),
            server_url: Some(format!("http://localhost:{}", port)),
        };

        // Create API route with SwaggerUI
        let route = create_api_route(ApiSpec(spec_value), &config);
        
        // Start server
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        info!("SwaggerUI available at http://{}/docs", addr);
        
        // Run in background
        tokio::spawn(async move {
            if let Err(e) = poem::Server::new(TcpListener::bind(addr))
                .run(route)
                .await
            {
                eprintln!("Server error: {}", e);
            }
        });

        Ok(format!(
            "SwaggerUI started at http://127.0.0.1:{}/docs\nPress Ctrl+C to stop",
            port
        ))
    }
}
