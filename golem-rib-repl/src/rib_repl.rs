// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::compiler::compile_rib_script;
use crate::dependency_manager::{RibComponentMetadata, RibDependencyManager};
use crate::invoke::WorkerFunctionInvoke;
use crate::repl_printer::{DefaultReplResultPrinter, ReplPrinter};
use crate::repl_state::ReplState;
use crate::rib_edit::RibEdit;
use async_trait::async_trait;
use colored::Colorize;
use rib::{EvaluatedFnArgs, EvaluatedFqFn, EvaluatedWorkerName, RibByteCode};
use rib::{RibCompilationError, RibFunctionInvoke};
use rib::{RibFunctionInvokeResult, RibResult, RibRuntimeError};
use rustyline::error::ReadlineError;
use rustyline::history::DefaultHistory;
use rustyline::{Config, Editor};
use std::fmt::{Display, Formatter};
use std::path::PathBuf;
use std::sync::Arc;

/// The REPL environment for Rib, providing an interactive shell for executing Rib code.
pub struct RibRepl {
    history_file_path: PathBuf,
    printer: Box<dyn ReplPrinter>,
    editor: Editor<RibEdit, DefaultHistory>,
    repl_state: ReplState,
    prompt: String,
}

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
}

impl RibRepl {
    /// Bootstraps and initializes the Rib REPL environment and returns a `RibRepl` instance,
    /// which can be used to `run` the REPL.
    ///
    pub async fn bootstrap(config: RibReplConfig) -> Result<RibRepl, ReplBootstrapError> {
        let history_file_path = config.history_file.unwrap_or_else(get_default_history_file);

        let rib_editor = RibEdit::init();

        let mut rl = Editor::<RibEdit, DefaultHistory>::with_history(
            Config::default(),
            DefaultHistory::new(),
        )
        .unwrap();

        rl.set_helper(Some(rib_editor));

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

        let rib_function_invoke = Arc::new(ReplRibFunctionInvoke::new(
            component_dependency.clone(),
            config.worker_function_invoke,
        ));

        let repl_state = ReplState::new(&component_dependency, rib_function_invoke);

        Ok(RibRepl {
            history_file_path,
            printer: config
                .printer
                .unwrap_or_else(|| Box::new(DefaultReplResultPrinter)),
            editor: rl,
            repl_state,
            prompt: config.prompt.unwrap_or_else(|| ">>> ".cyan().to_string()),
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
    pub async fn execute_rib(&mut self, rib: &str) -> Result<Option<RibResult>, RibExecutionError> {
        if !rib.is_empty() {
            let rib = rib.strip_suffix(";").unwrap_or(rib);

            self.update_rib(rib);

            // Add every rib script into the history (in memory) and save it
            // regardless of whether it compiles or not
            // History is never used for any progressive compilation or interpretation
            let _ = self.editor.add_history_entry(rib);
            let _ = self.editor.save_history(&self.history_file_path);

            match compile_rib_script(&self.current_rib_program(), &mut self.repl_state) {
                Ok(compilation) => {
                    let helper = self.editor.helper_mut().unwrap();

                    helper.update_progression(&compilation);

                    // Before evaluation
                    let result = eval(compilation.rib_byte_code, &mut self.repl_state).await;
                    match result {
                        Ok(result) => {
                            self.printer.print_rib_result(&result);
                            Ok(Some(result))
                        }
                        Err(err) => {
                            self.remove_rib_text_in_session();
                            self.printer.print_rib_runtime_error(&err);
                            Err(RibExecutionError::RibRuntimeError(err))
                        }
                    }
                }
                Err(err) => {
                    self.remove_rib_text_in_session();
                    self.printer.print_rib_compilation_error(&err);
                    Err(RibExecutionError::RibCompilationError(err))
                }
            }
        } else {
            Ok(None)
        }
    }

    /// Dynamically updates the REPL session with a new component dependency.
    ///
    /// This method is intended for use in interactive or user-controlled REPL loops,
    /// allowing runtime replacement of active component metadata.
    ///
    /// Note: Currently, only a single component is supported per session. Multi-component
    /// support is planned but not yet implemented.
    pub fn update_component_dependency(&mut self, dependency: RibComponentMetadata) {
        self.repl_state.update_dependency(dependency);
    }

    /// Starts the default REPL loop for executing Rib code interactively.
    ///
    /// This is a convenience method that repeatedly reads user input and executes
    /// it using [`Self::execute_rib`]. If you need more control over the REPL behavior,
    /// use [`Self::read_line`] and [`Self::execute_rib`] directly.
    pub async fn run(&mut self) {
        loop {
            let readline = self.read_line();
            match readline {
                Ok(rib) => {
                    let _ = self.execute_rib(&rib).await;
                }
                Err(ReadlineError::Eof) | Err(ReadlineError::Interrupted) => break,
                Err(_) => continue,
            }
        }
    }

    fn update_rib(&mut self, rib_text: &str) {
        self.repl_state.update_rib(rib_text);
    }

    fn remove_rib_text_in_session(&mut self) {
        self.repl_state.pop_rib_text()
    }

    fn current_rib_program(&self) -> String {
        self.repl_state.current_rib_program()
    }
}

#[derive(Debug)]
pub enum RibExecutionError {
    RibCompilationError(RibCompilationError),
    RibRuntimeError(RibRuntimeError),
}

impl std::error::Error for RibExecutionError {}

impl Display for RibExecutionError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            RibExecutionError::RibCompilationError(err) => write!(f, "{}", err),
            RibExecutionError::RibRuntimeError(err) => write!(f, "{}", err),
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

async fn eval(
    rib_byte_code: RibByteCode,
    repl_state: &mut ReplState,
) -> Result<RibResult, RibRuntimeError> {
    repl_state.interpreter().run(rib_byte_code).await
}

fn get_default_history_file() -> PathBuf {
    let mut path = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    path.push(".rib_history");
    path
}

/// Represents errors that can occur during the bootstrap phase of the Rib REPL environment.
#[derive(Debug, Clone, PartialEq)]
pub enum ReplBootstrapError {
    /// Multiple components were found, but the REPL requires a single component context.
    ///
    /// To resolve this, either:
    /// - Ensure the context includes only one component, or
    /// - Explicitly specify the component to load when starting the REPL.
    ///
    /// In the future, Rib will support multiple components
    MultipleComponentsFound(String),

    /// No components were found in the given context.
    NoComponentsFound,

    /// Failed to load a specified component.
    ComponentLoadError(String),

    /// Failed to read from or write to the REPL history file.
    ReplHistoryFileError(String),
}

impl std::error::Error for ReplBootstrapError {}

impl Display for ReplBootstrapError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ReplBootstrapError::MultipleComponentsFound(msg) => {
                write!(f, "Multiple components found: {}", msg)
            }
            ReplBootstrapError::NoComponentsFound => {
                write!(f, "No components found in the given context")
            }
            ReplBootstrapError::ComponentLoadError(msg) => {
                write!(f, "Failed to load component: {}", msg)
            }
            ReplBootstrapError::ReplHistoryFileError(msg) => {
                write!(f, "Failed to read/write REPL history file: {}", msg)
            }
        }
    }
}

