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

use crate::app::context::ApplicationContext;
use crate::command::shared_args::PostDeployArgs;
use crate::command_handler::repl::rust::RustRepl;
use crate::command_handler::repl::typescript::TypeScriptRepl;
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::fs;
use crate::model::app::{ApplicationComponentSelectMode, BuildConfig};
use crate::model::app_raw::{BuiltinServer, Server};
use crate::model::component::ComponentNameMatchKind;
use crate::model::deploy::DeployConfig;
use crate::model::environment::EnvironmentResolveMode;
use crate::model::repl::{BridgeReplArgs, ReplLanguage, ReplMetadata, ReplScriptSource};
use crate::model::GuestLanguage;
use anyhow::bail;
use golem_common::model::component::ComponentName;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

mod rust;
mod typescript;

#[derive(Clone)]
pub struct ReplHandler {
    ctx: Arc<Context>,
}

impl ReplHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn cmd_repl(
        &self,
        language: Option<ReplLanguage>,
        _component_name: Option<ComponentName>,
        _component_revision: Option<golem_common::model::component::ComponentRevision>,
        post_deploy_args: Option<&PostDeployArgs>,
        script: Option<String>,
        script_file: Option<PathBuf>,
        stream_logs: bool,
        disable_auto_imports: bool,
    ) -> anyhow::Result<()> {
        let script = {
            if let Some(script) = script {
                Some(ReplScriptSource::Inline(script))
            } else if let Some(script_path) = script_file {
                Some(ReplScriptSource::FromFile(fs::canonicalize_path(
                    &script_path,
                )?))
            } else {
                None
            }
        };

        self.bridge_repl(
            language.map(|l| l.to_guest_language()),
            script,
            stream_logs,
            disable_auto_imports,
            post_deploy_args,
        )
        .await
    }

    async fn bridge_repl(
        &self,
        language: Option<GuestLanguage>,
        script: Option<ReplScriptSource>,
        stream_logs: bool,
        disable_auto_imports: bool,
        post_deploy_args: Option<&PostDeployArgs>,
    ) -> anyhow::Result<()> {
        let language = match language {
            Some(language) => language,
            None => {
                let app_ctx = self.ctx.app_context_lock().await;
                let app_ctx = app_ctx.some_or_err()?;
                let languages = app_ctx
                    .application()
                    .component_names()
                    .filter_map(|component_name| {
                        app_ctx
                            .application()
                            .component(component_name)
                            .guess_language()
                    })
                    .collect::<HashSet<_>>();

                if languages.len() == 1 {
                    *languages.iter().next().unwrap()
                } else {
                    self.ctx
                        .interactive_handler()
                        .select_repl_language(&languages)?
                }
            }
        };

        let environment = self
            .ctx
            .environment_handler()
            .resolve_environment(EnvironmentResolveMode::ManifestOnly)
            .await?;

        let args = {
            let app_main_dir = fs::canonicalize_path(&std::env::current_dir()?)?;

            let app_ctx = self.ctx.app_context_lock().await;
            let app_ctx = app_ctx.some_or_err()?;

            let repl_root_dir = app_ctx.application().repl_root_dir(language);
            fs::create_dir_all(&repl_root_dir)?;
            let repl_root_dir = fs::canonicalize_path(&repl_root_dir)?;

            let repl_bridge_sdk_target = app_ctx.new_repl_bridge_sdk_target(language);
            let repl_root_bridge_sdk_dir = repl_bridge_sdk_target
                .output_dir
                .clone()
                .expect("Missing target dir");
            fs::create_dir_all(&repl_root_bridge_sdk_dir)?;
            let repl_root_bridge_sdk_dir = fs::canonicalize_path(&repl_root_bridge_sdk_dir)?;

            let repl_history_file_path = app_ctx.application().repl_history_file(language.into());
            if !repl_history_file_path.exists() {
                fs::write(&repl_history_file_path, "")?;
            }
            let repl_history_file_path = fs::canonicalize_path(&repl_history_file_path)?;

            let repl_cli_commands_metadata_json_path = app_ctx
                .application()
                .repl_cli_commands_metadata_json(language);
            // TODO: cleanup
            if !repl_cli_commands_metadata_json_path.exists() {
                fs::write(&repl_cli_commands_metadata_json_path, "")?;
            }
            let repl_cli_commands_metadata_json_path =
                fs::canonicalize_path(&repl_cli_commands_metadata_json_path)?;

            let repl_metadata_json_path = app_ctx.application().repl_metadata_json(language);
            // TODO: cleanup
            if !repl_metadata_json_path.exists() {
                fs::write(&repl_metadata_json_path, "")?;
            }
            let repl_metadata_json_path = fs::canonicalize_path(&repl_metadata_json_path)?;

            let component_names = app_ctx.application().component_names().cloned().collect();

            BridgeReplArgs {
                environment,
                component_names,
                script,
                stream_logs,
                disable_auto_imports,
                app_main_dir,
                repl_root_dir,
                repl_root_bridge_sdk_dir,
                repl_bridge_sdk_target,
                repl_history_file_path,
                repl_cli_commands_metadata_json_path,
                repl_metadata_json_path,
            }
        };

        match post_deploy_args {
            Some(post_deploy_args) => {
                self.ctx
                    .app_handler()
                    .deploy(DeployConfig {
                        plan: false,
                        stage: false,
                        approve_staging_steps: false,
                        force_build: None,
                        post_deploy_args: post_deploy_args.clone(),
                        repl_bridge_sdk_target: Some(language),
                        skip_build: false,
                    })
                    .await?;
            }
            None => {
                // We explicitly trigger 'build', so we ensure that we have the REPL bridge SDKs
                self.ctx
                    .app_handler()
                    .build(
                        &BuildConfig::new()
                            .with_repl_bridge_sdk_target(args.repl_bridge_sdk_target.clone()),
                        vec![],
                        &ApplicationComponentSelectMode::All,
                    )
                    .await?;

                // We check for all components, but in practice we usually only have one, and the
                // first missing one will trigger deployment for all components. We also skip
                // building, as that was already done above.
                for component_name in &args.component_names {
                    self.ctx
                        .component_handler()
                        .component_by_name_with_auto_deploy(
                            &args.environment,
                            ComponentNameMatchKind::App,
                            component_name,
                            None,
                            post_deploy_args,
                            Some(language),
                            true,
                        )
                        .await?;
                }
            }
        }

        match language {
            GuestLanguage::Rust => RustRepl::new(self.ctx.clone()).run(args).await,
            GuestLanguage::TypeScript => TypeScriptRepl::new(self.ctx.clone()).run(args).await,
        }
    }

    async fn repl_server_env_vars(&self) -> anyhow::Result<HashMap<String, String>> {
        let Some(environment) = self.ctx.manifest_environment() else {
            bail!("REPL requires a manifest environment to be used.");
        };

        let mut env_vars = HashMap::new();

        env_vars.insert(
            "GOLEM_REPL_APPLICATION".to_string(),
            environment.application_name.0.clone(),
        );
        env_vars.insert(
            "GOLEM_REPL_ENVIRONMENT".to_string(),
            environment.environment_name.0.clone(),
        );

        match environment.environment.server.as_ref() {
            Some(Server::Builtin(BuiltinServer::Local)) | None => {
                env_vars.insert("GOLEM_REPL_SERVER_KIND".to_string(), "local".to_string());
            }
            Some(Server::Builtin(BuiltinServer::Cloud)) => {
                env_vars.insert("GOLEM_REPL_SERVER_KIND".to_string(), "cloud".to_string());
                env_vars.insert(
                    "GOLEM_REPL_SERVER_TOKEN".to_string(),
                    self.ctx.auth_token().await?.into_secret(),
                );
            }
            Some(Server::Custom(custom)) => {
                env_vars.insert("GOLEM_REPL_SERVER_KIND".to_string(), "custom".to_string());
                env_vars.insert(
                    "GOLEM_REPL_SERVER_CUSTOM_URL".to_string(),
                    custom.url.to_string(),
                );
                env_vars.insert(
                    "GOLEM_REPL_SERVER_TOKEN".to_string(),
                    self.ctx.auth_token().await?.into_secret(),
                );
            }
        }

        Ok(env_vars)
    }
}

async fn load_repl_metadata(
    app_ctx: &ApplicationContext,
    language: GuestLanguage,
) -> anyhow::Result<ReplMetadata> {
    let metadata = serde_json::from_str(&fs::read_to_string(
        app_ctx.application().repl_metadata_json(language),
    )?)?;
    Ok(metadata)
}
