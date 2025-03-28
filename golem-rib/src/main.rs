use rib::Expr;
use rib::RibInput;
use rustyline::history::DefaultHistory;
use rustyline::Editor;
use rustyline::error::ReadlineError;
use tokio;

// Import Rib evaluation function
use rib::compile; // Assuming your Rib library exposes `eval`
use rib::interpret_pure;

struct RibRepl {
    loaded_files: Vec<String>, // Just store file names
}

impl RibRepl {
    fn new() -> Self {
        Self { loaded_files: Vec::new() }
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
        
        loop {
            let readline = rl.readline("> ");
            match readline {
                Ok(line) if !line.is_empty() => {
                    let _ = rl.add_history_entry(line.as_str());

                    if line.starts_with(":load ") {
                        let files: Vec<String> = line[6..]
                            .split(',')
                            .map(|s| s.trim().to_string())
                            .collect();
                        self.load_files(files);
                    } else {
                        // Evaluate normal Rib expressions
                        match eval(&line).await {
                            Ok(result) => {
                                if !line.ends_with(';') {
                                    println!("{}", result);
                                }
                            }
                            Err(err) => eprintln!("Error: {}", err),
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

async fn eval(line: &str) -> Result<String, String> {
    let expr = Expr::from_text(line).map_err(|e| format!("Parse error: {}", e))?;
    let compiled = compile(expr, &vec![]).map_err(|e| format!("Compile error: {}", e))?;
    let result = interpret_pure(&compiled.byte_code, &RibInput::default()).await.map_err(|e| format!("Runtime error: {}", e))?;
    Ok(format!("{:?}", result))
}

#[tokio::main]
async fn main() {
    let mut repl = RibRepl::new();
    repl.run().await;
}
