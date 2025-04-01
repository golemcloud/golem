use crate::compiler::compile_rib_script;
use crate::dependency_manager::{ComponentDependency, RibDependencyManager};
use crate::history::RibReplHistory;
use crate::invoke::WorkerFunctionInvoke;
use crate::repl_state::ReplState;
use crate::result_printer::{DefaultResultPrinter, ReplPrinter};
use crate::rib_edit::RibEdit;
use colored::Colorize;
use rib::RibByteCode;
use rib::RibFunctionInvoke;
use rib::RibResult;
use rustyline::error::ReadlineError;
use rustyline::history::History;
use rustyline::{Config, Editor};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio;

pub struct RibRepl {
    history_file_path: PathBuf,
    worker_function_invoke: Arc<dyn WorkerFunctionInvoke + Sync + Send>,
    printer: Box<dyn ReplPrinter>,
    editor: Editor<RibEdit, RibReplHistory>,
    current_session_rib_texts: Vec<String>,
    repl_state: ReplState,
    // Rib Repl for now will only support component dependency
    component_dependency: ComponentDependency,
}

impl RibRepl {
    pub async fn bootstrap(
        history_file: Option<PathBuf>,
        dependency_manager: Arc<dyn RibDependencyManager + Sync + Send>,
        worker_function_invoke: Option<Arc<dyn WorkerFunctionInvoke + Sync + Send>>,
        rib_result_printer: Option<Box<dyn ReplPrinter>>,
        component_details: Option<ComponentDetails>,
    ) -> Result<RibRepl, ReplBootstrapError> {
        let rib_editor = RibEdit::init();

        let mut rl = Editor::<RibEdit, RibReplHistory>::with_history(
            Config::default(),
            RibReplHistory::new(),
        )
        .unwrap();

        rl.set_helper(Some(rib_editor));

        if history_file.exists() {
            if let Err(err) = rl.load_history(&history_file) {
                return Err(ReplBootstrapError::ReplHistoryFileError(format!(
                    "Failed to load history: {}. Starting with an empty history.",
                    err
                )));
            }
        }

        let component_dependency = match component_details {
            Some(ref details) => dependency_manager
                .add_component_dependency(&details.source_path, details.component_name.clone())
                .await
                .map_err(ReplBootstrapError::ComponentLoadError),
            None => {
                let dependencies = dependency_manager.add_components().await;

                match dependencies {
                    Ok(dependencies) => {
                        let component_dependencies = dependencies.component_dependencies;

                        match &component_dependencies.len() {
                            0 => Err(ReplBootstrapError::NoComponentsFound),
                            1 => Ok(component_dependencies[0].clone()),
                            _ => Err(ReplBootstrapError::MultipleComponentsFound(
                                "multiple components detected. Rib Repl currently support only a single component".to_string(),
                            )),
                        }
                    }
                    Err(err) => Err(ReplBootstrapError::ComponentLoadError(format!(
                        "Failed to register components: {}",
                        err
                    ))),
                }
            }
        }?;

        let repl_state = ReplState::new(&component_dependency, Arc::clone(&worker_function_invoke));

        let printer = rib_result_printer.unwrap_or_else(|| Box::new(DefaultResultPrinter));

        Ok(RibRepl {
            history_file_path: history_file.unwrap_or_else(get_default_history_file),
            worker_function_invoke,
            printer,
            editor: rl,
            component_dependency,
            current_session_rib_texts: vec![],
            repl_state,
        })
    }

    pub async fn run(&mut self) {
        loop {
            let readline = self.editor.readline(">>> ".magenta().to_string().as_str());
            match readline {
                Ok(line) if !line.is_empty() => {
                    self.current_session_rib_texts.push(line.clone());

                    // Add every rib script into the history (in memory) and save it
                    // regardless of whether it compiles or not
                    // History is never used for any progressive compilation or interpretation
                    let _ = self.editor.add_history_entry(line.as_str());
                    let _ = self.editor.save_history(&self.history_file_path);

                    match compile_rib_script(
                        &self.current_session_rib_texts.join(";\n"),
                        &mut self.repl_state,
                    ) {
                        Ok(compilation) => {
                            let helper = self.editor.helper_mut().unwrap();

                            helper.update_progression(&compilation);

                            // Before evaluation
                            let result =
                                eval(compilation.rib_byte_code, &mut self.repl_state).await;
                            match result {
                                Ok(result) => {
                                    self.printer.print_rib_result(&result);
                                }
                                Err(err) => {
                                    self.current_session_rib_texts.pop();
                                    self.printer.print_runtime_error(&err);
                                }
                            }
                        }
                        Err(err) => {
                            self.current_session_rib_texts.pop();
                            self.printer.print_compilation_error(&err);
                        }
                    }
                }
                Ok(_) => continue,
                Err(ReadlineError::Eof) | Err(ReadlineError::Interrupted) => break,
                Err(_) => continue,
            }
        }
    }
}

pub struct ComponentDetails {
    pub component_name: String,
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

pub enum ReplBootstrapError {
    // Currently not supported
    // So either the context should have only 1 component
    // or specifically specify the component when starting the REPL
    // In future, when Rib supports multiple components (which may require the need
    // of root package names being the component name)
    MultipleComponentsFound(String),
    NoComponentsFound,
    ComponentLoadError(String),
    Internal(String),
    ReplHistoryFileError(String),
}
