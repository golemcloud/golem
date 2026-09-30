// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

use crate::command::api::McpImportSubcommand;
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::service::MapServiceError;
use crate::model::app::Application;
use crate::model::environment::EnvironmentResolveMode;
use crate::model::mcp::{
    McpImportAuthorization, McpImportAuthorizeView, McpImportCompleteView, McpImportDisconnectView,
    McpImportOAuthStatus, McpImportStatusView, McpImportToolsView,
};
use anyhow::Context as _;
use golem_client::api::{DeploymentClient, EnvironmentClient};
use golem_client::model::McpImportOAuthCallback;
use golem_common::model::deployment::DeploymentRevision;
use std::sync::Arc;

pub async fn handle(ctx: &Arc<Context>, command: McpImportSubcommand) -> anyhow::Result<()> {
    if matches!(
        &command,
        McpImportSubcommand::Authorize { manifest: true, .. }
            | McpImportSubcommand::Complete { manifest: true, .. }
            | McpImportSubcommand::Status { manifest: true, .. }
            | McpImportSubcommand::Disconnect { manifest: true, .. }
    ) {
        return handle_declared(ctx, command).await;
    }
    let environment = ctx
        .environment_handler()
        .resolve_environment(EnvironmentResolveMode::Any)
        .await?;
    let (index, requested) = match &command {
        McpImportSubcommand::Tools {
            import_index,
            revision,
        }
        | McpImportSubcommand::Refresh {
            import_index,
            revision,
        }
        | McpImportSubcommand::Authorize {
            import_index,
            revision,
            ..
        }
        | McpImportSubcommand::Complete {
            import_index,
            revision,
            ..
        }
        | McpImportSubcommand::Status {
            import_index,
            revision,
            ..
        }
        | McpImportSubcommand::Disconnect {
            import_index,
            revision,
            ..
        } => (*import_index, *revision),
    };
    let revision = resolve_revision(requested, || {
        Ok(environment.current_deployment_or_err()?.deployment_revision)
    })?;
    let clients = ctx.golem_clients().await?;
    match command {
        McpImportSubcommand::Tools { .. } => {
            let result = clients
                .deployment
                .get_mcp_import_tools(&environment.environment_id.0, revision.get(), index)
                .await
                .map_service_error()?;
            ctx.log_handler().log_output(McpImportToolsView(result))?;
        }
        McpImportSubcommand::Refresh { .. } => {
            let result = clients
                .deployment
                .refresh_mcp_import_tools(&environment.environment_id.0, revision.get(), index)
                .await
                .map_service_error()?;
            ctx.log_handler().log_output(McpImportToolsView(result))?;
        }
        McpImportSubcommand::Authorize { .. } => {
            let result = clients
                .deployment
                .authorize_mcp_import(&environment.environment_id.0, revision.get(), index)
                .await
                .map_service_error()?;
            ctx.log_handler()
                .log_output(McpImportAuthorizeView(result.into()))?;
        }
        McpImportSubcommand::Complete { callback_url, .. } => {
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
                .log_output(McpImportCompleteView(result.into()))?;
        }
        McpImportSubcommand::Status { .. } => {
            let result = clients
                .deployment
                .get_mcp_import_authorization_status(
                    &environment.environment_id.0,
                    revision.get(),
                    index,
                )
                .await
                .map_service_error()?;
            ctx.log_handler()
                .log_output(McpImportStatusView(result.into()))?;
        }
        McpImportSubcommand::Disconnect { .. } => {
            let result = clients
                .deployment
                .disconnect_mcp_import(&environment.environment_id.0, revision.get(), index)
                .await
                .map_service_error()?;
            ctx.log_handler()
                .log_output(McpImportDisconnectView(result.into()))?;
        }
    }
    Ok(())
}

async fn handle_declared(ctx: &Arc<Context>, command: McpImportSubcommand) -> anyhow::Result<()> {
    let index = match &command {
        McpImportSubcommand::Authorize { import_index, .. }
        | McpImportSubcommand::Complete { import_index, .. }
        | McpImportSubcommand::Status { import_index, .. }
        | McpImportSubcommand::Disconnect { import_index, .. } => *import_index,
        _ => unreachable!(),
    };
    let import = {
        let app_ctx = ctx.app_context_lock().await;
        let app_ctx = app_ctx.some_or_err()?;
        declared_import(app_ctx.application(), index)?
    };
    let environment = ctx
        .environment_handler()
        .resolve_environment(EnvironmentResolveMode::ManifestOnly)
        .await?;
    let clients = ctx.golem_clients().await?;
    let environment_id = &environment.environment_id.0;
    let request = golem_client::model::DeclaredMcpImportOAuthRequest { import };
    match command {
        McpImportSubcommand::Authorize { .. } => {
            let result = clients
                .environment
                .authorize_declared_mcp_import(environment_id, &request)
                .await
                .map_service_error()?;
            ctx.log_handler()
                .log_output(McpImportAuthorizeView(McpImportAuthorization {
                    authorization_url: result.authorization_url,
                    import_index: index,
                    deployment_revision: None,
                }))?;
        }
        McpImportSubcommand::Complete { callback_url, .. } => {
            let request = golem_client::model::CompleteDeclaredMcpImportOAuthRequest {
                import: request.import,
                callback: parse_callback(callback_url.into_inner())?,
            };
            let result = clients
                .environment
                .complete_declared_mcp_import_authorization(environment_id, &request)
                .await
                .map_service_error()?;
            ctx.log_handler()
                .log_output(McpImportCompleteView(McpImportOAuthStatus::declared(
                    result, index,
                )))?;
        }
        McpImportSubcommand::Status { .. } => {
            let result = clients
                .environment
                .get_declared_mcp_import_authorization_status(environment_id, &request)
                .await
                .map_service_error()?;
            ctx.log_handler()
                .log_output(McpImportStatusView(McpImportOAuthStatus::declared(
                    result, index,
                )))?;
        }
        McpImportSubcommand::Disconnect { .. } => {
            let result = clients
                .environment
                .disconnect_declared_mcp_import(environment_id, &request)
                .await
                .map_service_error()?;
            ctx.log_handler().log_output(McpImportDisconnectView(
                McpImportOAuthStatus::declared(result, index),
            ))?;
        }
        _ => unreachable!(),
    }
    Ok(())
}

