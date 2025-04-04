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
use crate::repl_printer::ReplPrinter;
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

pub struct RibRepl {
    history_file_path: PathBuf,
    printer: Box<dyn ReplPrinter>,
    editor: Editor<RibEdit, DefaultHistory>,
    repl_state: ReplState,
}

impl RibRepl {
    pub fn read_line(&mut self) -> rustyline::Result<String> {
        self.editor.readline(">>> ".magenta().to_string().as_str())
    }

    pub async fn bootstrap(
        history_file: Option<PathBuf>,
        dependency_manager: Arc<dyn RibDependencyManager + Sync + Send>,
        worker_function_invoke: Arc<dyn WorkerFunctionInvoke + Sync + Send>,
        printer: Box<dyn ReplPrinter>,
        component_details: Option<ComponentSource>,
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

        let component_dependency = match component_details {
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
            printer,
            editor: rl,
            repl_state,
        })
    }

    pub async fn process_command(&mut self, prompt: &str) -> Result<Option<RibResult>, RibError> {
        if !prompt.is_empty() {
            self.update_rib_text_in_session(prompt);

            // Add every rib script into the history (in memory) and save it
            // regardless of whether it compiles or not
            // History is never used for any progressive compilation or interpretation
            let _ = self.editor.add_history_entry(prompt);
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

    pub async fn run(&mut self) {
        loop {
            let readline = self.editor.readline(">>> ".magenta().to_string().as_str());
            match readline {
                Ok(line) => {
                    let _ = self.process_command(&line).await;
                }
                Err(ReadlineError::Eof) | Err(ReadlineError::Interrupted) => break,
                Err(_) => continue,
            }
        }
    }

    fn update_rib_text_in_session(&mut self, rib_text: &str) {
        self.repl_state.update_rib_text(rib_text);
    }

    fn remove_rib_text_in_session(&mut self) {
        self.repl_state.pop_rib_text()
    }

    fn current_rib_program(&self) -> String {
        self.repl_state.current_rib_program()
    }
}

/// Represents the source of a component in the Rib framework.
///
/// The `component_name` should be the component's name without the `.wasm` extension
/// (e.g., `"shopping-cart"`), while `source_path` must include the full file path,
/// including the `.wasm` file (e.g., `"/path/to/shopping-cart.wasm"`).
pub struct ComponentSource {
    /// The name of the component without the `.wasm` extension.
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


// Developer note: As of now Rib interpreter can work with only one component, and there-fore,
// `RibFunctionInvoke` trait within golem-rib module doesn't include component_id in the `invoke`
// arguments.  It only needs to know about the optional worker_name, function name and arguments.
// Once golem-rib supports multiple components, the `RibFunctionInvoke`
// trait will be updated to include component_id and we can directly use that instead of
// `WorkerFunctionInvoke` which is exposed to the clients of `golem-rib-repl` module
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
