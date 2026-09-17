// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.

use super::ApiResult;
use crate::services::auth::AuthService;
use crate::services::mcp_import::McpImportResolver;
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::mcp_import::{McpImportDeployment, McpImportSource};
use golem_common::recorded_http_api_request;
use golem_common::schema::tool::Tool;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::{AuthCtx, GolemSecurityScheme};
use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use poem_openapi::{Object, OpenApi};
use std::sync::Arc;
use tracing::Instrument;

#[derive(Debug, Object)]
#[oai(rename_all = "camelCase")]
pub struct McpImportedTool {
    pub upstream_name: String,
    pub digest: String,
    pub definition: Tool,
}

#[derive(Debug, Object)]
#[oai(rename_all = "camelCase")]
pub struct McpImportDiagnostic {
    pub upstream_name: String,
    pub reason: String,
}

#[derive(Debug, Object)]
#[oai(rename_all = "camelCase")]
pub struct McpImportTools {
    pub environment_id: EnvironmentId,
    pub deployment_revision: DeploymentRevision,
    pub import_index: u32,
    pub protocol_version: String,
    pub tools: Vec<McpImportedTool>,
    pub diagnostics: Vec<McpImportDiagnostic>,
}

#[derive(Debug, Object)]
#[oai(rename_all = "camelCase")]
pub struct McpImportResolutionRequest {
    pub imports: Vec<McpImportDeployment>,
    pub native_tool_names: Vec<String>,
}

#[derive(Debug, Object)]
#[oai(rename_all = "camelCase")]
pub struct McpResolvedTool {
    pub import_index: u32,
    pub upstream_name: String,
    pub digest: String,
    pub definition: Tool,
}

#[derive(Debug, Object)]
#[oai(rename_all = "camelCase")]
pub struct McpResolvedDiagnostic {
    pub import_index: u32,
    pub upstream_name: String,
    pub reason: String,
}

#[derive(Debug, Object)]
#[oai(rename_all = "camelCase")]
pub struct McpImportResolution {
    pub protocol_versions: Vec<String>,
    pub tools: Vec<McpResolvedTool>,
    pub diagnostics: Vec<McpResolvedDiagnostic>,
}

pub struct McpImportsApi {
    resolver: Arc<McpImportResolver>,
    auth: Arc<AuthService>,
}

impl McpImportsApi {
    pub fn new(resolver: Arc<McpImportResolver>, auth: Arc<AuthService>) -> Self {
        Self { resolver, auth }
    }

    async fn inspect(
        &self,
        environment_id: EnvironmentId,
        deployment_revision: DeploymentRevision,
        import_index: u32,
        auth: AuthCtx,
        refresh: bool,
    ) -> ApiResult<Json<McpImportTools>> {
        let observation = self
            .resolver
            .inspect(
                McpImportSource {
                    environment_id,
                    deployment_revision,
                    import_index,
                    upstream_tool_name: String::new(),
                },
                auth,
                refresh,
            )
            .await?;
        Ok(Json(McpImportTools {
            environment_id,
            deployment_revision,
            import_index,
            protocol_version: observation.protocol_version.clone(),
            tools: observation
                .tools
                .iter()
                .map(|tool| McpImportedTool {
                    upstream_name: tool.upstream_name.clone(),
                    digest: tool.digest.clone(),
                    definition: tool.definition.clone(),
                })
                .collect(),
            diagnostics: observation
                .diagnostics
                .iter()
                .map(|diagnostic| McpImportDiagnostic {
                    upstream_name: diagnostic.upstream_name.clone(),
                    reason: diagnostic.reason.clone(),
                })
                .collect(),
        }))
    }
}

#[OpenApi(prefix_path = "/v1", tag = ApiTags::RegistryService)]
impl McpImportsApi {
    #[oai(path = "/envs/:environment_id/mcp-imports/resolve", method = "post", operation_id = "resolve_mcp_imports", tag = ApiTags::Environment)]
    async fn resolve_imports(
        &self,
        environment_id: Path<EnvironmentId>,
        request: Json<McpImportResolutionRequest>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<McpImportResolution>> {
        let record = recorded_http_api_request!(
            "resolve_mcp_imports",
            environment_id = environment_id.0.to_string()
        );
        let auth = self.auth.authenticate_token(token.secret()).await?;
        let request = request.0;
        let result = async {
            let preview = self
                .resolver
                .preview(
                    environment_id.0,
                    request.imports,
                    request.native_tool_names,
                    auth,
                )
                .await?;
            Ok(Json(McpImportResolution {
                protocol_versions: preview.protocol_versions,
                tools: preview
                    .tools
                    .into_iter()
                    .map(|(index, tool)| McpResolvedTool {
                        import_index: index as u32,
                        upstream_name: tool.upstream_name,
                        digest: tool.digest,
                        definition: tool.definition,
                    })
                    .collect(),
                diagnostics: preview
                    .diagnostics
                    .into_iter()
                    .map(|(index, diagnostic)| McpResolvedDiagnostic {
                        import_index: index as u32,
                        upstream_name: diagnostic.upstream_name,
                        reason: diagnostic.reason,
                    })
                    .collect(),
            }))
        }
        .instrument(record.span.clone())
        .await;
        record.result(result)
    }

    /// Inspect projected tools from an import in a specific deployment.
    /// Cache misses fetch upstream metadata on demand.
    /// This is the individual import's projection, before native and earlier-import precedence.
    /// Requires environment/deployment visibility and ViewTools; network requests use the owner's quota.
    #[oai(path = "/envs/:environment_id/deployments/:deployment_revision/mcp-imports/:import_index/tools", method = "get", operation_id = "get_mcp_import_tools", tag = ApiTags::Environment, tag = ApiTags::Deployment)]
    async fn tools(
        &self,
        environment_id: Path<EnvironmentId>,
        deployment_revision: Path<DeploymentRevision>,
        import_index: Path<u32>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<McpImportTools>> {
        let record = recorded_http_api_request!(
            "get_mcp_import_tools",
            environment_id = environment_id.0.to_string(),
            deployment_revision = deployment_revision.0.to_string(),
            import_index = import_index.0
        );
        let auth = self.auth.authenticate_token(token.secret()).await?;
        let result = self
            .inspect(
                environment_id.0,
                deployment_revision.0,
                import_index.0,
                auth,
                false,
            )
            .instrument(record.span.clone())
            .await;
        record.result(result)
    }

    /// Refresh and inspect projected tools from an import in a specific deployment.
    /// Uses the same permissions and owner quota as inspection, bypassing cached success.
    /// Upstream failure is returned explicitly rather than serving stale metadata.
    #[oai(path = "/envs/:environment_id/deployments/:deployment_revision/mcp-imports/:import_index/refresh", method = "post", operation_id = "refresh_mcp_import_tools", tag = ApiTags::Environment, tag = ApiTags::Deployment)]
    async fn refresh(
        &self,
        environment_id: Path<EnvironmentId>,
        deployment_revision: Path<DeploymentRevision>,
        import_index: Path<u32>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<McpImportTools>> {
        let record = recorded_http_api_request!(
            "refresh_mcp_import_tools",
            environment_id = environment_id.0.to_string(),
            deployment_revision = deployment_revision.0.to_string(),
            import_index = import_index.0
        );
        let auth = self.auth.authenticate_token(token.secret()).await?;
        let result = self
            .inspect(
                environment_id.0,
                deployment_revision.0,
                import_index.0,
                auth,
                true,
            )
            .instrument(record.span.clone())
            .await;
        record.result(result)
    }
}