fn declared_import(
    application: &Application,
    index: u32,
) -> anyhow::Result<golem_common::model::mcp_import::McpImportDeployment> {
    let import = application
        .mcp_imports(application.environment_name())
        .and_then(|imports| imports.get(index as usize))
        .cloned()
        .with_context(|| {
            format!(
                "MCP import {index} is not declared for environment '{}'",
                application.environment_name()
            )
        })?;
    crate::command_handler::app::resolve_mcp_import_env_vars(import, index as usize)
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
    fn manifest_oauth_commands_reject_revision_and_preserve_callback() {
        use crate::command::api::{ApiSubcommand, McpImportSubcommand};
        use crate::command::{GolemCliCommand, GolemCliSubcommand};
        use clap::Parser;

        for action in ["authorize", "complete", "status", "disconnect"] {
            let mut args = vec!["golem", "api", "mcp-import", action, "3", "--manifest"];
            if action == "complete" {
                args.push("https://callback.example/?state=s&code=c");
            }
            let parsed = GolemCliCommand::try_parse_from(&args).unwrap();
            let GolemCliSubcommand::Api {
                subcommand: ApiSubcommand::McpImport { subcommand },
            } = parsed.subcommand
            else {
                panic!("expected MCP command")
            };
            let (index, revision, manifest) = match subcommand {
                McpImportSubcommand::Authorize {
                    import_index,
                    revision,
                    manifest,
                } if action == "authorize" => (import_index, revision, manifest),
                McpImportSubcommand::Status {
                    import_index,
                    revision,
                    manifest,
                } if action == "status" => (import_index, revision, manifest),
                McpImportSubcommand::Disconnect {
                    import_index,
                    revision,
                    manifest,
                } if action == "disconnect" => (import_index, revision, manifest),
                McpImportSubcommand::Complete {
                    import_index,
                    revision,
                    manifest,
                    callback_url,
                } if action == "complete" => {
                    let callback = parse_callback(callback_url.into_inner()).unwrap();
                    assert_eq!(callback.state, "s");
                    assert_eq!(callback.code.as_deref(), Some("c"));
                    (import_index, revision, manifest)
                }
                other => panic!("wrong action: {other:?}"),
            };
            assert_eq!(index, 3);
            assert!(revision.is_none());
            assert!(manifest);
            args.extend(["--revision", "42"]);
            assert!(GolemCliCommand::try_parse_from(args).is_err());
        }
    }

    #[test]
    fn declared_oauth_selects_exact_manifest_environment_and_index() {
        use crate::model::app::{Application, ComponentPresetSelector};
        use crate::model::app_raw::ApplicationWithSource;

        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join("golem.yaml");
        crate::fs::write(
            &manifest,
            r#"
app: consent
environments:
  local:
    server: local
  staging:
    server: local
components:
  app:main:
    componentWasm: main.wasm
mcp:
  imports:
    local:
      - url: https://wrong.example/mcp
    staging:
      - url: https://first.example/mcp
      - url: https://selected.example/mcp
        securityScheme: selected-oauth
"#,
        )
        .unwrap();
        let apps = vec![ApplicationWithSource::from_yaml_file(&manifest).unwrap()];
        let (preload, _, errors) = Application::preload_from_raw_apps(&apps).into_product();
        assert!(errors.is_empty(), "{errors:?}");
        let preload = preload.unwrap();
        let (application, _, errors) = Application::from_raw_apps(
            dir.path().into(),
            preload.application_name,
            preload.environments,
            preload.local_server,
            ComponentPresetSelector {
                environment: "staging".parse().unwrap(),
                presets: vec![],
            },
            apps,
        )
        .into_product();
        assert!(errors.is_empty(), "{errors:?}");
        let application = application.unwrap();
        let selected = super::declared_import(&application, 1).unwrap();
        assert_eq!(selected.url, "https://selected.example/mcp");
        assert_eq!(
            selected.security_scheme.unwrap().to_string(),
            "selected-oauth"
        );
        assert!(super::declared_import(&application, 2).is_err());
    }

    #[test]
    fn inspection_commands_preserve_explicit_import_and_revision() {
        use crate::command::api::{ApiSubcommand, McpImportSubcommand};
        use crate::command::{GolemCliCommand, GolemCliSubcommand};
        use clap::Parser;

        for action in ["tools", "refresh"] {
            let parsed = <GolemCliCommand as Parser>::try_parse_from([
                "golem",
                "api",
                "mcp-import",
                action,
                "3",
                "--revision",
                "42",
            ])
            .unwrap();
            let GolemCliSubcommand::Api {
                subcommand: ApiSubcommand::McpImport { subcommand },
            } = parsed.subcommand
            else {
                panic!("expected MCP import command");
            };
            let (import_index, revision) = match subcommand {
                McpImportSubcommand::Tools {
                    import_index,
                    revision,
                } if action == "tools" => (import_index, revision),
                McpImportSubcommand::Refresh {
                    import_index,
                    revision,
                } if action == "refresh" => (import_index, revision),
                other => panic!("wrong action: {other:?}"),
            };
            assert_eq!(import_index, 3);
            assert_eq!(revision.unwrap().get(), 42);
        }
    }

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
