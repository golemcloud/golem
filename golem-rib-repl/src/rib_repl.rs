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

use crate::compiler::compile_rib_script;
use crate::dependency_manager::RibDependencyManager;
use crate::eval::eval;
use crate::invoke::WorkerFunctionInvoke;
use crate::repl_printer::{DefaultReplResultPrinter, ReplPrinter};
use crate::repl_state::ReplState;
use crate::rib_context::ReplContext;
use crate::rib_edit::RibEdit;
use crate::{CommandRegistry, ReplBootstrapError, RibExecutionError, UntypedCommand};
use colored::Colorize;
use rib::{RibCompiler, RibCompilerConfig, RibResult};
use rustyline::error::ReadlineError;
use rustyline::history::DefaultHistory;
use rustyline::{Config, Editor};
use std::path::PathBuf;
use std::sync::Arc;

/// Config options:
///
/// - `history_file`: Optional path to a file where the REPL history will be stored and loaded from.
///   If `None`, it will be loaded from `~/.rib_history`.
/// - `dependency_manager`: This is responsible for how to load all the components or a specific
///   custom component.
/// - `worker_function_invoke`: An implementation of the `WorkerFunctionInvoke` trait,
/// - `printer`: Optional custom printer for displaying results and errors in the REPL. If `None`
///   a default printer will be used.
/// - `component_source`: Optional details about the component to be loaded, including its name
///   and source file path. If `None`, the REPL will try to load all the components using the
///   `dependency_manager`, otherwise, `dependency_manager` will load only the specified component.
/// - `prompt`: optional custom prompt, defaults to `>>>` in cyan
pub struct RibReplConfig {
    pub history_file: Option<PathBuf>,
    pub dependency_manager: Arc<dyn RibDependencyManager + Sync + Send>,
    pub worker_function_invoke: Arc<dyn WorkerFunctionInvoke + Sync + Send>,
    pub printer: Option<Box<dyn ReplPrinter>>,
    pub component_source: Option<ComponentSource>,
    pub prompt: Option<String>,
    pub command_registry: Option<CommandRegistry>,
}

/// The REPL environment for Rib, providing an interactive shell for executing Rib code.
pub struct RibRepl {
    printer: Box<dyn ReplPrinter>,
    editor: Editor<RibEdit, DefaultHistory>,
    repl_state: Arc<ReplState>,
    prompt: String,
    command_registry: CommandRegistry,
}

impl RibRepl {
    /// Bootstraps and initializes the Rib REPL environment and returns a `RibRepl` instance,
    /// which can be used to `run` the REPL.
    ///
    pub async fn bootstrap(config: RibReplConfig) -> Result<RibRepl, ReplBootstrapError> {
        let history_file_path = config.history_file.unwrap_or_else(get_default_history_file);

        let mut command_registry = CommandRegistry::built_in();
        let external_commands = config.command_registry.unwrap_or_default();
        command_registry.merge(external_commands);

        let helper = RibEdit::init(&command_registry);

        let mut rl = Editor::<RibEdit, DefaultHistory>::with_history(
            Config::default(),
            DefaultHistory::new(),
        )
        .unwrap();

        rl.set_helper(Some(helper));

        if history_file_path.exists() {
            if let Err(err) = rl.load_history(&history_file_path) {
                return Err(ReplBootstrapError::ReplHistoryFileError(format!(
                    "Failed to load history: {}. Starting with an empty history.",
                    err
                )));
            }
        }

        let component_dependency = match config.component_source {
            Some(ref details) => config
                .dependency_manager
                .add_component(&details.source_path, details.component_name.clone())
                .await
                .map_err(|err| ReplBootstrapError::ComponentLoadError(err.to_string())),
            None => {
                let dependencies = config.dependency_manager.get_dependencies().await;

                match dependencies {
                    Ok(dependencies) => {
                        let mut component_dependencies = dependencies.component_dependencies;

                        match &component_dependencies.len() {
                            0 => Err(ReplBootstrapError::NoComponentsFound),
                            1 => Ok(component_dependencies.pop().unwrap()),
                            _ => Err(ReplBootstrapError::MultipleComponentsFound(
                                "multiple components detected. rib repl currently support only a single component".to_string(),
                            )),
                        }
                    }
                    Err(err) => Err(ReplBootstrapError::ComponentLoadError(format!(
                        "failed to register components: {}",
                        err
                    ))),
                }
            }
        }?;

        // Once https://github.com/golemcloud/golem/issues/1608 is resolved,
        // component dependency will not be required in the REPL state
        let repl_state = ReplState::new(
            component_dependency.clone(),
            config.worker_function_invoke,
            RibCompiler::new(RibCompilerConfig::new(
                component_dependency.metadata,
                vec![],
            )),
            history_file_path.clone(),
        );

        Ok(RibRepl {
            printer: config
                .printer
                .unwrap_or_else(|| Box::new(DefaultReplResultPrinter)),
            editor: rl,
            repl_state: Arc::new(repl_state),
            prompt: config
                .prompt
                .unwrap_or_else(|| ">>> ".truecolor(192, 192, 192).to_string()),
            command_registry,
        })
    }

