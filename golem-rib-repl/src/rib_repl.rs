use rib::{RibError, RibResult};
use rib::{Expr, RibByteCode};
use rib::{RibFunctionInvoke};
use rustyline::error::ReadlineError;
use rustyline::history::{DefaultHistory, History};
use rustyline::Editor;
use std::path::PathBuf;
use std::sync::Arc;
use colored::Colorize;
use tokio;

use crate::dependency_manager::ComponentDependency;
use crate::result_printer::{DefaultResultPrinter, ResultPrinter};
use crate::syntax_highlighter::RibSyntaxHighlighter;
use rib::compile;
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

        let mut rl = Editor::<RibSyntaxHighlighter, DefaultHistory>::new().unwrap();
        rl.set_helper(Some(RibSyntaxHighlighter::default()));

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
                    let _ = rl.add_history_entry(line.as_str());
                    let _ = rl.save_history(&self.history_file_path);

                    match compile_rib_script(&session_history.join(";\n"), &mut repl_state).await {
                        Ok(result) => {
                            let result = eval(result, &mut repl_state).await;
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

async fn compile_rib_script(rib_script: &str, repl_state: &mut ReplState) -> Result<RibByteCode, RibError> {
    let expr = Expr::from_text(rib_script).map_err(|e| format!("parse error: {}", e)).map_err(
        |e| RibError::InvalidRibScript(format!("Parse error: {}", e)),
    )?;

    let compiled = compile(expr, &repl_state.dependency().metadata)?;

    let new_byte_code = compiled.byte_code;
    let byte_code = new_byte_code.diff(repl_state.byte_code());

    repl_state.update_byte_code(new_byte_code);

    Ok(byte_code)
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