// Note: Currently, the Rib interpreter supports only one component, so the
// `RibFunctionInvoke` trait in the `golem-rib` module does not include `component_id` in
// the `invoke` arguments. It only requires the optional worker name, function name, and arguments.
// Once multi-component support is added, the trait will be updated to include `component_id`,
// and we can use it directly instead of `WorkerFunctionInvoke` in the `golem-rib-repl` module.
struct ReplRibFunctionInvoke {
    component_dependency: RibComponentMetadata,
    worker_function_invoke: Arc<dyn WorkerFunctionInvoke + Sync + Send>,
}

impl ReplRibFunctionInvoke {
    fn new(
        component_dependency: RibComponentMetadata,
        worker_function_invoke: Arc<dyn WorkerFunctionInvoke + Sync + Send>,
    ) -> Self {
        Self {
            component_dependency,
            worker_function_invoke,
        }
    }
}

#[async_trait]
impl RibFunctionInvoke for ReplRibFunctionInvoke {
    async fn invoke(
        &self,
        worker_name: Option<EvaluatedWorkerName>,
        function_name: EvaluatedFqFn,
        args: EvaluatedFnArgs,
    ) -> RibFunctionInvokeResult {
        let component_id = self.component_dependency.component_id;
        let component_name = &self.component_dependency.component_name;

        self.worker_function_invoke
            .invoke(
                component_id,
                component_name,
                worker_name.map(|x| x.0),
                function_name.0.as_str(),
                args.0,
            )
            .await
            .map_err(|e| e.into())
    }
}
