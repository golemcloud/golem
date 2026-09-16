// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

use crate::command::api::McpImportOAuthSubcommand;
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::service::MapServiceError;
use crate::model::environment::EnvironmentResolveMode;
use crate::model::mcp::{
    McpImportAuthorizeView, McpImportCompleteView, McpImportDisconnectView, McpImportStatusView,
};
use golem_client::api::DeploymentClient;
use golem_client::model::McpImportOAuthCallback;
use golem_common::model::deployment::DeploymentRevision;
use std::sync::Arc;

pub async fn handle(ctx: &Arc<Context>, command: McpImportOAuthSubcommand) -> anyhow::Result<()> {
    let environment = ctx
        .environment_handler()
        .resolve_environment(EnvironmentResolveMode::Any)
        .await?;
    let (index, requested) = match &command {
        McpImportOAuthSubcommand::Authorize {
            import_index,
            revision,
        }
        | McpImportOAuthSubcommand::Complete {
            import_index,
            revision,
            ..
        }
        | McpImportOAuthSubcommand::Status {
            import_index,
            revision,
        }
        | McpImportOAuthSubcommand::Disconnect {
            import_index,
            revision,
        } => (*import_index, *revision),
    };
    let revision = resolve_revision(requested, || {
        Ok(environment.current_deployment_or_err()?.deployment_revision)
    })?;
    let clients = ctx.golem_clients().await?;
    match command {
        McpImportOAuthSubcommand::Authorize { .. } => {
            let result = clients
                .deployment
                .authorize_mcp_import(&environment.environment_id.0, revision.get(), index)
                .await
                .map_service_error()?;
            ctx.log_handler()
                .log_output(McpImportAuthorizeView(result))?;
        }
        McpImportOAuthSubcommand::Complete { callback_url, .. } => {
            let callback = parse_callback(callback_url.into_inner())?;
            let result = clients
                .deployment
                .complete_mcp_import_authorization(
                    &environment.environment_id.0,
                    revision.get(),
                    index,
                    &callback,
                )
                .await
                .map_service_error()?;
            ctx.log_handler()
                .log_output(McpImportCompleteView(result))?;
        }
        McpImportOAuthSubcommand::Status { .. } => {
            let result = clients
                .deployment
                .get_mcp_import_authorization_status(
                    &environment.environment_id.0,
                    revision.get(),
                    index,
                )
                .await
                .map_service_error()?;
            ctx.log_handler().log_output(McpImportStatusView(result))?;
        }
        McpImportOAuthSubcommand::Disconnect { .. } => {
            let result = clients
                .deployment
                .disconnect_mcp_import(&environment.environment_id.0, revision.get(), index)
                .await
                .map_service_error()?;
            ctx.log_handler()
                .log_output(McpImportDisconnectView(result))?;
        }
    }
    Ok(())
}

fn resolve_revision(
    requested: Option<DeploymentRevision>,
    current: impl FnOnce() -> anyhow::Result<DeploymentRevision>,
) -> anyhow::Result<DeploymentRevision> {
    match requested {
        Some(revision) => Ok(revision),
        None => current(),
    }
}

fn parse_callback(url: url::Url) -> anyhow::Result<McpImportOAuthCallback> {
    let mut state = None;
    let mut issuer = None;
    let mut code = None;
    let mut error = None;
    for (name, value) in url.query_pairs() {
        let slot = match name.as_ref() {
            "state" => &mut state,
            "iss" => &mut issuer,
            "code" => &mut code,
            "error" => &mut error,
            _ => continue,
        };
        if slot.replace(value.into_owned()).is_some() {
            anyhow::bail!("duplicate OAuth callback parameter: {name}");
        }
    }
    let state = state.ok_or_else(|| anyhow::anyhow!("OAuth callback is missing state"))?;
    if code.is_some() == error.is_some() {
        anyhow::bail!("OAuth callback must contain exactly one of code or error");
    }
    Ok(McpImportOAuthCallback {
        state,
        issuer,
        code,
        error,
    })
}

#[cfg(test)]
mod tests {
    use super::{parse_callback, resolve_revision};
    use golem_common::model::deployment::DeploymentRevision;
    use test_r::test;

    #[test]
    fn explicit_revision_does_not_resolve_current_deployment() {
        let revision = DeploymentRevision::try_from(42_u64).unwrap();
        let resolved = resolve_revision(Some(revision), || anyhow::bail!("no deployment"));
        assert_eq!(resolved.unwrap(), revision);
    }

    #[test]
    fn callback_requires_exactly_one_state() {
        assert!(parse_callback("https://app/callback?code=x".parse().unwrap()).is_err());
        assert!(
            parse_callback(
                "https://app/callback?state=a&state=b&code=x"
                    .parse()
                    .unwrap()
            )
            .is_err()
        );
    }

    #[test]
    fn callback_rejects_code_and_error_together() {
        assert!(
            parse_callback(
                "https://app/callback?state=s&code=x&error=no"
                    .parse()
                    .unwrap()
            )
            .is_err()
        );
    }

    #[test]
    fn callback_decodes_issuer() {
        let callback = parse_callback(
            "https://app/callback?state=s&code=x&iss=https%3A%2F%2Fissuer.example%2Fa%3Fb%3Dc"
                .parse()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            callback.issuer.as_deref(),
            Some("https://issuer.example/a?b=c")
        );
    }
}
