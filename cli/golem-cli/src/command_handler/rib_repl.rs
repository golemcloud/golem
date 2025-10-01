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

use crate::command::shared_args::{DeployArgs, StreamArgs};
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::NonSuccessfulExit;
use crate::fs;
use crate::log::{logln, set_log_output, Output};
use crate::model::component::ComponentView;
use crate::model::text::component::ComponentReplStartedView;
use crate::model::text::fmt::log_error;
use crate::model::{
    ComponentName, ComponentNameMatchKind, ComponentVersionSelection, Format, IdempotencyKey,
    WorkerName,
};
use anyhow::bail;
use async_trait::async_trait;
use colored::Colorize;
use golem_rib_repl::{
    Command, CommandRegistry, ReplComponentDependencies, ReplContext, RibDependencyManager,
    RibRepl, RibReplConfig, WorkerFunctionInvoke,
};
use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::json::OptionallyValueAndTypeJson;
use golem_wasm_rpc::ValueAndType;
use rib::{ComponentDependency, ComponentDependencyKey};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone)]
pub struct RibReplHandler {
    ctx: Arc<Context>,
    stream_logs: Arc<AtomicBool>,
}

impl RibReplHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self {
            ctx,
            stream_logs: Arc::new(AtomicBool::new(true)),
        }
    }

    pub async fn cmd_repl(
        &self,
        component_name: Option<ComponentName>,
        component_version: Option<u64>,
        deploy_args: Option<&DeployArgs>,
        script: Option<String>,
        script_file: Option<PathBuf>,
        stream_logs: bool,
    ) -> anyhow::Result<()> {
        self.stream_logs
            .store(stream_logs, std::sync::atomic::Ordering::Release);

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
                selected_components.project.as_ref(),
                ComponentNameMatchKind::App,
                &component_name,
                component_version.map(|v| v.into()),
                deploy_args,
            )
            .await?;

        let component_dependency_key = ComponentDependencyKey {
            component_name: component.component_name.0.clone(),
            component_id: component.versioned_component_id.component_id,
            root_package_name: component.metadata.root_package_name().clone(),
            root_package_version: component.metadata.root_package_version().clone(),
        };

        // The REPL has to know about the custom instance parameters
        // to support creating instances using agent interface names.
        let custom_instance_spec = component
            .metadata
            .agent_types()
            .iter()
            .map(|agent_type| {
                rib::CustomInstanceSpec::new(
                    agent_type.type_name.to_string(),
                    agent_type.constructor.wit_arg_types(),
                    Some(rib::InterfaceName {
                        name: agent_type.type_name.to_string(),
                        version: None,
                    }),
                )
            })
            .collect::<Vec<_>>();

        self.ctx
            .set_rib_repl_dependencies(ReplComponentDependencies {
                component_dependencies: vec![ComponentDependency::new(
                    component_dependency_key,
                    component.metadata.exports().to_vec(),
                )],
                custom_instance_spec,
            })
            .await;

        let mut command_registry = CommandRegistry::default();

        command_registry.register(Agents);
        command_registry.register(Logs {
            stream_logs: self.stream_logs.clone(),
        });

        let mut repl = RibRepl::bootstrap(RibReplConfig {
            history_file: Some(self.ctx.rib_repl_history_file().await?),
            dependency_manager: Arc::new(self.clone()),
            worker_function_invoke: Arc::new(self.clone()),
            printer: None,
            component_source: None,
            prompt: None,
            command_registry: Some(command_registry),
        })
        .await?;

        if script_input.is_none() {
            logln("");
            self.ctx.log_handler().log_view(&ComponentReplStartedView(
                ComponentView::new_rib_style(self.ctx.show_sensitive(), component),
            ));
            logln("");
        }

        match script_input {
            Some(script) => {
                let result = repl.execute(&script).await;
                match &result {
                    Ok(rib_result) => match self.ctx.format() {
                        Format::Json | Format::PrettyJson | Format::Yaml | Format::PrettyYaml => {
                            let result = rib_result.as_ref().and_then(|r| r.get_val());
                            self.ctx.log_handler().log_serializable(&result);
                        }
                        Format::Text => {
                            repl.print_execute_result(&result);
                        }
                    },
                    Err(_) => {
                        set_log_output(Output::Stderr);
                        repl.print_execute_result(&result);
                        bail!(NonSuccessfulExit);
                    }
                }
            }
            None => repl.run().await,
        }

        Ok(())
    }
}

