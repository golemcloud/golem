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
use crate::model::component::ComponentNameMatchKind;
use crate::model::environment::ResolvedEnvironmentIdentity;
use crate::model::repl::ReplLanguage;
use ::rib::{ComponentDependency, ComponentDependencyKey, CustomInstanceSpec, InterfaceName};
use anyhow::anyhow;
use golem_common::model::component::{ComponentName, ComponentRevision};
use golem_rib_repl::ReplComponentDependencies;
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

        let selected_components = self
            .ctx
            .component_handler()
            .must_select_components_by_app_dir_or_name(component_name.as_ref())
            .await?;

        match language {
            None => {
                todo!()
            }
            Some(language) => match language {
                ReplLanguage::Rib => {
                    let component_name = {
                        if selected_components.component_names.len() == 1 {
                            selected_components.component_names[0].clone()
                        } else {
                            self.ctx.interactive_handler().select_component_for_repl(
                                selected_components.component_names.clone(),
                            )?
                        }
                    };
                    self.rib_repl(
                        selected_components.environment,
                        component_name,
                        component_revision,
                        deploy_args,
                        script_input,
                        stream_logs,
                    )
                    .await
                }
                ReplLanguage::Rust => {
                    todo!()
                }
                ReplLanguage::TypeScript => {
                    todo!()
                }
            },
        }
    }

    async fn bridge_repl() -> anyhow::Result<()> {
        todo!()
    }

    async fn rib_repl(
        &self,
        environment: ResolvedEnvironmentIdentity,
        component_name: ComponentName,
        component_revision: Option<ComponentRevision>,
        deploy_args: Option<&DeployArgs>,
        script: Option<String>,
        stream_logs: bool,
    ) -> anyhow::Result<()> {
        // NOTE: we pre-create the ReplDependencies, because trying to do it in RibDependencyManager::get_dependencies
        //       results in thread safety errors on the path when cargo component could be called for client building
        let component = self
            .ctx
            .component_handler()
            .component_by_name_with_auto_deploy(
                &environment,
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
