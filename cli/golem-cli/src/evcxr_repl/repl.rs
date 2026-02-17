// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::evcxr_repl::cli_repl_interop::CliReplInterop;
use crate::evcxr_repl::config::ReplResolvedConfig;
use colored::Colorize;
use evcxr::{CommandContext, EvalContext, EvalContextOutputs};
use rustyline::completion::{Completer, Pair};
use rustyline::config::Builder;
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::history::DefaultHistory;
use rustyline::validate::Validator;
use rustyline::{CompletionType, Editor, ExternalPrinter, Helper};
use std::cell::RefCell;
use std::path::PathBuf;
use std::process::Child;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

pub struct Repl {
    command_context: Rc<RefCell<CommandContext>>,
    outputs: EvalContextOutputs,
    editor: Editor<ReplRustyLineEditorHelper, DefaultHistory>,
    history_path: PathBuf,
    config: ReplResolvedConfig,
    cli_repl: Rc<CliReplInterop>,
}

impl Repl {
    pub fn new() -> anyhow::Result<Self> {
        evcxr::runtime_hook();

        let (mut eval_context, outputs) = EvalContext::new()?;
        let command_context = Rc::new(RefCell::new(CommandContext::with_eval_context(
            eval_context,
        )));

        let config = ReplResolvedConfig::load()?;
        let cli_repl = Rc::new(CliReplInterop::new(&config));

        let mut config_builder = Builder::new().history_ignore_space(true);
        config_builder = config_builder.history_ignore_dups(true)?;
        let editor_config = config_builder
            .completion_type(CompletionType::List)
            .auto_add_history(true)
            .build();
        let mut editor =
            Editor::<ReplRustyLineEditorHelper, DefaultHistory>::with_config(editor_config)?;
        editor.set_helper(Some(ReplRustyLineEditorHelper::new(
            command_context.clone(),
            cli_repl.clone(),
        )));

        let history_path = PathBuf::from(&config.base_config.history_file);
        editor.load_history(&history_path)?;

        Ok(Self {
            command_context,
            outputs,
            editor,
            history_path,
            config,
            cli_repl,
        })
    }

    pub fn run(mut self) -> anyhow::Result<()> {
        spawn_output_task(self.outputs.stdout, self.editor.create_external_printer()?);
        spawn_output_task(self.outputs.stderr, self.editor.create_external_printer()?);
        let mut printer = self.editor.create_external_printer()?;

        let prompt = {
            if self.config.script_mode() {
                "".to_string()
            } else {
                let name = "golem-rust-repl".cyan();
                let app = format!("[{}]", self.config.client_config.application.green().bold());
                let env = format!(
                    "[{}]",
                    self.config.client_config.environment.yellow().bold()
                );
                let arrow = ">".green().bold();
                format!("{name}{app}{env}{arrow} ")
            }
        };

        let mut interrupted = false;
        loop {
            let line = self.editor.readline(&prompt);
            match line {
                Ok(line) => {
                    interrupted = false;
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }

                    if self.cli_repl.try_run_command(&line)? {
                        continue;
                    }

                    match self.command_context.borrow_mut().execute(&line) {
                        Ok(output) => {
                            if let Some(text) = output.get("text/plain") {
                                println!("{text}");
                            }
                            if let Some(duration) = output.timing {
                                println!("{}", format!("Took {}ms", duration.as_millis()).blue());

                                for phase in output.phases {
                                    println!(
                                        "{}",
                                        format!(
                                            "  {}: {}ms",
                                            phase.name,
                                            phase.duration.as_millis()
                                        )
                                        .blue()
                                    );
                                }
                            }
                        }
                        Err(err) => {
                            eprintln!("{err}");
                        }
                    }
                }
                Err(ReadlineError::Interrupted) => {
                    if interrupted {
                        break;
                    }
                    interrupted = true;
                    let _ = printer.print(
                        "(To exit, press Ctrl+C again or Ctrl+D or type :quit)\n".to_string(),
                    );
                }
                Err(ReadlineError::Eof) => {
                    break;
                }
                Err(err) => {
                    return Err(err.into());
                }
            }
        }

        self.editor.save_history(&self.history_path)?;
        Ok(())
    }
}

struct ReplRustyLineEditorHelper {
    context: Rc<RefCell<CommandContext>>,
    cli_repl: Rc<CliReplInterop>,
}

impl ReplRustyLineEditorHelper {
    fn new(context: Rc<RefCell<CommandContext>>, cli_repl: Rc<CliReplInterop>) -> Self {
        Self { context, cli_repl }
    }
}

impl Helper for ReplRustyLineEditorHelper {}

impl Hinter for ReplRustyLineEditorHelper {
    type Hint = String;

    fn hint(&self, _line: &str, _pos: usize, _ctx: &rustyline::Context<'_>) -> Option<String> {
        None
    }
}

impl Highlighter for ReplRustyLineEditorHelper {}

impl Validator for ReplRustyLineEditorHelper {}

impl Completer for ReplRustyLineEditorHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &rustyline::Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        if let Some(result) = self.cli_repl.complete(line, pos) {
            let pairs = result
                .values
                .into_iter()
                .map(|value| Pair {
                    display: value.clone(),
                    replacement: value,
                })
                .collect();
            return Ok((result.start, pairs));
        }

        let completions = match self.context.borrow_mut().completions(line, pos) {
            Ok(completions) => completions,
            Err(_) => return Ok((pos, Vec::new())),
        };

        let mut candidates = Vec::with_capacity(completions.completions.len());
        for completion in completions.completions {
            candidates.push(Pair {
                display: completion.code.clone(),
                replacement: completion.code,
            });
        }

        Ok((completions.start_offset, candidates))
    }
}

// TODO: handle fallbacks + decide output based on scrip mode
fn spawn_output_task(
    receiver: crossbeam_channel::Receiver<String>,
    mut printer: impl ExternalPrinter + Send + 'static,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        for line in receiver.iter() {
            if printer.print(format!("{line}\n")).is_err() {
                break;
            };
        }
    })
}
