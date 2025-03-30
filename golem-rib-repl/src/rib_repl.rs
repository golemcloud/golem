use rib::InterpreterEnv;
use rib::InterpreterStack;
use rib::RibInput;
use rib::{Expr, RibByteCode};
use rib::{Interpreter, RibFunctionInvoke};
use rustyline::error::ReadlineError;
use rustyline::history::{DefaultHistory, History};
use rustyline::Editor;
use std::path::PathBuf;
use tokio;

// Import Rib evaluation function
use crate::dependency_manager::{ComponentDependency, RibDependencyManager};
use crate::result_printer::{DefaultResultPrinter, ResultPrinter};
use crate::syntax_highlighter::RibSyntaxHighlighter;
use rib::compile;

struct RibRepl {
    history_file_path: PathBuf,
    dependency_manager: Box<dyn RibDependencyManager>,
    worker_function_invoke: Box<dyn RibFunctionInvoke>,
    rib_result_printer: Box<dyn ResultPrinter>,
}

impl RibRepl {
    fn new(
        history_file: Option<PathBuf>,
        dependency_manager: Box<dyn RibDependencyManager>,
        worker_function_invoke: Box<dyn RibFunctionInvoke>,
        rib_result_printer: Option<Box<dyn ResultPrinter>>,
    ) -> Self {
        Self {
            history_file_path: history_file.unwrap_or_else(|| get_default_history_file()),
            dependency_manager,
            worker_function_invoke,
            rib_result_printer: rib_result_printer
                .unwrap_or_else(|| Box::new(DefaultResultPrinter)),
        }
    }

    async fn run(&mut self) {
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
        let mut repl_state = ReplState::new(self.);

        loop {
            let readline = rl.readline("> ");
            match readline {
                Ok(line) if !line.is_empty() => {
                    if line.starts_with(":load ") {
                        let files: Vec<String> =
                            line[6..].split(',').map(|s| s.trim().to_string()).collect();
                        self.load_files(files);
                    } else {
                        session_history.push(line.clone());
                        let _ = rl.add_history_entry(line.as_str());

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
                }
                Ok(_) => continue,
                Err(ReadlineError::Eof) | Err(ReadlineError::Interrupted) => break,
                Err(_) => continue,
            }
        }
    }
}

async fn eval(line: &str, repl_state: &mut ReplState) -> Result<String, String> {
    let expr = Expr::from_text(line).map_err(|e| format!("Parse error: {}", e))?;
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
    fn new(dependency: ComponentDependency) -> Self {
        Self {
            byte_code: RibByteCode::default(),
            interpreter: Interpreter::pure(
                &RibInput::default(), // no input to begin with, but as we go on, we will add input in the interpreter
                Some(InterpreterStack::default()),
                Some(InterpreterEnv::default()),
            ),
            dependency,
        }
    }
}
