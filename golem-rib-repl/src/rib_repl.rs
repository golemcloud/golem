use rib::InterpreterEnv;
use rib::InterpreterStack;
use rib::RibInput;
use rib::{Expr, RibByteCode};
use rib::{Interpreter, RibFunctionInvoke};
use rustyline::error::ReadlineError;
use rustyline::history::{DefaultHistory, History};
use rustyline::Editor;
use std::path::PathBuf;
use std::sync::Arc;
use tokio;

// Import Rib evaluation function
use crate::dependency_manager::ComponentDependency;
use crate::result_printer::{DefaultResultPrinter, ResultPrinter};
use crate::syntax_highlighter::RibSyntaxHighlighter;
use rib::compile;

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
        let history_file = get_default_history_file();

        let mut rl = Editor::<RibSyntaxHighlighter, DefaultHistory>::new().unwrap();
        rl.set_helper(Some(RibSyntaxHighlighter::default()));

        // Try to load history from the file
        if history_file.exists() {
            if let Err(err) = rl.load_history(&history_file) {
                eprintln!("Failed to load history: {}", err);
            }
        }

        let mut session_history = vec![];

        let mut repl_state = ReplState::new(
            &self.component_dependency,
            Arc::clone(&self.worker_function_invoke),
        );

        loop {
            let readline = rl.readline(">>> ");
            match readline {
                Ok(line) if !line.is_empty() => {
                    session_history.push(line.clone());
                    let _ = rl.add_history_entry(line.as_str());
                    let _ = rl.save_history(&history_file);

                    match eval(&session_history.join(";\n"), &mut repl_state).await {
                        Ok(result) => {
                            println!("{}", result);
                        }
                        Err(err) => {
                            session_history.pop();
                            eprintln!("Error: {}", err)
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

async fn eval(rib_script: &str, repl_state: &mut ReplState) -> Result<String, String> {
    let expr = Expr::from_text(rib_script).map_err(|e| format!("Parse error: {}", e))?;
    let compiled = compile(expr, &repl_state.dependency.metadata)
        .map_err(|e| format!("Compile error: {}", e))?;
    let new_byte_code = compiled.byte_code;
    let byte_code = new_byte_code.diff(&repl_state.byte_code);

    repl_state.byte_code = new_byte_code;

    let result = repl_state
        .interpreter
        .run(byte_code)
        .await
        .map_err(|e| format!("Runtime error: {}", e))?;

    Ok(format!("{}", result))
}

fn get_default_history_file() -> PathBuf {
    let mut path = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    path.push(".rib_history");
    path
}

struct ReplState {
    byte_code: RibByteCode,
    interpreter: Interpreter,
    dependency: ComponentDependency, // To be changed to multiple dependency
}

impl ReplState {
    fn new(
        dependency: &ComponentDependency,
        invoke: Arc<dyn RibFunctionInvoke + Sync + Send>,
    ) -> Self {
        let interpreter_env = InterpreterEnv::from(&RibInput::default(), &invoke);

        Self {
            byte_code: RibByteCode::default(),
            interpreter: Interpreter::new(
                &RibInput::default(),
                invoke,
                Some(InterpreterStack::default()),
                Some(interpreter_env),
            ),
            dependency: dependency.clone(),
        }
    }
}
