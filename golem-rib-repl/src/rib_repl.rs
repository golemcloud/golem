use colored::Colorize;
use rib::{RibByteCode};
use rib::RibFunctionInvoke;
use rib::RibResult;
use rustyline::error::ReadlineError;
use rustyline::history::{History};
use rustyline::{Config, Editor};
use std::path::{Path, PathBuf};
use std::process::exit;
use std::sync::Arc;
use tokio;
use crate::bootstrap::{bootstrap, ReplBootstrapError};
use crate::compiler::compile_rib_script;
use crate::dependency_manager::{RibDependencyManager};
use crate::history::RibReplHistory;
use crate::repl_state::ReplState;
use crate::result_printer::{DefaultResultPrinter, ReplPrinter};
use crate::rib_edit::RibEdit;

pub struct RibRepl {
    pub history_file_path: PathBuf,
    pub dependency_manager: Arc<dyn RibDependencyManager + Sync + Send>,
    pub worker_function_invoke: Arc<dyn RibFunctionInvoke + Sync + Send>,
    pub printer: Box<dyn ReplPrinter>,
    // Dependency manager will deploy only this component if the REPL is specialised to just 1 component
    pub component_details: Option<ComponentDetails>,
}

impl RibRepl {
    pub fn new(
        history_file: Option<PathBuf>,
        component_dependency: Arc<dyn RibDependencyManager + Sync + Send>,
        worker_function_invoke: Arc<dyn RibFunctionInvoke + Sync + Send>,
        rib_result_printer: Option<Box<dyn ReplPrinter>>,
        component_name: Option<ComponentDetails>,
    ) -> Self {
        Self {
            history_file_path: history_file.unwrap_or_else(|| get_default_history_file()),
            dependency_manager: component_dependency,
            worker_function_invoke,
            printer: rib_result_printer
                .unwrap_or_else(|| Box::new(DefaultResultPrinter)),
            component_details: component_name,
        }
    }

    pub async fn run(&mut self) {
        let rib_editor = RibEdit::init();

        let mut rl = Editor::<RibEdit, RibReplHistory>::with_history(
            Config::default(),
            RibReplHistory::new(),
        )
        .unwrap();

        rl.set_helper(Some(rib_editor));

        let component_dependency =
            bootstrap(&self, &mut rl).await.map_err(|e| {
                match e {
                    ReplBootstrapError::NoComponentsFound => {
                        self.printer.print_bootstrap_error("No components found");
                    }
                    _ => {
                        self.printer.print_bootstrap_error(&format!("Failed to bootstrap: {}", e));
                    }
                }
                self.printer.print_bootstrap_error(&format!("Failed to bootstrap: {}", e));
                exit(1);
            });

        let mut session_history = vec![];

        let mut repl_state = ReplState::new(
            &component_dependency,
            Arc::clone(&self.worker_function_invoke),
        );

        loop {
            let readline = rl.readline(">>> ".magenta().to_string().as_str());
            match readline {
                Ok(line) if !line.is_empty() => {
                    session_history.push(line.clone());

                    // Add every rib script into the history (in memory) and save it
                    // regardless of whether it compiles or not
                    // History is never used for any progressive compilation or interpretation
                    let _ = rl.add_history_entry(line.as_str());
                    let _ = rl.save_history(&self.history_file_path);

                    match compile_rib_script(&session_history.join(";\n"), &mut repl_state) {
                        Ok(compilation) => {
                            let helper = rl.helper_mut().unwrap();

                            helper.update_progression(&compilation);

                            // Before evaluation
                            let result = eval(compilation.rib_byte_code, &mut repl_state).await;
                            match result {
                                Ok(result) => {
                                    self.printer.print_rib_result(&result);
                                }
                                Err(err) => {
                                    session_history.pop();
                                    self.printer.print_runtime_error(&err);
                                }
                            }
                        }
                        Err(err) => {
                            session_history.pop();
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
    pub source_path: Path,
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
