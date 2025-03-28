use rib::Interpreter;
use rib::InterpreterEnv;
use rib::InterpreterStack;
use rib::RibInput;
use rib::{Expr, RibByteCode};
use rustyline::error::ReadlineError;
use rustyline::history::DefaultHistory;
use rustyline::Editor;
use tokio;

// Import Rib evaluation function
use rib::compile;

struct RibRepl {
    loaded_files: Vec<String>,
}

impl RibRepl {
    fn new() -> Self {
        Self {
            loaded_files: Vec::new(),
        }
    }

    fn load_files(&mut self, files: Vec<String>) {
        for file in &files {
            if !self.loaded_files.contains(file) {
                self.loaded_files.push(file.clone());
            }
        }
        println!("Loaded files: {:?}", self.loaded_files);
    }

    async fn run(&mut self) {
        let mut rl = Editor::<(), DefaultHistory>::new().unwrap();

        let mut lines = vec![];
        let mut repl_state = ReplState::default();

        loop {
            let readline = rl.readline("> ");
            match readline {
                Ok(line) if !line.is_empty() => {
                    let _ = rl.add_history_entry(line.as_str());

                    if line.starts_with(":load ") {
                        let files: Vec<String> =
                            line[6..].split(',').map(|s| s.trim().to_string()).collect();
                        self.load_files(files);
                    } else {
                        // Evaluate normal Rib expressions
                        lines.push(line);
                        match eval(&lines.join(";\n"), &mut repl_state).await {
                            Ok(result) => {
                                println!("{}", result);
                            }
                            Err(err) => {
                                lines.pop(); // Removing the wrong line
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
    let compiled = compile(expr, &vec![]).map_err(|e| format!("Compile error: {}", e))?;
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

struct ReplState {
    byte_code: RibByteCode,
    interpreter: Interpreter,
}

impl Default for ReplState {
    fn default() -> Self {
        Self {
            byte_code: RibByteCode::default(),
            interpreter: Interpreter::pure(
                &RibInput::default(), // TODO to be changed
                Some(InterpreterStack::default()),
                Some(InterpreterEnv::default()),
            ),
        }
    }
}

#[tokio::main]
async fn main() {
    let mut repl = RibRepl::new();
    repl.run().await;
}
