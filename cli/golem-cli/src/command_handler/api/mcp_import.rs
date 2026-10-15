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
use anyhow::{Context as _, bail};
use golem_client::api::{DeploymentClient, EnvironmentClient};
use golem_client::model::McpImportOAuthCallback;
use golem_common::model::deployment::{DeploymentPlanMcpImportEntry, DeploymentRevision};
use golem_common::model::diff::{Hash, Hashable};
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
    let (selector, requested) = match &command {
        McpImportSubcommand::Tools { import, revision }
        | McpImportSubcommand::Refresh { import, revision }
        | McpImportSubcommand::Authorize {
            import, revision, ..
        }
        | McpImportSubcommand::Complete {
            import, revision, ..
        }
        | McpImportSubcommand::Status {
            import, revision, ..
        }
        | McpImportSubcommand::Disconnect {
            import, revision, ..
        } => (import.clone(), *revision),
    };
    let (import, import_name) = {
        let app_ctx = ctx.app_context_lock().await;
        let app_ctx = app_ctx.some_or_err().context(
            "MCP import commands require a local manifest for the selected environment; select an environment name from the manifest",
        )?;
        declared_import(app_ctx.application(), &selector)?
    };
    let environment = ctx
        .environment_handler()
        .resolve_environment(EnvironmentResolveMode::ManifestOnly)
        .await?;
    let revision = resolve_revision(requested, || {
        Ok(environment.current_deployment_or_err()?.deployment_revision)
    })?;
    let clients = ctx.golem_clients().await?;
    let (descriptor, _) = import
        .into_parts(environment.environment_id)
        .map_err(anyhow::Error::msg)?;
    let summary = clients
        .environment
        .get_deployment_summary(&environment.environment_id.0, revision.get())
        .await
        .map_service_error()?;
    let index = deployed_import_index(&summary.mcp_imports, &descriptor.hash()?, revision)?;
    match command {
        McpImportSubcommand::Tools { .. } => {
            let result = clients
                .deployment
                .get_mcp_import_tools(&environment.environment_id.0, revision.get(), index)
                .await
                .map_service_error()?;
            ctx.log_handler()
                .log_output(McpImportToolsView((result, import_name).into()))?;
        }
        McpImportSubcommand::Refresh { .. } => {
            let result = clients
                .deployment
                .refresh_mcp_import_tools(&environment.environment_id.0, revision.get(), index)
                .await
                .map_service_error()?;
            ctx.log_handler()
                .log_output(McpImportToolsView((result, import_name).into()))?;
        }
        McpImportSubcommand::Authorize { .. } => {
            let result = clients
                .deployment
                .authorize_mcp_import(&environment.environment_id.0, revision.get(), index)
                .await
                .map_service_error()?;
            ctx.log_handler()
                .log_output(McpImportAuthorizeView((result, import_name).into()))?;
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
                .log_output(McpImportCompleteView((result, import_name).into()))?;
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
                .log_output(McpImportStatusView((result, import_name).into()))?;
        }
        McpImportSubcommand::Disconnect { .. } => {
            let result = clients
                .deployment
                .disconnect_mcp_import(&environment.environment_id.0, revision.get(), index)
                .await
                .map_service_error()?;
            ctx.log_handler()
                .log_output(McpImportDisconnectView((result, import_name).into()))?;
        }
    }
    Ok(())
}

async fn handle_declared(ctx: &Arc<Context>, command: McpImportSubcommand) -> anyhow::Result<()> {
    let selector = match &command {
        McpImportSubcommand::Authorize { import, .. }
        | McpImportSubcommand::Complete { import, .. }
        | McpImportSubcommand::Status { import, .. }
        | McpImportSubcommand::Disconnect { import, .. } => import.clone(),
        _ => unreachable!(),
    };
    let (import, import_name) = {
        let app_ctx = ctx.app_context_lock().await;
        let app_ctx = app_ctx.some_or_err()?;
        declared_import(app_ctx.application(), &selector)?
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
                    import: import_name,
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
                    result,
                    import_name,
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
                    result,
                    import_name,
                )))?;
        }
        McpImportSubcommand::Disconnect { .. } => {
            let result = clients
                .environment
                .disconnect_declared_mcp_import(environment_id, &request)
                .await
                .map_service_error()?;
            ctx.log_handler().log_output(McpImportDisconnectView(
                McpImportOAuthStatus::declared(result, import_name),
            ))?;
        }
        _ => unreachable!(),
    }
    Ok(())
}

fn declared_import(
    application: &Application,
    selector: &str,
) -> anyhow::Result<(
    golem_common::model::mcp_import::McpImportDeployment,
    Option<String>,
)> {
    let imports = application
        .mcp_imports(application.environment_name())
        .map(Vec::as_slice)
        .unwrap_or_default();
    let index = select_import(imports, selector).with_context(|| {
        format!(
            "Selecting an MCP import in environment '{}'",
            application.environment_name()
        )
    })?;
    let import = crate::command_handler::app::resolve_mcp_import_env_vars(
        imports[index].import.clone(),
        index,
    )?;
    Ok((import, imports[index].name.clone()))
}

