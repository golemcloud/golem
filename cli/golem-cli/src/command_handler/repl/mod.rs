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

use crate::command::shared_args::DeployArgs;
use crate::command_handler::repl::rib::CliRibRepl;
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::fs;
use crate::model::app::{ApplicationComponentSelectMode, CustomBridgeSdkTarget};
use crate::model::component::ComponentNameMatchKind;
use crate::model::environment::EnvironmentResolveMode;
use crate::model::repl::ReplLanguage;
use ::rib::{ComponentDependency, ComponentDependencyKey, CustomInstanceSpec, InterfaceName};
use anyhow::{anyhow, bail};
use golem_common::model::component::{ComponentName, ComponentRevision};
use golem_rib_repl::ReplComponentDependencies;
use golem_templates::model::GuestLanguage;
use serde_json::json;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

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

        {
            let mut app_ctx = self.ctx.app_context_lock_mut().await?;
            let app_ctx = app_ctx.some_or_err_mut()?;
            app_ctx.custom_repl_bridge_sdk_target = Some(CustomBridgeSdkTarget {
                agent_type_names: Default::default(),
                target_language: Some(language),
                output_dir: Some(app_ctx.application.repl_bridge_sdk_dir(language)),
            });
        }

        self.ctx
            .app_handler()
            .build(vec![], None, &ApplicationComponentSelectMode::All)
            .await?;

        match language {
            GuestLanguage::Rust => self.rust_repl(script, stream_logs).await?,
            GuestLanguage::TypeScript => self.ts_repl(script, stream_logs).await?,
        }

        Ok(())
    }

    async fn ts_repl(&self, script: Option<String>, stream_logs: bool) -> anyhow::Result<()> {
        json!({
          "name": "repl",
          "type": "module",
          "private": true,
          "workspaces": [
            "external/counter-agent"
          ],
          "dependencies": {
            "counter_agent": "^0.0.1"
          },
          "devDependencies": {
            "tsx": "^4.7",
            "typescript": "^5.9"
          }
        });
        todo!("TypeScript REPL is not implemented yet")
    }

    async fn rust_repl(&self, script: Option<String>, stream_logs: bool) -> anyhow::Result<()> {
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
