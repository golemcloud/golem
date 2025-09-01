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

use crate::command::api::definition::ApiDefinitionSubcommand;
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::service::AnyhowMapServiceError;
use crate::error::NonSuccessfulExit;
use crate::log::{log_warn_action, LogColorize};
use crate::model::api::{ApiDefinitionId, ApiDefinitionVersion};
use crate::model::environment::ResolvedEnvironmentIdentity;
use crate::model::text::fmt::log_error;
use crate::model::OpenApiDefinitionOutputFormat;
use anyhow::{bail, Context as AnyhowContext};
use itertools::Itertools;
use serde::de::DeserializeOwned;
use std::sync::Arc;
use warp;
use webbrowser;

pub struct ApiDefinitionCommandHandler {
    ctx: Arc<Context>,
}

impl ApiDefinitionCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&self, command: ApiDefinitionSubcommand) -> anyhow::Result<()> {
        match command {
            ApiDefinitionSubcommand::Get { id, version } => self.cmd_get(id, version).await,
            ApiDefinitionSubcommand::List { id } => self.cmd_list(id).await,
            ApiDefinitionSubcommand::Export {
                id,
                version,
                format,
                output_name,
            } => self.cmd_export(id, version, format, output_name).await,
            ApiDefinitionSubcommand::Swagger { id, version, port } => {
                self.cmd_swagger(id, version, port).await
            }
        }
    }

    async fn cmd_get(
        &self,
        api_def_id: ApiDefinitionId,
        version: ApiDefinitionVersion,
    ) -> anyhow::Result<()> {
        let project = self
            .ctx
            .cloud_project_handler()
            .opt_select_project(project.project.as_ref())
            .await?;

        match self
            .api_definition(project.as_ref(), &api_def_id.0, &version.0)
            .await?
        {
            Some(result) => {
                self.ctx
                    .log_handler()
                    .log_view(&ApiDefinitionGetView(result));
                Ok(())
            }
            None => {
                log_error("Not found");
                bail!(NonSuccessfulExit)
            }
        }
    }

    async fn cmd_list(&self, api_definition_id: Option<ApiDefinitionId>) -> anyhow::Result<()> {
        let project = self
            .ctx
            .cloud_project_handler()
            .opt_select_project(project.project.as_ref())
            .await?;

        let clients = self.ctx.golem_clients().await?;

        let definitions = clients
            .api_definition
            .list_definitions(
                &self
                    .ctx
                    .cloud_project_handler()
                    .selected_project_id_or_default(project.as_ref())
                    .await?
                    .0,
                api_definition_id.as_ref().map(|id| id.0.as_str()),
            )
            .await
            .map_service_error()?;

        self.ctx.log_handler().log_view(&definitions);

        Ok(())
    }

    async fn cmd_export(
        &self,
        id: ApiDefinitionId,
        version: ApiDefinitionVersion,
        format: OpenApiDefinitionOutputFormat,
        output_name: Option<String>,
    ) -> anyhow::Result<()> {
        let project = self
            .ctx
            .cloud_project_handler()
            .opt_select_project(project.project.as_ref())
            .await?;

        let clients = self.ctx.golem_clients().await?;

        let response = clients
            .api_definition
            .export_definition(
                &self
                    .ctx
                    .cloud_project_handler()
                    .selected_project_id_or_default(project.as_ref())
                    .await?
                    .0,
                &id.0,
                &version.0,
            )
            .await
            .map_service_error()?;

        let openapi_spec = response.openapi_yaml;
        let file_name = output_name.unwrap_or_else(|| format!("{}_{}", id.0, version.0));
        let file_path = match format {
            OpenApiDefinitionOutputFormat::Json => format!("{file_name}.json"),
            OpenApiDefinitionOutputFormat::Yaml => format!("{file_name}.yaml"),
        };

        match format {
            OpenApiDefinitionOutputFormat::Json => {
                // Convert YAML to JSON for the JSON format option
                let yaml_obj: serde_yaml::Value = serde_yaml::from_str(&openapi_spec)?;
                let json_obj: serde_json::Value = serde_json::to_value(yaml_obj)?;
                let json_str = serde_json::to_string_pretty(&json_obj)?;
                std::fs::write(&file_path, json_str)?;
            }
            OpenApiDefinitionOutputFormat::Yaml => {
                // Use YAML directly for the YAML format option
                std::fs::write(&file_path, openapi_spec)?;
            }
        }

        let result = format!("Exported to {file_path}");

        self.ctx
            .log_handler()
            .log_view(&ApiDefinitionExportView(result));

        Ok(())
    }

    async fn cmd_swagger(
        &self,
        project: ProjectOptionalFlagArg,
        id: ApiDefinitionId,
        version: ApiDefinitionVersion,
        port: u16,
    ) -> anyhow::Result<()> {
        let project = self
            .ctx
            .cloud_project_handler()
            .opt_select_project(project.project.as_ref())
            .await?;

        let clients = self.ctx.golem_clients().await?;

        // First export the API spec
        let response = clients
            .api_definition
            .export_definition(
                &self
                    .ctx
                    .cloud_project_handler()
                    .selected_project_id_or_default(project.as_ref())
                    .await?
                    .0,
                &id.0,
                &version.0,
            )
            .await
            .map_service_error()?;

        let openapi_spec = response.openapi_yaml;

        // Parse the YAML spec into JSON Value
        let mut spec: serde_json::Value = serde_yaml::from_str(&openapi_spec)?;

        // Filter paths to only include methods with binding-type: default
        filter_routes(&mut spec);

        // Fetch deployments
        let project_id = self
            .ctx
            .cloud_project_handler()
            .selected_project_id_or_default(project.as_ref())
            .await?
            .0;

        let deployments = clients
            .api_deployment
            .list_deployments(&project_id, Some(&id.0))
            .await
            .map_service_error()?;

        // Add server information if deployments exist
        if !deployments.is_empty() {
            // Initialize servers array if it doesn't exist
            if !spec.as_object().unwrap().contains_key("servers") {
                spec.as_object_mut()
                    .unwrap()
                    .insert("servers".to_string(), serde_json::Value::Array(Vec::new()));
            }

            // Add servers to the spec
            if let Some(servers) = spec.get_mut("servers") {
                if let Some(servers) = servers.as_array_mut() {
                    // Add deployment servers with HTTP
                    for deployment in &deployments {
                        let url = match &deployment.site.subdomain {
                            Some(subdomain) => {
                                format!("http://{}.{}", subdomain, deployment.site.host)
                            }
                            None => format!("http://{}", deployment.site.host),
                        };
                        servers.push(serde_json::json!({
                            "url": url,
                            "description": "Deployed instance"
                        }));
                    }
                }
            }
        }

        // Validate port
        if port == 0 {
            return Err(anyhow::anyhow!(
                "Port number must be greater than 0. Provided: {}",
                port
            ));
        }

        // Start local Swagger UI server
        self.start_local_swagger_ui(serde_yaml::to_string(&spec)?, port)
            .await?;

        self.ctx
            .log_handler()
            .log_view(&ApiDefinitionExportView(format!(
                "Swagger UI running at http://localhost:{}\n{}",
                port,
                if deployments.is_empty() {
                    "No deployments found - displaying API schema without server information"
                        .to_string()
                } else {
                    format!("API is deployed at {} locations", deployments.len())
                }
            )));

        // Wait for ctrl+c
        tokio::signal::ctrl_c().await?;

        Ok(())
    }

    async fn start_local_swagger_ui(&self, spec: String, port: u16) -> anyhow::Result<()> {
        use std::net::TcpListener;
        use std::sync::Arc;
        use warp::Filter;

        // First check if the port is available
        let listener = TcpListener::bind(("127.0.0.1", port));
        if listener.is_err() {
            return Err(anyhow::anyhow!(
                "Port {} is already in use. Please choose a different port.",
                port
            ));
        }
        drop(listener); // Close the test connection

        // Parse the YAML to ensure it's valid
        let spec_value: serde_yaml::Value = serde_yaml::from_str(&spec)?;

        // Convert to JSON for Swagger UI
        let spec_json = serde_json::to_value(&spec_value)?;

        // Create a shared spec for the server
        let spec = Arc::new(spec_json);

        // Serve the OpenAPI spec as JSON
        let openapi_route =
            warp::path("openapi.json").map(move || warp::reply::json(spec.clone().as_ref()));

        // Serve the Swagger UI HTML
        let swagger_ui_html = r#"
            <!DOCTYPE html>
            <html lang="en">
            <head>
                <meta charset="UTF-8">
                <title>Swagger UI</title>
                <link rel="stylesheet" type="text/css" href="https://cdn.jsdelivr.net/npm/swagger-ui-dist@5/swagger-ui.css">
                <script src="https://cdn.jsdelivr.net/npm/swagger-ui-dist@5/swagger-ui-bundle.js"></script>
                <script src="https://cdn.jsdelivr.net/npm/swagger-ui-dist@5/swagger-ui-standalone-preset.js"></script>
            </head>
            <body>
                <div id="swagger-ui"></div>
                <script>
                    window.onload = () => {
                        window.ui = SwaggerUIBundle({
                            url: '/openapi.json',
                            dom_id: '#swagger-ui',
                            deepLinking: true,
                            presets: [
                                SwaggerUIBundle.presets.apis,
                                SwaggerUIStandalonePreset
                            ],
                            layout: "StandaloneLayout"
                        });
                    };
                </script>
            </body>
            </html>
        "#;

        let swagger_route = warp::path::end().map(move || warp::reply::html(swagger_ui_html));

        // Combine routes
        let routes = openapi_route.or(swagger_route);

        // Start server in a background task
        tokio::spawn(async move {
            warp::serve(routes).run(([127, 0, 0, 1], port)).await;
        });

        // Try to open browser
        match webbrowser::open(&format!("http://localhost:{port}")) {
            Ok(_) => println!("Browser opened successfully."),
            Err(_) => println!(
                "Could not open browser automatically. You can access the Swagger UI at: http://localhost:{port}"
            ),
        }

        Ok(())
    }

    async fn api_definition(
        &self,
        environment: Option<&ResolvedEnvironmentIdentity>,
        name: &str,
        version: &str,
    ) -> anyhow::Result<Option<HttpApiDefinitionResponseData>> {
        let clients = self.ctx.golem_clients().await?;

        clients
            .api_definition
            .get_definition(
                &self
                    .ctx
                    .cloud_project_handler()
                    .selected_project_id_or_default(project)
                    .await?
                    .0,
                name,
                version,
            )
            .await
            .map_service_error_not_found_as_opt()
    }

    async fn update_api_definition(
        &self,
        environment: Option<&ResolvedEnvironmentIdentity>,
        manifest_api_definition: &HttpApiDefinitionRequest,
    ) -> anyhow::Result<HttpApiDefinitionResponseData> {
        let clients = self.ctx.golem_clients().await?;

        clients
            .api_definition
            .update_definition_json(
                &self
                    .ctx
                    .cloud_project_handler()
                    .selected_project_id_or_default(project)
                    .await?
                    .0,
                &manifest_api_definition.id,
                &manifest_api_definition.version,
                // TODO: would be nice to share the model between oss and cloud instead of "re-encoding"
                &parse_api_definition(&serde_yaml::to_string(&manifest_api_definition)?)?,
            )
            .await
            .map_service_error()
    }

    async fn new_api_definition(
        &self,
        environment: Option<&ResolvedEnvironmentIdentity>,
        api_definition: &HttpApiDefinitionRequest,
    ) -> anyhow::Result<HttpApiDefinitionResponseData> {
        let clients = self.ctx.golem_clients().await?;

        clients
            .api_definition
            .create_definition_json(
                &self
                    .ctx
                    .cloud_project_handler()
                    .selected_project_id_or_default(project)
                    .await?
                    .0,
                // TODO: would be nice to share the model between oss and cloud instead of "re-encoding"
                &parse_api_definition(&serde_yaml::to_string(&api_definition)?)?,
            )
            .await
            .map_service_error()
    }
}

