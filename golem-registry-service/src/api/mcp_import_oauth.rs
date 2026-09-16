// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");

use super::ApiResult;
use crate::repo::model::mcp_oauth::McpOAuthGrantStatus;
use crate::services::auth::AuthService;
use crate::services::mcp_oauth::{McpOAuthCallback, McpOAuthService};
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::mcp_import::McpImportSource;
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use poem_openapi::{Object, OpenApi};
use std::sync::Arc;
use tracing::Instrument;

#[derive(Object)]
#[oai(rename_all = "camelCase")]
pub struct McpImportAuthorization {
    pub authorization_url: String,
    pub deployment_revision: DeploymentRevision,
    pub import_index: u32,
}

#[derive(Object)]
pub struct McpImportOAuthCallback {
    pub state: String,
    pub issuer: Option<String>,
    pub code: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Object)]
#[oai(rename_all = "camelCase")]
pub struct McpImportOAuthStatus {
    pub environment_id: EnvironmentId,
    pub deployment_revision: DeploymentRevision,
    pub import_index: u32,
    pub security_scheme: String,
    pub status: String,
}

pub struct McpImportOAuthApi {
    service: Arc<McpOAuthService>,
    auth: Arc<AuthService>,
}

impl McpImportOAuthApi {
    pub fn new(service: Arc<McpOAuthService>, auth: Arc<AuthService>) -> Self {
        Self { service, auth }
    }
}