    /// Reads a single line of input from the REPL prompt.
    ///
    /// This method is exposed for users who want to manage their own REPL loop
    /// instead of using the built-in [`Self::run`] method.
    pub fn read_line(&mut self) -> rustyline::Result<String> {
        self.editor.readline(&self.prompt)
    }

    /// Executes a single line of Rib code and returns the result.
    ///
    /// This function is exposed for users who want to implement custom REPL loops
    /// or integrate Rib execution into other workflows.
    /// For a built-in REPL loop, see [`Self::run`].
    pub async fn execute(
        &mut self,
        script_or_command: &str,
    ) -> Result<Option<RibResult>, RibExecutionError> {
        let script_or_command = CommandOrExpr::from_str(script_or_command, &self.command_registry)
            .map_err(RibExecutionError::Custom)?;

        match script_or_command {
            CommandOrExpr::Command { args, executor } => {
                let mut repl_context =
                    ReplContext::new(self.printer.as_ref(), &self.repl_state, &mut self.editor);

                executor.run(args.as_str(), &mut repl_context);

                Ok(None)
            }
            CommandOrExpr::RawExpr(script) => {
                // If the script is empty, we do not execute it
                if !script.is_empty() {
                    let rib = script.strip_suffix(";").unwrap_or(script.as_str()).trim();

                    self.repl_state.update_rib(rib);

                    // Add every rib script into the history (in memory) and save it
                    // regardless of whether it compiles or not
                    // History is never used for any progressive compilation or interpretation
                    let _ = self.editor.add_history_entry(rib);
                    let _ = self
                        .editor
                        .save_history(self.repl_state.history_file_path());

                    match compile_rib_script(&self.current_rib_program(), &self.repl_state) {
                        Ok(compiler_output) => {
                            let rib_edit = self.editor.helper_mut().unwrap();

                            rib_edit.update_progression(&compiler_output);

                            let result =
                                eval(compiler_output.rib_byte_code, &self.repl_state).await;

                            match result {
                                Ok(result) => Ok(Some(result)),
                                Err(err) => {
                                    self.repl_state.remove_last_rib_expression();

                                    Err(RibExecutionError::RibRuntimeError(err))
                                }
                            }
                        }
                        Err(err) => {
                            self.repl_state.remove_last_rib_expression();

                            Err(RibExecutionError::RibCompilationError(err))
                        }
                    }
                } else {
                    Ok(None)
                }
            }
        }
    }

    /// Starts the default REPL loop for executing Rib code interactively.
    ///
    /// This is a convenience method that repeatedly reads user input and executes
    /// it using [`Self::execute`]. If you need more control over the REPL behavior,
    /// use [`Self::read_line`] and [`Self::execute`] directly.
    pub async fn run(&mut self) {
        loop {
            let readline = self.read_line();
            match readline {
                Ok(rib) => {
                    let result = self.execute(rib.as_str()).await;

                    match result {
                        Ok(Some(result)) => {
                            self.printer.print_rib_result(&result);
                        }

                        Ok(None) => {}

                        Err(err) => match err {
                            RibExecutionError::RibRuntimeError(runtime_error) => {
                                self.printer.print_rib_runtime_error(&runtime_error);
                            }
                            RibExecutionError::RibCompilationError(runtime_error) => {
                                self.printer.print_rib_compilation_error(&runtime_error);
                            }
                            RibExecutionError::Custom(custom_error) => {
                                self.printer.print_custom_error(&custom_error);
                            }
                        },
                    }
                }
                Err(ReadlineError::Eof) | Err(ReadlineError::Interrupted) => break,
                Err(_) => continue,
            }
        }
    }

    fn current_rib_program(&self) -> String {
        self.repl_state.current_rib_program()
    }
}

enum CommandOrExpr {
    Command {
        args: String,
        executor: Arc<dyn UntypedCommand>,
    },
    RawExpr(String),
}

impl CommandOrExpr {
    pub fn from_str(input: &str, command_registry: &CommandRegistry) -> Result<Self, String> {
        if input.starts_with(":") {
            let repl_input = input.split_whitespace().collect::<Vec<&str>>();

            let command_name = repl_input
                .first()
                .map(|x| x.strip_prefix(":").unwrap_or(x).trim())
                .ok_or("Expecting a command name after `:`".to_string())?;

            let input_args = repl_input[1..].join(" ");

            let command = command_registry
                .get_command(command_name)
                .map(|command| CommandOrExpr::Command {
                    args: input_args,
                    executor: command,
                })
                .ok_or_else(|| format!("Command '{}' not found", command_name))?;

            Ok(command)
        } else {
            Ok(CommandOrExpr::RawExpr(input.trim().to_string()))
        }
    }
}

/// Represents the source of a component in the REPL session.
///
/// The `source_path` must include the full file path,
/// including the `.wasm` file (e.g., `"/path/to/shopping-cart.wasm"`).
pub struct ComponentSource {
    /// The name of the component
    pub component_name: String,

    /// The full file path to the WebAssembly source file, including the `.wasm` extension.
    pub source_path: PathBuf,
}

fn get_default_history_file() -> PathBuf {
    let mut path = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    path.push(".rib_history");
    path
}