fn parse_api_definition<T: DeserializeOwned>(input: &str) -> anyhow::Result<T> {
    serde_yaml::from_str(input).context("Failed to parse API definition")
}

/// Filters the OpenAPI specification to only include methods with `binding-type: default`.
fn filter_routes(spec: &mut serde_json::Value) {
    if let Some(paths) = spec.get_mut("paths").and_then(|p| p.as_object_mut()) {
        let paths_to_remove: Vec<String> = paths
            .iter_mut()
            .filter_map(|(path, methods)| {
                if let Some(methods) = methods.as_object_mut() {
                    // Filter methods within this path
                    let methods_to_remove: Vec<String> = methods
                        .iter()
                        .filter_map(|(method, details)| {
                            // Check if the method has a binding-type and if it's not "default"
                            if let Some(binding) = details.get("x-golem-api-gateway-binding") {
                                if let Some(binding_type) = binding.get("binding-type") {
                                    if binding_type != "default" {
                                        return Some(method.clone());
                                    }
                                }
                            }
                            None
                        })
                        .collect();

                    // Remove filtered methods
                    for method in methods_to_remove {
                        methods.remove(&method);
                    }

                    // If no methods left in this path, mark it for removal
                    if methods.is_empty() {
                        return Some(path.clone());
                    }
                }
                None
            })
            .collect();

        // Remove empty paths
        for path in paths_to_remove {
            paths.remove(&path);
        }
    }
}
