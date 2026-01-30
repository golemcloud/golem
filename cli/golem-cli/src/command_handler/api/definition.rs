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
use crate::log::{log_action, log_warn_action, logln, LogColorize, LogIndent};
use crate::model::environment::{EnvironmentResolveMode, ResolvedEnvironmentIdentity};
use crate::model::text::fmt::{log_error, log_warn};
use crate::model::text::http_api_definition::HttpApiDefinitionGetView;
use crate::model::{app, OpenApiDefinitionOutputFormat};
use anyhow::{anyhow, bail};
use golem_client::api::{ApiDeploymentClient, EnvironmentClient, HttpApiDefinitionClient};
use golem_client::model::{HttpApiDefinitionCreation, HttpApiDefinitionUpdate};
use golem_common::cache::SimpleCache;
use golem_common::model::deployment::{DeploymentPlanHttpApiDefintionEntry, DeploymentRevision};
use golem_common::model::diff;
use golem_common::model::http_api_definition::{
    HttpApiDefinition, HttpApiDefinitionId, HttpApiDefinitionName, HttpApiDefinitionRevision,
    HttpApiDefinitionVersion,
};
use serde_json::Value;
use std::collections::BTreeMap;
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
            ApiDefinitionSubcommand::Get { name, revision } => self.cmd_get(name, revision).await,
            ApiDefinitionSubcommand::List => self.cmd_list().await,
            ApiDefinitionSubcommand::OpenApi {
                name,
                deployment_revision,
                format,
                output_name,
            } => {
                self.cmd_openapi(name, deployment_revision, format, output_name)
                    .await
            }
            ApiDefinitionSubcommand::Swagger {
                name,
                revision,
                port,
            } => self.cmd_swagger(name, revision, port).await,
        }
    }

    async fn cmd_get(
        &self,
        name: HttpApiDefinitionName,
        revision: Option<HttpApiDefinitionRevision>,
    ) -> anyhow::Result<()> {
        let environment = self
            .ctx
            .environment_handler()
            .resolve_environment(EnvironmentResolveMode::Any)
            .await?;

        let Some(definition) = self
            .resolve_http_api_definition(&environment, &name, revision.as_ref())
            .await?
        else {
            // TODO: atomic: log latest revision
            log_error("HTTP API definition not found");
            bail!(NonSuccessfulExit)
        };

        self.ctx
            .log_handler()
            .log_view(&HttpApiDefinitionGetView(definition));

        Ok(())
    }

    async fn cmd_list(&self) -> anyhow::Result<()> {
        let environment = self
            .ctx
            .environment_handler()
            .resolve_environment(EnvironmentResolveMode::Any)
            .await?;

        let definitions = environment
            .with_current_deployment_revision_or_default_warn(
                |current_deployment_revision| async move {
                    Ok(self
                        .ctx
                        .golem_clients()
                        .await?
                        .environment
                        .list_deployment_http_api_definitions_legacy(
                            &environment.environment_id.0,
                            current_deployment_revision.into(),
                        )
                        .await
                        .map_service_error()?
                        .values)
                },
            )
            .await?;

        self.ctx.log_handler().log_view(&definitions);

        Ok(())
    }

    async fn cmd_openapi(
        &self,
        name: HttpApiDefinitionName,
        deployment_revision: Option<DeploymentRevision>,
        format: OpenApiDefinitionOutputFormat,
        output_name: Option<String>,
    ) -> anyhow::Result<()> {
        let environment = self
            .ctx
            .environment_handler()
            .resolve_environment(EnvironmentResolveMode::Any)
            .await?;

        let openapi_spec = self
            .get_deployed_openapi_spec(&environment, &name, deployment_revision.as_ref())
            .await?;

        let file_name = output_name.unwrap_or_else(|| name.0.clone());
        let file_path = match format {
            OpenApiDefinitionOutputFormat::Json => format!("{file_name}.json"),
            OpenApiDefinitionOutputFormat::Yaml => format!("{file_name}.yaml"),
        };

        match format {
            OpenApiDefinitionOutputFormat::Json => {
                let json_obj: serde_json::Value = serde_json::to_value(openapi_spec)?;
                let json_str = serde_json::to_string_pretty(&json_obj)?;
                std::fs::write(&file_path, json_str)?;
            }
            OpenApiDefinitionOutputFormat::Yaml => {
                let yaml_str = serde_json::to_string_pretty(&openapi_spec)?;
                std::fs::write(&file_path, yaml_str)?;
            }
        }

        log_action(
            "Exported",
            format!(
                "OpenAPI spec for {} to {}",
                name.0.log_color_highlight(),
                file_path.log_color_highlight()
            ),
        );

        Ok(())
    }

    async fn cmd_swagger(
        &self,
        name: HttpApiDefinitionName,
        deployment_revision: Option<DeploymentRevision>,
        port: u16,
    ) -> anyhow::Result<()> {
        let environment = self
            .ctx
            .environment_handler()
            .resolve_environment(EnvironmentResolveMode::Any)
            .await?;

        let current_deployment = environment.current_deployment_or_err()?;

        let mut openapi_spec = self
            .get_deployed_openapi_spec(&environment, &name, deployment_revision.as_ref())
            .await?;

        // Filter paths to only include methods with binding-type: default
        filter_routes(&mut openapi_spec);

        // Fetch deployments
        let deployments = self
            .ctx
            .golem_clients()
            .await?
            .api_deployment
            .list_http_api_deployments_in_deployment_legacy(
                &environment.environment_id.0,
                current_deployment.deployment_revision.get(),
            )
            .await
            .map_service_error()?
            .values
            .into_iter()
            .filter(|d| d.api_definitions.contains(&name))
            .collect::<Vec<_>>();

        // Add server information if deployments exist
        if !deployments.is_empty() {
            // Initialize servers array if it doesn't exist
            if !openapi_spec.as_object().unwrap().contains_key("servers") {
                openapi_spec
                    .as_object_mut()
                    .unwrap()
                    .insert("servers".to_string(), serde_json::Value::Array(Vec::new()));
            }

            // Add servers to the spec
            if let Some(servers) = openapi_spec.get_mut("servers") {
                if let Some(servers) = servers.as_array_mut() {
                    // Add deployment servers with HTTP
                    for deployment in &deployments {
                        // TODO: how should we know if it is HTTP or HTTPS?
                        servers.push(serde_json::json!({
                            "url": format!("http://{}", deployment.domain),
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
        self.start_local_swagger_ui(serde_yaml::to_string(&openapi_spec)?, port)
            .await?;

        log_action(
            "Started",
            format!("Swagger UI, running at http://localhost:{}", port),
        );
        logln("");
        if deployments.is_empty() {
            log_warn("No deployments found - displaying API schema without server information")
        } else {
            logln("API is deployed at:".log_color_help_group().to_string());
            for deployment in &deployments {
                logln(format!("- {}", deployment.domain));
            }
        }
        logln("");
        logln("Press Ctrl+C to quit");

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

    async fn resolve_http_api_definition(
        &self,
        environment: &ResolvedEnvironmentIdentity,
        name: &HttpApiDefinitionName,
        revision: Option<&HttpApiDefinitionRevision>,
    ) -> anyhow::Result<Option<HttpApiDefinition>> {
        environment
            .with_current_deployment_revision_or_default_warn(
                |current_deployment_revision| async move {
                    let clients = self.ctx.golem_clients().await?;
                    let Some(definition) = clients
                        .api_definition
                        .get_http_api_definition_in_deployment_legacy(
                            &environment.environment_id.0,
                            current_deployment_revision.into(),
                            &name.0,
                        )
                        .await
                        .map_service_error_not_found_as_opt()?
                    else {
                        return Ok(None);
                    };

                    let Some(revision) = revision else {
                        return Ok(Some(definition));
                    };

                    clients
                        .api_definition
                        .get_http_api_definition_revision_legacy(
                            &definition.id.0,
                            (*revision).into(),
                        )
                        .await
                        .map_service_error_not_found_as_opt()
                },
            )
            .await
    }

    pub async fn get_http_api_definition_revision_by_id(
        &self,
        http_api_definition_id: &HttpApiDefinitionId,
        revision: HttpApiDefinitionRevision,
    ) -> anyhow::Result<HttpApiDefinition> {
        self.ctx
            .caches()
            .http_api_definition_revision
            .get_or_insert_simple(&(*http_api_definition_id, revision), {
                let ctx = self.ctx.clone();
                async move || {
                    ctx.golem_clients()
                        .await?
                        .api_definition
                        .get_http_api_definition_revision_legacy(
                            &http_api_definition_id.0,
                            revision.into(),
                        )
                        .await
                        .map_service_error()
                        .map_err(Arc::new)
                }
            })
            .await
            .map_err(|err| anyhow!(err))
    }

    async fn get_deployed_openapi_spec(
        &self,
        environment: &ResolvedEnvironmentIdentity,
        name: &HttpApiDefinitionName,
        deployment_revision: Option<&DeploymentRevision>,
    ) -> anyhow::Result<Value> {
        let deployment_revision = match deployment_revision {
            Some(deployment_revision) => deployment_revision.to_owned(),
            None => match &environment.server_environment.current_deployment {
                Some(deployment) => deployment.deployment_revision,
                None => {
                    log_error(format!(
                        "The application is not deployed. Run {}, then retry the command!",
                        "golem deploy"
                    ));
                    bail!(NonSuccessfulExit);
                }
            },
        };

        Ok(self
            .ctx
            .golem_clients()
            .await?
            .api_definition
            .get_openapi_of_http_api_definition_in_deployment_legacy(
                &environment.environment_id.0,
                deployment_revision.into(),
                &name.0,
            )
            .await?
            .0)
    }

    pub async fn deployable_manifest_http_api_definitions(
        &self,
    ) -> anyhow::Result<BTreeMap<HttpApiDefinitionName, app::HttpApiDefinition>> {
        let app_ctx = self.ctx.app_context_lock().await;
        let app_ctx = app_ctx.some_or_err()?;

        Ok(app_ctx
            .application()
            .http_api_definitions()
            .iter()
            .map(|(name, source_and_value)| (name.clone(), source_and_value.value.clone()))
            .collect())
    }

    pub async fn create_staged_http_api_definition(
        &self,
        environment: &ResolvedEnvironmentIdentity,
        http_api_definition_name: &HttpApiDefinitionName,
        deployable_http_api_definition: &app::HttpApiDefinition,
    ) -> anyhow::Result<()> {
        log_action(
            "Creating",
            format!(
                "HTTP API definition {}",
                http_api_definition_name.0.log_color_highlight()
            ),
        );
        let _indent = LogIndent::new();

        let definition = self
            .ctx
            .golem_clients()
            .await?
            .api_definition
            .create_http_api_definition_legacy(
                &environment.environment_id.0,
                &HttpApiDefinitionCreation {
                    name: http_api_definition_name.clone(),
                    version: HttpApiDefinitionVersion(
                        deployable_http_api_definition.version.0.clone(),
                    ),
                    routes: deployable_http_api_definition.routes.clone(),
                },
            )
            .await
            .map_service_error()?;

        log_action(
            "Created",
            format!(
                "HTTP API definition revision: {} {}",
                definition.name.0.log_color_highlight(),
                definition.revision.to_string().log_color_highlight()
            ),
        );

        Ok(())
    }

    pub async fn delete_staged_http_api_definition(
        &self,
        http_api_definition: &DeploymentPlanHttpApiDefintionEntry,
    ) -> anyhow::Result<()> {
        log_warn_action(
            "Deleting",
            format!(
                "HTTP API definition {}",
                http_api_definition.name.0.log_color_highlight()
            ),
        );
        let _indent = LogIndent::new();

        self.ctx
            .golem_clients()
            .await?
            .api_definition
            .delete_http_api_definition_legacy(
                &http_api_definition.id.0,
                http_api_definition.revision.into(),
            )
            .await
            .map_service_error()?;

        log_action(
            "Deleted",
            format!(
                "HTTP API definition revision: {} {}",
                http_api_definition.name.0.log_color_highlight(),
                http_api_definition
                    .revision
                    .to_string()
                    .log_color_highlight()
            ),
        );

        Ok(())
    }

    pub async fn update_staged_http_api_definition(
        &self,
        http_api_definition: &DeploymentPlanHttpApiDefintionEntry,
        deployable_http_api_definition: &app::HttpApiDefinition,
        _diff: &diff::DiffForHashOf<diff::HttpApiDefinition>,
    ) -> anyhow::Result<()> {
        log_action(
            "Updating",
            format!(
                "HTTP API definition {}",
                http_api_definition.name.0.log_color_highlight()
            ),
        );
        let _indent = LogIndent::new();

        let definition = self
            .ctx
            .golem_clients()
            .await?
            .api_definition
            .update_http_api_definition_legacy(
                &http_api_definition.id.0,
                &HttpApiDefinitionUpdate {
                    current_revision: http_api_definition.revision,
                    version: Some(deployable_http_api_definition.version.clone()),
                    routes: Some(deployable_http_api_definition.routes.clone()),
                },
            )
            .await
            .map_service_error()?;

        log_action(
            "Created",
            format!(
                "HTTP API definition revision: {} {}",
                definition.name.0.log_color_highlight(),
                definition.revision.to_string().log_color_highlight()
            ),
        );

        Ok(())
    }
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
