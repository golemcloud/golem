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

use crate::command::shared_args::StreamArgs;
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::NonSuccessfulExit;
use crate::log::log_error;
use crate::log::{logln, set_log_output, Output};
use crate::model::component::{ComponentRevisionSelection, ComponentView};
use crate::model::environment::EnvironmentResolveMode;
use crate::model::format::Format;
use crate::model::repl::{ReplLanguage, ReplScriptSource};
use crate::model::text::component::ComponentReplStartedView;
use crate::model::worker::WorkerName;
use anyhow::{anyhow, bail};
use async_trait::async_trait;
use golem_common::base_model::agent::{AgentId, AgentMode};
use golem_common::base_model::component::{ComponentDto, ComponentName};
use golem_common::base_model::IdempotencyKey;
use golem_rib_repl::{
    CommandRegistry, ReplComponentDependencies, RibDependencyManager, RibRepl, RibReplConfig,
    WorkerFunctionInvoke,
};
use golem_wasm::analysis::AnalysedType;
use golem_wasm::json::OptionallyValueAndTypeJson;
use golem_wasm::ValueAndType;
use rib::ComponentDependency;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use uuid::Uuid;

pub struct CliRibRepl {
    ctx: Arc<Context>,
    stream_logs: Arc<AtomicBool>,
    dependencies: ReplComponentDependencies,
    component: ComponentDto,
}

impl CliRibRepl {
    pub fn new(
        cli_ctx: Arc<Context>,
        stream_logs: bool,
        dependencies: ReplComponentDependencies,
        component: ComponentDto,
    ) -> Arc<Self> {
        Arc::new(Self {
            ctx: cli_ctx,
            stream_logs: Arc::new(AtomicBool::new(stream_logs)),
            dependencies,
            component,
        })
    }

    pub async fn run(self: &Arc<Self>, script: Option<ReplScriptSource>) -> anyhow::Result<()> {
        let script_input = match script {
            Some(ReplScriptSource::Inline(script)) => Some(script),
            Some(ReplScriptSource::FromFile(path)) => Some(std::fs::read_to_string(path)?),
            None => None,
        };

        let mut command_registry = CommandRegistry::default();

        command_registry.register(commands::Agents);
        command_registry.register(commands::Logs::new(self.stream_logs.clone()));

        let mut repl = RibRepl::bootstrap(RibReplConfig {
            history_file: Some(self.ctx.repl_history_file(ReplLanguage::Rust).await?),
            dependency_manager: self.clone(),
            worker_function_invoke: self.clone(),
            printer: None,
            component_source: None,
            prompt: None,
            command_registry: Some(command_registry),
        })
        .await?;

        if script_input.is_none() {
            logln("");
            self.ctx.log_handler().log_view(&ComponentReplStartedView(
                ComponentView::new_rib_style(self.ctx.show_sensitive(), self.component.clone()),
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

#[async_trait]
impl RibDependencyManager for CliRibRepl {
    async fn get_dependencies(&self) -> anyhow::Result<ReplComponentDependencies> {
        Ok(self.dependencies.clone())
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
impl WorkerFunctionInvoke for CliRibRepl {
    async fn invoke(
        &self,
        _component_id: Uuid,
        component_name: &str,
        worker_name: &str,
        function_name: &str,
        args: Vec<ValueAndType>,
        _return_type: Option<AnalysedType>,
    ) -> anyhow::Result<Option<ValueAndType>> {
        let agent_id =
            AgentId::parse(worker_name, &self.component.metadata).map_err(|err| anyhow!(err))?;

        let worker_name: WorkerName = if self.component.metadata.is_agent() {
            let agent_type_name =
                AgentId::parse_agent_type_name(worker_name).map_err(|err| anyhow!(err))?;
            let agent_type = self
                .component
                .metadata
                .find_agent_type_by_wrapper_name(&agent_type_name)
                .map_err(|err| anyhow!(err))?
                .ok_or_else(|| anyhow!("Agent type not found"))?;

            if agent_type.mode == AgentMode::Ephemeral {
                let phantom_agent_id = agent_id.with_phantom_id(Some(Uuid::new_v4()));
                phantom_agent_id.to_string().into()
            } else {
                agent_id.to_string().into()
            }
        } else {
            agent_id.to_string().into()
        };

        let component_name = ComponentName(component_name.to_string());

        let environment = self
            .ctx
            .environment_handler()
            .resolve_environment(EnvironmentResolveMode::ManifestOnly)
            .await?;

        let component = self
            .ctx
            .component_handler()
            .resolve_component(
                &environment,
                &component_name,
                Some(ComponentRevisionSelection::ByWorkerName(&worker_name)),
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
                IdempotencyKey::fresh(),
                false,
                stream_args,
            )
            .await?
            .unwrap();

        Ok(result.result)
    }
}

mod commands {
    use colored::Colorize;
    use golem_rib_repl::{Command, ReplContext};
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    pub struct Logs {
        stream_logs: Arc<AtomicBool>,
    }

    impl Logs {
        pub fn new(stream_logs: Arc<AtomicBool>) -> Self {
            Self { stream_logs }
        }
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
            input.parse::<bool>().map_err(|err| {
                format!("Failed to parse parameter: {err}; must be 'true' or 'false'")
            })
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

        fn print_input_parse_error(
            &self,
            error: Self::InputParseError,
            _repl_context: &ReplContext,
        ) {
            println!("{}", error.red());
        }

        fn print_execution_error(&self, _error: Self::ExecutionError, _repl_context: &ReplContext) {
        }
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

        fn print_input_parse_error(
            &self,
            _error: Self::InputParseError,
            _repl_context: &ReplContext,
        ) {
        }

        fn print_execution_error(&self, _error: Self::ExecutionError, _repl_context: &ReplContext) {
        }
    }
}