fn select_import(
    imports: &[crate::model::app_raw::McpImport],
    selector: &str,
) -> anyhow::Result<usize> {
    if let Some(index) = imports
        .iter()
        .position(|entry| entry.name.as_deref() == Some(selector))
    {
        return Ok(index);
    }
    let renderer = crate::command_handler::template::EnvVarRenderer::new();
    let mut matches = Vec::new();
    let mut failures = Vec::new();
    for (index, entry) in imports.iter().enumerate() {
        match renderer.render_str(&entry.import.url) {
            Ok(url) if url == selector => matches.push(index),
            Ok(_) => {}
            Err(error) => {
                let missing = renderer.missing_env_vars(&entry.import.url, &error);
                failures.push(format!(
                    "import at position {index}: {}",
                    if missing.is_empty() {
                        "URL template cannot be rendered".into()
                    } else {
                        format!("missing environment variables: {}", missing.join(", "))
                    }
                ));
            }
        }
    }
    match matches.as_slice() {
        [index] => Ok(*index),
        [] if !failures.is_empty() => bail!(
            "No MCP import matches the selector. Cannot resolve some import URLs: {}. Supply their environment variables, correct their templates, or select an explicit import name",
            failures.join("; ")
        ),
        [] => {
            let names = imports
                .iter()
                .filter_map(|entry| entry.name.as_deref())
                .collect::<Vec<_>>();
            bail!(
                "No MCP import matches the selector. Use a declared name or exact full URL. Available names: {}",
                names.join(", ")
            )
        }
        _ => bail!("Multiple MCP imports use this URL; add distinct names and select one"),
    }
}

