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

use crate::bridge_gen::bridge_client_directory_name;
use crate::command::shared_args::DeployArgs;
use crate::command_handler::repl::rib::CliRibRepl;
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::fs;
use crate::model::app::{ApplicationComponentSelectMode, CustomBridgeSdkTarget};
use crate::model::app_raw::{BuiltinServer, Server};
use crate::model::component::ComponentNameMatchKind;
use crate::model::environment::{EnvironmentResolveMode, ResolvedEnvironmentIdentity};
use crate::model::repl::ReplLanguage;
use crate::process::{CommandExt, ExitStatusExt};
use ::rib::{ComponentDependency, ComponentDependencyKey, CustomInstanceSpec, InterfaceName};
use anyhow::{anyhow, bail};
use camino::Utf8PathBuf;
use golem_common::base_model::agent::AgentTypeName;
use golem_common::model::component::{ComponentName, ComponentRevision};
use golem_rib_repl::ReplComponentDependencies;
use golem_templates::model::GuestLanguage;
use indoc::{formatdoc, indoc};
use itertools::Itertools;
use serde_json::json;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::process::Command;

mod rib;

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
        deploy_args: Option<&DeployArgs>,
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
                deploy_args,
                script_input,
                stream_logs,
            )
            .await
        } else {
            if component_name.is_some() {
                bail!("Component name and revision is only supported for Rib REPL.");
            }

            if deploy_args.is_some() {
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
                    .application
                    .component_names()
                    .filter_map(|component_name| {
                        app_ctx
                            .application
                            .component(component_name)
                            .guess_language()
                    })
                    .collect::<HashSet<_>>();

                if languages.len() == 1 {
                    languages.iter().next().unwrap().clone()
                } else {
                    todo!("interactive language selection for REPL is not implemented")
                }
            }
        };

        let environment = self
            .ctx
            .environment_handler()
            .resolve_environment(EnvironmentResolveMode::ManifestOnly)
            .await?;

        let (repl_root_dir, repl_root_bridge_sdk_dir) = {
            let mut app_ctx = self.ctx.app_context_lock_mut().await?;
            let app_ctx = app_ctx.some_or_err_mut()?;
            let repl_root_dir = app_ctx.application.repl_root_dir(language);
            let repl_root_bridge_sdk_dir = app_ctx.application.repl_root_bridge_sdk_dir(language);
            app_ctx.custom_repl_bridge_sdk_target = Some(CustomBridgeSdkTarget {
                agent_type_names: Default::default(),
                target_language: Some(language),
                output_dir: Some(repl_root_bridge_sdk_dir.clone()),
            });
            (repl_root_dir, repl_root_bridge_sdk_dir)
        };

        self.ctx
            .app_handler()
            .build(vec![], None, &ApplicationComponentSelectMode::All)
            .await?;

        let agent_type_names = {
            let app_ctx = self.ctx.app_context_lock().await;
            let app_ctx = app_ctx.some_or_err()?;
            app_ctx.wit.get_all_extracted_agent_type_names().await
        };

        match language {
            GuestLanguage::Rust => {
                self.rust_repl(
                    environment,
                    script,
                    stream_logs,
                    repl_root_dir,
                    repl_root_bridge_sdk_dir,
                    agent_type_names,
                )
                .await?
            }
            GuestLanguage::TypeScript => {
                self.ts_repl(
                    environment,
                    script,
                    stream_logs,
                    repl_root_dir,
                    repl_root_bridge_sdk_dir,
                    agent_type_names,
                )
                .await?
            }
        }

        Ok(())
    }

    async fn ts_repl(
        &self,
        environment: ResolvedEnvironmentIdentity,
        script: Option<String>,
        stream_logs: bool,
        repl_root_dir: PathBuf,
        repl_root_bridge_sdk_dir: PathBuf,
        agent_type_names: Vec<AgentTypeName>,
    ) -> anyhow::Result<()> {
        let repl_root_dir = repl_root_dir.canonicalize()?;
        let repl_root_bridge_sdk_dir = repl_root_bridge_sdk_dir.canonicalize()?;
        let relative_bridge_sdk_unix_path = repl_root_bridge_sdk_dir
            .strip_prefix(&repl_root_dir)?
            .display()
            .to_string()
            .replace("\\", "/");

        let workspaces = agent_type_names
            .iter()
            .map(|agent_type_name| {
                format!(
                    "{}/{}",
                    relative_bridge_sdk_unix_path,
                    bridge_client_directory_name(agent_type_name)
                )
            })
            .collect::<Vec<_>>();

        let dependencies = agent_type_names
            .iter()
            .map(|agent_type_name| (bridge_client_directory_name(agent_type_name), "*"))
            .collect::<BTreeMap<_, _>>();

        let package_json = json!({
          "name": "repl",
          "type": "module",
          "private": true,
          "workspaces": workspaces,
          "dependencies": dependencies,
          "devDependencies": {
            "@golem/golem-ts-repl": self.ctx.template_sdk_overrides().ts_package_version_or_path("golem-ts-repl"),
            "tsx": "^4.7",
            "typescript": "^5.9"
          }
        });

        fs::write_str(
            repl_root_dir.join("package.json"),
            serde_json::to_string_pretty(&package_json)?,
        )?;

        let tsconfig_json = json!({
          "compilerOptions": {
            "composite": true,
            "declaration": true,
            "esModuleInterop": true,
            "forceConsistentCasingInFileNames": true,
            "module": "ES2022",
            "moduleResolution": "nodenext",
            "skipLibCheck": true,
            "sourceMap": true,
            "strict": true,
            "target": "ES2022"
          },
          "include": [
            format!("{}/ts/**/*.ts", repl_root_bridge_sdk_dir.display())
          ]
        });

        fs::write_str(
            repl_root_dir.join("tsconfig.json"),
            serde_json::to_string_pretty(&tsconfig_json)?,
        )?;

        let agents_config = agent_type_names
            .iter()
            .map(|agent_type_name| {
                formatdoc! {"
                    '{agent_type_name}': {{
                      typeName: '{agent_type_name}',
                      clientPackageName: '{client_package_name}',
                      package: await import('{client_package_name}'),
                    }}",
                    client_package_name = bridge_client_directory_name(agent_type_name)
                }
                .lines()
                .enumerate()
                .map(|(idx, l)| {
                    if idx == 0 {
                        l.to_string()
                    } else {
                        format!("    {l}")
                    }
                })
                .join("\n")
            })
            .collect::<Vec<_>>()
            .join(",\n");

        fs::write_str(
            repl_root_dir.join("repl.ts"),
            formatdoc! {"
                    import 'tsx/patch-repl';
                    const {{ Repl }} = await import('@golem/golem-ts-repl');

                    const repl = new Repl({{
                      agents: {{
                        {agents_config}
                      }}
                    }});

                    await repl.run();
                ",
            },
        )?;

        Command::new("npm")
            .arg("install")
            .current_dir(&repl_root_dir)
            .stream_and_wait_for_status("TS REPL - npm install")
            .await?
            .check_exit_status()?;

        let repl_env_vars = {
            let mut env = HashMap::new();
            env.insert(
                "GOLEM_REPL_APPLICATION",
                environment.application_name.0.clone(),
            );
            env.insert(
                "GOLEM_REPL_ENVIRONMENT",
                environment.environment_name.0.clone(),
            );
            match self
                .ctx
                .manifest_environment()
                .and_then(|e| e.environment.server.as_ref())
            {
                Some(Server::Builtin(BuiltinServer::Local)) | None => {
                    env.insert("GOLEM_REPL_SERVER_KIND", "local".to_string());
                }
                Some(Server::Builtin(BuiltinServer::Cloud)) => {
                    env.insert("GOLEM_REPL_SERVER_KIND", "cloud".to_string());
                    env.insert(
                        "GOLEM_REPL_SERVER_TOKEN",
                        self.ctx.auth_token().await?.into_secret(),
                    );
                }
                Some(Server::Custom(custom)) => {
                    env.insert("GOLEM_REPL_SERVER_KIND", "custom".to_string());
                    env.insert("GOLEM_REPL_SERVER_CUSTOM_URL", custom.url.to_string());
                    env.insert(
                        "GOLEM_REPL_SERVER_TOKEN",
                        self.ctx.auth_token().await?.into_secret(),
                    );
                }
            }
            env
        };

        Command::new("npx")
            .args(&["tsx", "repl.ts"])
            .current_dir(&repl_root_dir)
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .stdin(std::process::Stdio::inherit())
            .envs(repl_env_vars)
            .spawn()?
            .wait()
            .await?
            .check_exit_status()
    }

    async fn rust_repl(
        &self,
        environment: ResolvedEnvironmentIdentity,
        script: Option<String>,
        stream_logs: bool,
        repl_root_dir: PathBuf,
        repl_root_bridge_sdk_dir: PathBuf,
        agent_type_names: Vec<AgentTypeName>,
    ) -> anyhow::Result<()> {
        todo!("Rust REPL is not implemented yet")
    }

    async fn rib_repl(
        &self,
        component_name: Option<ComponentName>,
        component_revision: Option<ComponentRevision>,
        deploy_args: Option<&DeployArgs>,
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
                deploy_args,
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
}
