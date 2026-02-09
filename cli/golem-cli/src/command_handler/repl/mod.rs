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
use crate::command_handler::repl::rib::CliRibRepl;
use crate::command_handler::repl::rust::RustRepl;
use crate::command_handler::repl::typescript::TypeScriptRepl;
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::fs;
use crate::model::app::{ApplicationComponentSelectMode, BuildConfig};
use crate::model::app_raw::{BuiltinServer, Server};
use crate::model::component::ComponentNameMatchKind;
use crate::model::environment::EnvironmentResolveMode;
use crate::model::repl::{BridgeReplArgs, ReplLanguage, ReplMetadata};
use ::rib::{ComponentDependency, ComponentDependencyKey, CustomInstanceSpec, InterfaceName};
use anyhow::{anyhow, bail};
use golem_common::model::component::{ComponentName, ComponentRevision};
use golem_rib_repl::ReplComponentDependencies;
use golem_templates::model::GuestLanguage;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

mod rib;
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
        component_name: Option<ComponentName>,
        component_revision: Option<ComponentRevision>,
        post_deploy_args: Option<&PostDeployArgs>,
        script: Option<String>,
        script_file: Option<PathBuf>,
        stream_logs: bool,
    ) -> anyhow::Result<()> {
        let script_input = {
            if let Some(script) = script {
                Some(script)
            } else if let Some(script_path) = script_file {
                Some(fs::read_to_string(script_path)?)
            } else {
                None
            }
        };

        if language.is_some_and(|l| l.is_rib()) {
            self.rib_repl(
                component_name,
                component_revision,
                post_deploy_args,
                script_input,
                stream_logs,
            )
            .await
        } else {
            if component_name.is_some() {
                bail!("Component name and revision is only supported for Rib REPL.");
            }

            if post_deploy_args.is_some() {
                bail!("Deploy arguments are only supported for Rib REPL.");
            }

            self.bridge_repl(
                language.and_then(|l| l.to_guest_language()),
                script_input,
                stream_logs,
            )
            .await
        }
    }

    async fn bridge_repl(
        &self,
        language: Option<GuestLanguage>,
        script: Option<String>,
        stream_logs: bool,
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

            BridgeReplArgs {
                environment,
                script,
                stream_logs,
                repl_root_dir,
                repl_root_bridge_sdk_dir,
                repl_bridge_sdk_target,
                repl_history_file_path,
                repl_cli_commands_metadata_json_path,
            }
        };

        self.ctx
            .app_handler()
            .build(
                &BuildConfig::new()
                    .with_repl_bridge_sdk_target(args.repl_bridge_sdk_target.clone()),
                vec![],
                &ApplicationComponentSelectMode::All,
            )
            .await?;

        match language {
            GuestLanguage::Rust => RustRepl::new(self.ctx.clone()).run(args).await,
            GuestLanguage::TypeScript => TypeScriptRepl::new(self.ctx.clone()).run(args).await,
        }
    }

    async fn rib_repl(
        &self,
        component_name: Option<ComponentName>,
        component_revision: Option<ComponentRevision>,
        post_deploy_args: Option<&PostDeployArgs>,
        script: Option<String>,
        stream_logs: bool,
    ) -> anyhow::Result<()> {
        let selected_components = self
            .ctx
            .component_handler()
            .must_select_components_by_app_dir_or_name(component_name.as_ref())
            .await?;

        let component_name = {
            if selected_components.component_names.len() == 1 {
                selected_components.component_names[0].clone()
            } else {
                self.ctx
                    .interactive_handler()
                    .select_component_for_repl(selected_components.component_names.clone())?
            }
        };

        // NOTE: we pre-create the ReplDependencies, because trying to do it in RibDependencyManager::get_dependencies
        //       results in thread safety errors on the path when cargo component could be called for client building
        let component = self
            .ctx
            .component_handler()
            .component_by_name_with_auto_deploy(
                &selected_components.environment,
                ComponentNameMatchKind::App,
                &component_name,
                component_revision.map(|r| r.into()),
                post_deploy_args,
            )
            .await?;

        let component_dependency_key = ComponentDependencyKey {
            component_name: component.component_name.0.clone(),
            component_id: component.id.0,
            component_revision: component.revision.into(),
            root_package_name: component.metadata.root_package_name().clone(),
            root_package_version: component.metadata.root_package_version().clone(),
        };

        // The REPL has to know about the custom instance parameters
        // to support creating instances using agent interface names.
        let mut custom_instance_spec = Vec::new();

        for agent_type in component.metadata.agent_types() {
            let wrapper_function = component
                .metadata
                .find_wrapper_function_by_agent_constructor(&agent_type.type_name)
                .map_err(|err| anyhow!(err))?
                .ok_or_else(|| {
                    anyhow!(
                        "Missing static WIT wrapper for constructor of agent type {}",
                        agent_type.type_name
                    )
                })?;

            custom_instance_spec.push(CustomInstanceSpec {
                instance_name: agent_type.wrapper_type_name(),
                parameter_types: wrapper_function
                    .analysed_export
                    .parameters
                    .iter()
                    .map(|p| p.typ.clone())
                    .collect(),
                interface_name: Some(InterfaceName {
                    name: agent_type.wrapper_type_name(),
                    version: None,
                }),
            });
        }

        CliRibRepl::new(
            self.ctx.clone(),
            stream_logs,
            ReplComponentDependencies {
                component_dependencies: vec![ComponentDependency::new(
                    component_dependency_key,
                    component.metadata.exports().to_vec(),
                )],
                custom_instance_spec,
            },
            component,
        )
        .run(script)
        .await
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