fn deployed_import_index(
    imports: &[DeploymentPlanMcpImportEntry],
    hash: &Hash,
    revision: DeploymentRevision,
) -> anyhow::Result<u32> {
    let matches = imports
        .iter()
        .filter(|entry| &entry.hash == hash)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [entry] => Ok(entry.index),
        [] => bail!(
            "The selected MCP import configuration does not match deployment revision {revision}; use the declaration and environment values for that revision, deploy the changed configuration, or use --manifest for pre-deployment OAuth"
        ),
        _ => bail!(
            "Deployment revision {revision} contains identical MCP import configurations; CLI names cannot distinguish them. Deploy distinct configurations before selecting this import"
        ),
    }
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
    use crate::model::mcp::{McpImportAuthorization, McpImportAuthorizeView};
    use golem_common::model::deployment::DeploymentRevision;
    use test_r::test;

    #[test]
    fn manifest_oauth_commands_reject_revision_and_preserve_callback() {
        use crate::command::api::{ApiSubcommand, McpImportSubcommand};
        use crate::command::{GolemCliCommand, GolemCliSubcommand};
        use clap::Parser;

        for action in ["authorize", "complete", "status", "disconnect"] {
            let mut args = vec![
                "golem",
                "api",
                "mcp-import",
                action,
                "catalog",
                "--manifest",
            ];
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
            let (import, revision, manifest) = match subcommand {
                McpImportSubcommand::Authorize {
                    import,
                    revision,
                    manifest,
                } if action == "authorize" => (import, revision, manifest),
                McpImportSubcommand::Status {
                    import,
                    revision,
                    manifest,
                } if action == "status" => (import, revision, manifest),
                McpImportSubcommand::Disconnect {
                    import,
                    revision,
                    manifest,
                } if action == "disconnect" => (import, revision, manifest),
                McpImportSubcommand::Complete {
                    import,
                    revision,
                    manifest,
                    callback_url,
                } if action == "complete" => {
                    let callback = parse_callback(callback_url.into_inner()).unwrap();
                    assert_eq!(callback.state, "s");
                    assert_eq!(callback.code.as_deref(), Some("c"));
                    (import, revision, manifest)
                }
                other => panic!("wrong action: {other:?}"),
            };
            assert_eq!(import, "catalog");
            assert!(revision.is_none());
            assert!(manifest);
            args.extend(["--revision", "42"]);
            assert!(GolemCliCommand::try_parse_from(args).is_err());
        }
    }

    #[test]
    fn declared_oauth_selects_exact_manifest_environment_and_name() {
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
      - name: selected
        url: https://selector-user:selector-password@selected.example/selector-path?token=selector-query#selector-fragment
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
        let (selected, name) = super::declared_import(&application, "selected").unwrap();
        assert_eq!(name.as_deref(), Some("selected"));
        let url = "https://selector-user:selector-password@selected.example/selector-path?token=selector-query#selector-fragment";
        assert_eq!(selected.url, url);
        assert_eq!(
            selected.security_scheme.unwrap().to_string(),
            "selected-oauth"
        );
        let (selected_by_url, name) = super::declared_import(&application, url).unwrap();
        assert_eq!(selected_by_url.url, url);
        let view = McpImportAuthorizeView(McpImportAuthorization {
            authorization_url: "https://provider.example/authorize?state=consent".into(),
            import: name,
            deployment_revision: None,
        });
        use crate::model::text_format::MessageWithFields;
        let output = format!(
            "{:?} {:?} {}",
            view,
            view.fields(),
            serde_json::to_string(&view).unwrap()
        );
        assert!(output.contains("selected"));
        for secret in [
            "selector-user",
            "selector-password",
            "selector-path",
            "selector-query",
            "selector-fragment",
        ] {
            assert!(!output.contains(secret));
        }
        let (first, name) =
            super::declared_import(&application, "https://first.example/mcp").unwrap();
        assert!(name.is_none());
        assert_eq!(first.url, "https://first.example/mcp");
        assert!(super::declared_import(&application, "0").is_err());
        assert!(super::declared_import(&application, "https://wrong.example/mcp").is_err());
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
                "https://catalog.example/mcp?scope=read&mode=full",
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
            let (import, revision) = match subcommand {
                McpImportSubcommand::Tools { import, revision } if action == "tools" => {
                    (import, revision)
                }
                McpImportSubcommand::Refresh { import, revision } if action == "refresh" => {
                    (import, revision)
                }
                other => panic!("wrong action: {other:?}"),
            };
            assert_eq!(import, "https://catalog.example/mcp?scope=read&mode=full");
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
    fn import_selectors_are_stable_and_deployed_matching_fails_closed() {
        use super::{deployed_import_index, select_import};
        use golem_common::model::deployment::DeploymentPlanMcpImportEntry;
        use golem_common::model::diff::Hashable;
        use golem_common::model::environment::EnvironmentId;

        let mut imports: Vec<crate::model::app_raw::McpImport> = serde_json::from_value(serde_json::json!([
            {"name": "read", "url": "https://shared.example/mcp", "include": ["read_*"], "auth": {"bearer": "secret-one"}},
            {"name": "write", "url": "https://shared.example/mcp", "include": ["write_*"], "auth": {"bearer": "secret-two"}},
            {"url": "https://unique.example/mcp?mode=read&limit=7"}
        ])).unwrap();
        assert_eq!(select_import(&imports, "write").unwrap(), 1);
        assert_eq!(
            select_import(&imports, "https://unique.example/mcp?mode=read&limit=7").unwrap(),
            2
        );
        assert!(
            select_import(&imports, "https://shared.example/mcp")
                .unwrap_err()
                .to_string()
                .contains("distinct names")
        );
        assert!(select_import(&imports, "0").is_err());
        let environment = EnvironmentId::new();
        let hash = imports[1]
            .import
            .clone()
            .into_parts(environment)
            .unwrap()
            .0
            .hash()
            .unwrap();
        let revision = DeploymentRevision::try_from(42_u64).unwrap();
        let entries = vec![DeploymentPlanMcpImportEntry { index: 9, hash }];
        imports.reverse();
        let selected = &imports[select_import(&imports, "write").unwrap()];
        let selected_hash = selected
            .import
            .clone()
            .into_parts(environment)
            .unwrap()
            .0
            .hash()
            .unwrap();
        assert_eq!(
            deployed_import_index(&entries, &selected_hash, revision).unwrap(),
            9
        );
        let mut changed = selected.import.clone();
        changed.include = Some(vec!["delete_*".into()]);
        let changed_hash = changed.into_parts(environment).unwrap().0.hash().unwrap();
        assert!(deployed_import_index(&entries, &changed_hash, revision).is_err());
        let other_environment_hash = selected
            .import
            .clone()
            .into_parts(EnvironmentId::new())
            .unwrap()
            .0
            .hash()
            .unwrap();
        assert!(deployed_import_index(&entries, &other_environment_hash, revision).is_err());
        let duplicates = vec![
            DeploymentPlanMcpImportEntry {
                index: 4,
                hash: selected_hash,
            },
            DeploymentPlanMcpImportEntry {
                index: 9,
                hash: selected_hash,
            },
        ];
        assert!(
            deployed_import_index(&duplicates, &selected_hash, revision)
                .unwrap_err()
                .to_string()
                .contains("identical")
        );
    }

    #[test]
    fn url_selection_ignores_unresolved_candidates_but_reports_them_without_a_match() {
        let missing: crate::model::app_raw::McpImport = serde_json::from_value(serde_json::json!({
            "url": "https://secret-template.example/{{ GOL_875_MISSING_REVIEW_URL }}/private-token"
        }))
        .unwrap();
        let literal: crate::model::app_raw::McpImport = serde_json::from_value(serde_json::json!({
            "url": "https://selected.example/mcp"
        }))
        .unwrap();
        for (imports, expected) in [
            (vec![missing.clone(), literal.clone()], 1),
            (vec![literal.clone(), missing.clone()], 0),
        ] {
            assert_eq!(
                super::select_import(&imports, &literal.import.url).unwrap(),
                expected
            );
            let error = super::select_import(&imports, "https://absent.example/mcp")
                .unwrap_err()
                .to_string();
            assert!(error.contains("GOL_875_MISSING_REVIEW_URL"));
            assert!(!error.contains("secret-template"));
            assert!(!error.contains("private-token"));
        }
        assert!(
            super::select_import(
                &[missing, literal.clone(), literal.clone()],
                &literal.import.url
            )
            .unwrap_err()
            .to_string()
            .contains("distinct names")
        );
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