#[OpenApi(prefix_path = "/v1", tag = ApiTags::RegistryService)]
impl McpImportOAuthApi {
    #[oai(path = "/envs/:environment_id/deployments/:deployment_revision/mcp-imports/:import_index/oauth/authorize", method = "post", operation_id = "authorize_mcp_import", tag = ApiTags::Environment, tag = ApiTags::Deployment)]
    async fn authorize(
        &self,
        environment_id: Path<EnvironmentId>,
        deployment_revision: Path<DeploymentRevision>,
        import_index: Path<u32>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<McpImportAuthorization>> {
        let record = recorded_http_api_request!(
            "authorize_mcp_import",
            environment_id = environment_id.0.to_string(),
            deployment_revision = deployment_revision.0.to_string(),
            import_index = import_index.0
        );
        let auth = self.auth.authenticate_token(token.secret()).await?;
        let source = source(environment_id.0, deployment_revision.0, import_index.0);
        let result = async {
            Ok(Json(McpImportAuthorization {
                authorization_url: self.service.authorize(&source, &auth).await?.into(),
                deployment_revision: source.deployment_revision,
                import_index: source.import_index,
            }))
        }
        .instrument(record.span.clone())
        .await;
        record.result(result)
    }

    #[oai(path = "/envs/:environment_id/deployments/:deployment_revision/mcp-imports/:import_index/oauth/complete", method = "post", operation_id = "complete_mcp_import_authorization", tag = ApiTags::Environment, tag = ApiTags::Deployment)]
    async fn complete(
        &self,
        environment_id: Path<EnvironmentId>,
        deployment_revision: Path<DeploymentRevision>,
        import_index: Path<u32>,
        callback: Json<McpImportOAuthCallback>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<McpImportOAuthStatus>> {
        let record = recorded_http_api_request!(
            "complete_mcp_import_authorization",
            environment_id = environment_id.0.to_string(),
            deployment_revision = deployment_revision.0.to_string(),
            import_index = import_index.0
        );
        let auth = self.auth.authenticate_token(token.secret()).await?;
        let source = source(environment_id.0, deployment_revision.0, import_index.0);
        let callback = callback.0;
        let result = async {
            let scheme = self
                .service
                .complete_operator(
                    &source,
                    McpOAuthCallback {
                        state: callback.state,
                        issuer: callback.issuer,
                        code: callback.code,
                        error: callback.error,
                    },
                    &auth,
                )
                .await?;
            Ok(Json(status_value(
                source,
                scheme.0,
                McpOAuthGrantStatus::Granted,
            )))
        }
        .instrument(record.span.clone())
        .await;
        record.result(result)
    }

    #[oai(path = "/envs/:environment_id/deployments/:deployment_revision/mcp-imports/:import_index/oauth", method = "get", operation_id = "get_mcp_import_authorization_status", tag = ApiTags::Environment, tag = ApiTags::Deployment)]
    async fn status(
        &self,
        environment_id: Path<EnvironmentId>,
        deployment_revision: Path<DeploymentRevision>,
        import_index: Path<u32>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<McpImportOAuthStatus>> {
        let record = recorded_http_api_request!(
            "get_mcp_import_authorization_status",
            environment_id = environment_id.0.to_string(),
            deployment_revision = deployment_revision.0.to_string(),
            import_index = import_index.0
        );
        let auth = self.auth.authenticate_token(token.secret()).await?;
        let result = status(
            &self.service,
            source(environment_id.0, deployment_revision.0, import_index.0),
            &auth,
        )
        .instrument(record.span.clone())
        .await;
        record.result(result)
    }

    #[oai(path = "/envs/:environment_id/deployments/:deployment_revision/mcp-imports/:import_index/oauth", method = "delete", operation_id = "disconnect_mcp_import", tag = ApiTags::Environment, tag = ApiTags::Deployment)]
    async fn disconnect(
        &self,
        environment_id: Path<EnvironmentId>,
        deployment_revision: Path<DeploymentRevision>,
        import_index: Path<u32>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<McpImportOAuthStatus>> {
        let record = recorded_http_api_request!(
            "disconnect_mcp_import",
            environment_id = environment_id.0.to_string(),
            deployment_revision = deployment_revision.0.to_string(),
            import_index = import_index.0
        );
        let auth = self.auth.authenticate_token(token.secret()).await?;
        let source = source(environment_id.0, deployment_revision.0, import_index.0);
        let result = async {
            let scheme = self.service.disconnect(&source, &auth).await?;
            Ok(Json(status_value(
                source,
                scheme.0,
                McpOAuthGrantStatus::Revoked,
            )))
        }
        .instrument(record.span.clone())
        .await;
        record.result(result)
    }
}

fn source(
    environment_id: EnvironmentId,
    deployment_revision: DeploymentRevision,
    import_index: u32,
) -> McpImportSource {
    McpImportSource {
        environment_id,
        deployment_revision,
        import_index,
        upstream_tool_name: String::new(),
    }
}

async fn status(
    service: &McpOAuthService,
    source: McpImportSource,
    auth: &golem_service_base::model::auth::AuthCtx,
) -> ApiResult<Json<McpImportOAuthStatus>> {
    let (scheme, state) = service.status(&source, auth).await?;
    Ok(Json(status_value(source, scheme.0, state)))
}

fn status_value(
    source: McpImportSource,
    scheme: String,
    state: McpOAuthGrantStatus,
) -> McpImportOAuthStatus {
    McpImportOAuthStatus {
        environment_id: source.environment_id,
        deployment_revision: source.deployment_revision,
        import_index: source.import_index,
        security_scheme: scheme,
        status: match state {
            crate::repo::model::mcp_oauth::McpOAuthGrantStatus::PendingConsent => "pending-consent",
            crate::repo::model::mcp_oauth::McpOAuthGrantStatus::Exchanging => "exchanging",
            crate::repo::model::mcp_oauth::McpOAuthGrantStatus::Granted => "granted",
            crate::repo::model::mcp_oauth::McpOAuthGrantStatus::Refreshing => "refreshing",
            crate::repo::model::mcp_oauth::McpOAuthGrantStatus::ReauthorizationRequired => {
                "authorization-required"
            }
            crate::repo::model::mcp_oauth::McpOAuthGrantStatus::Revoked => "revoked",
        }
        .into(),
    }
}

#[cfg(test)]
mod tests;
