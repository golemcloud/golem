use rib::{RibResult};
use rib::{RibByteCode};
use rib::{RibFunctionInvoke};
use rustyline::error::ReadlineError;
use rustyline::history::{DefaultHistory, History};
use rustyline::{Config, Editor};
use std::path::PathBuf;
use std::sync::Arc;
use colored::Colorize;
use tokio;

use crate::dependency_manager::ComponentDependency;
use crate::result_printer::{DefaultResultPrinter, ResultPrinter};
use crate::rib_edit::RibEdit;
use crate::compiler::compile_rib_script;
use crate::history::RibReplHistory;
use crate::repl_state::ReplState;

pub struct RibRepl {
    history_file_path: PathBuf,
    component_dependency: ComponentDependency,
    worker_function_invoke: Arc<dyn RibFunctionInvoke + Sync + Send>,
    rib_result_printer: Box<dyn ResultPrinter>,
    component_name: Option<String>,
}

impl RibRepl {
    pub fn new(
        history_file: Option<PathBuf>,
        component_dependency: ComponentDependency,
        worker_function_invoke: Arc<dyn RibFunctionInvoke + Sync + Send>,
        rib_result_printer: Option<Box<dyn ResultPrinter>>,
        component_name: Option<String>,
    ) -> Self {
        Self {
            history_file_path: history_file.unwrap_or_else(|| get_default_history_file()),
            component_dependency,
            worker_function_invoke,
            rib_result_printer: rib_result_printer
                .unwrap_or_else(|| Box::new(DefaultResultPrinter)),
            component_name,
        }
    }

    pub async fn run(&mut self) {

        let rib_editor = RibEdit::init();

        let mut rl =
            Editor::<RibEdit, RibReplHistory>::with_history(Config::default(), RibReplHistory::new()).unwrap();

        rl.set_helper(Some(rib_editor));

        // Try to load history from the file
        if self.history_file_path.exists() {
            if let Err(err) = rl.load_history(&self.history_file_path) {
                self.rib_result_printer.print_bootstrap_error(&format!(
                    "Failed to load history: {}. Starting with an empty history.",
                    err
                ));
            }
        }

        let mut session_history = vec![];

        let mut repl_state = ReplState::new(
            &self.component_dependency,
            Arc::clone(&self.worker_function_invoke),
        );

        loop {
            let readline = rl.readline(">>> ".magenta().to_string().as_str());
            match readline {
                Ok(line) if !line.is_empty() => {
                    session_history.push(line.clone());

                    // Add every rib script into the history (in memory) and save it
                    // regardless of whether it compiles or not
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
                                    self.rib_result_printer.print_rib_result(&result);
                                }
                                Err(err) => {
                                    session_history.pop();
                                    self.rib_result_printer.print_runtime_error(&err);
                                }
                            }
                        }
                        Err(err) => {
                            session_history.pop();
                            self.rib_result_printer.print_compilation_error(&err);
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
