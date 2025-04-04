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
use golem_wasm_rpc::ValueAndType;
use rib::RibResult;
use rib::{EvaluatedFnArgs, EvaluatedFqFn, EvaluatedWorkerName, RibByteCode};
use rib::{RibError, RibFunctionInvoke};
use rustyline::error::ReadlineError;
use rustyline::history::DefaultHistory;
use rustyline::{Config, Editor};
use std::path::PathBuf;
use std::sync::Arc;

/// The REPL environment for Rib, providing an interactive shell for executing Rib code.
pub struct RibRepl {
    history_file_path: PathBuf,
    printer: Box<dyn ReplPrinter>,
    editor: Editor<RibEdit, DefaultHistory>,
    repl_state: ReplState,
}

impl RibRepl {
    /// Bootstraps and initializes the Rib REPL environment and returns a `RibRepl` instance,
    /// which can be used to `run` the REPL.
    ///
    /// # Arguments
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
    ///
    pub async fn bootstrap(
        history_file: Option<PathBuf>,
        dependency_manager: Arc<dyn RibDependencyManager + Sync + Send>,
        worker_function_invoke: Arc<dyn WorkerFunctionInvoke + Sync + Send>,
        printer: Option<Box<dyn ReplPrinter>>,
        component_source: Option<ComponentSource>,
    ) -> Result<RibRepl, ReplBootstrapError> {
        let history_file_path = history_file.unwrap_or_else(get_default_history_file);

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

        let component_dependency = match component_source {
            Some(ref details) => dependency_manager
                .add_component(&details.source_path, details.component_name.clone())
                .await
                .map_err(ReplBootstrapError::ComponentLoadError),
            None => {
                let dependencies = dependency_manager.get_dependencies().await;

                match dependencies {
                    Ok(dependencies) => {
                        let component_dependencies = dependencies.component_dependencies;

                        match &component_dependencies.len() {
                            0 => Err(ReplBootstrapError::NoComponentsFound),
                            1 => Ok(component_dependencies[0].clone()),
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
            worker_function_invoke,
        ));

        let repl_state = ReplState::new(&component_dependency, rib_function_invoke);

        Ok(RibRepl {
            history_file_path,
            printer: printer.unwrap_or_else(|| Box::new(DefaultReplResultPrinter)),
            editor: rl,
            repl_state,
        })
    }

    /// Reads a single line of input from the REPL prompt.
    ///
    /// This method is exposed for users who want to manage their own REPL loop
    /// instead of using the built-in [`Self::run`] method.
    pub fn read_line(&mut self) -> rustyline::Result<String> {
        self.editor.readline(">>> ".magenta().to_string().as_str())
    }

    /// Executes a single line of Rib code and returns the result.
    ///
    /// This function is exposed for users who want to implement custom REPL loops
    /// or integrate Rib execution into other workflows.
    /// For a built-in REPL loop, see [`Self::run`].
    pub async fn execute_rib(&mut self, rib: &str) -> Result<Option<RibResult>, RibError> {
        if !rib.is_empty() {
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
                            self.printer.print_runtime_error(&err);
                            Err(RibError::InternalError(err))
                        }
                    }
                }
                Err(err) => {
                    self.remove_rib_text_in_session();
                    self.printer.print_rib_error(&err);
                    Err(RibError::InternalError(err.to_string()))
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
            let readline = self.editor.readline(">>> ".magenta().to_string().as_str());
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

async fn eval(rib_byte_code: RibByteCode, repl_state: &mut ReplState) -> Result<RibResult, String> {
    repl_state
        .interpreter()
        .run(rib_byte_code)
        .await
        .map_err(|e| format!("Runtime error: {}", e))
}

fn get_default_history_file() -> PathBuf {
    let mut path = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    path.push(".rib_history");
    path
}

/// Represents errors that can occur during the bootstrap phase of the Rib REPL environment.
#[derive(Debug, Clone)]
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
    ) -> Result<ValueAndType, String> {
        let component_id = self.component_dependency.component_id;

        self.worker_function_invoke
            .invoke(component_id, worker_name, function_name, args)
            .await
    }
}