pub struct Logs {
    stream_logs: Arc<AtomicBool>,
}

impl Command for Logs {
    type Input = bool;
    type Output = bool;
    type InputParseError = String;
    type ExecutionError = ();

    fn parse(
        &self,
        input: &str,
        _repl_context: &ReplContext,
    ) -> Result<Self::Input, Self::InputParseError> {
        input
            .parse::<bool>()
            .map_err(|err| format!("Failed to parse parameter: {err}; must be 'true' or 'false'"))
    }

    fn execute(
        &self,
        input: Self::Input,
        _repl_context: &mut ReplContext,
    ) -> Result<Self::Output, Self::ExecutionError> {
        self.stream_logs
            .store(input, std::sync::atomic::Ordering::Release);
        Ok(input)
    }

    fn print_output(&self, output: Self::Output, _repl_context: &ReplContext) {
        if output {
            println!("Log streaming enabled");
        } else {
            println!("Log streaming disabled");
        }
    }

    fn print_input_parse_error(&self, error: Self::InputParseError, _repl_context: &ReplContext) {
        println!("{}", error.red());
    }

    fn print_execution_error(&self, _error: Self::ExecutionError, _repl_context: &ReplContext) {}
}

pub struct Agents;

impl Command for Agents {
    type Input = ();
    type Output = Vec<String>;
    type InputParseError = ();
    type ExecutionError = ();

    fn parse(
        &self,
        _input: &str,
        _repl_context: &ReplContext,
    ) -> Result<Self::Input, Self::InputParseError> {
        Ok(())
    }

    fn execute(
        &self,
        _input: Self::Input,
        repl_context: &mut ReplContext,
    ) -> Result<Self::Output, Self::ExecutionError> {
        let agent_names = repl_context.get_rib_compiler().get_custom_instance_names();

        Ok(agent_names)
    }

    fn print_output(&self, output: Self::Output, _repl_context: &ReplContext) {
        println!("{}", "Available agents:".green());
        for agent_name in output {
            println!("  - {}", agent_name.cyan());
        }
        println!()
    }

    fn print_input_parse_error(&self, _error: Self::InputParseError, _repl_context: &ReplContext) {}

    fn print_execution_error(&self, _error: Self::ExecutionError, _repl_context: &ReplContext) {}
}

#[async_trait]
impl RibDependencyManager for RibReplHandler {
    async fn get_dependencies(&self) -> anyhow::Result<ReplComponentDependencies> {
        Ok(self.ctx.get_rib_repl_dependencies().await)
    }

    async fn add_component(
        &self,
        _source_path: &Path,
        _component_name: String,
    ) -> anyhow::Result<ComponentDependency> {
        unreachable!("add_component should not be used in CLI")
    }
}

#[async_trait]
impl WorkerFunctionInvoke for RibReplHandler {
    async fn invoke(
        &self,
        component_id: Uuid,
        component_name: &str,
        worker_name: &str,
        function_name: &str,
        args: Vec<ValueAndType>,
        _return_type: Option<AnalysedType>,
    ) -> anyhow::Result<Option<ValueAndType>> {
        let worker_name = WorkerName::from(worker_name);

        let component = self
            .ctx
            .component_handler()
            .component(
                None,
                component_id.into(),
                Some(ComponentVersionSelection::ByWorkerName(&WorkerName(
                    worker_name.to_string(),
                ))),
            )
            .await?;

        let Some(component) = component else {
            log_error(format!("Component {component_name} not found"));
            bail!(NonSuccessfulExit);
        };

        let arguments: Vec<OptionallyValueAndTypeJson> = args
            .into_iter()
            .map(|vat| vat.try_into().unwrap())
            .collect();

        let stream_args = if self.stream_logs.load(std::sync::atomic::Ordering::Acquire) {
            Some(StreamArgs {
                stream_no_log_level: false,
                stream_no_timestamp: false,
                logs_only: true,
            })
        } else {
            None
        };

        let result = self
            .ctx
            .worker_handler()
            .invoke_worker(
                &component,
                &worker_name,
                function_name,
                arguments,
                IdempotencyKey::new(),
                false,
                stream_args,
            )
            .await?
            .unwrap();

        Ok(result.result)
    }
}
