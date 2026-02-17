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

use crate::evcxr_repl::cli_repl_interop::{CliReplInterop, CompletionResult};
use crate::evcxr_repl::config::ReplResolvedConfig;
use colored::Colorize;
use evcxr::{CommandContext, EvalContextOutputs};
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
use std::rc::Rc;
use std::sync::{Arc, Mutex};

pub struct Repl {
    context: Rc<RefCell<CommandContext>>,
    outputs: EvalContextOutputs,
    editor: Editor<ReplHelper, DefaultHistory>,
    history_path: PathBuf,
    config: ReplResolvedConfig,
    cli_repl: Option<Rc<CliReplInterop>>,
}

impl Repl {
    pub fn new() -> anyhow::Result<Self> {
        evcxr::runtime_hook();
        let (context, outputs) = CommandContext::new()?;
        let context = Rc::new(RefCell::new(context));

        let config = ReplResolvedConfig::load()?;
        let cli_repl = build_cli_repl_interop(&config)?;

        let mut config_builder = Builder::new().history_ignore_space(true);
        config_builder = config_builder.history_ignore_dups(true)?;
        let editor_config = config_builder
            .completion_type(CompletionType::List)
            .auto_add_history(true)
            .build();
        let mut editor = Editor::<ReplHelper, DefaultHistory>::with_config(editor_config)?;
        editor.set_helper(Some(ReplHelper::new(Rc::clone(&context), cli_repl.clone())));

        let history_path = PathBuf::from(&config.base_config.history_file);
        editor.load_history(&history_path)?;

        Ok(Self {
            context,
            outputs,
            editor,
            history_path,
            config,
            cli_repl,
        })
    }

    pub async fn run(mut self) -> anyhow::Result<()> {
        let printer = self.editor.create_external_printer().ok().map(|printer| {
            Arc::new(Mutex::new(
                Box::new(printer) as Box<dyn ExternalPrinter + Send>
            ))
        });
        let stdout_handle = spawn_output_task(self.outputs.stdout, printer.clone(), false);
        let stderr_handle = spawn_output_task(self.outputs.stderr, printer.clone(), true);
        let mut saw_interrupt = false;

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

        loop {
            let line = tokio::task::block_in_place(|| self.editor.readline(&prompt));
            match line {
                Ok(line) => {
                    saw_interrupt = false;
                    if line.trim().is_empty() {
                        continue;
                    }
                    if let Some(cli_repl) = self.cli_repl.as_ref() {
                        if cli_repl.try_run_command(&line).await? {
                            continue;
                        }
                    }
                    if let Err(err) = self.context.borrow_mut().execute(&line) {
                        eprintln!("{err}");
                    }
                }
                Err(ReadlineError::Interrupted) => {
                    if saw_interrupt {
                        break;
                    }
                    saw_interrupt = true;
                    print_exit_hint(&printer);
                }
                Err(ReadlineError::Eof) => break,
                Err(err) => return Err(err.into()),
            }
        }

        self.editor.save_history(&self.history_path)?;

        drop(self.context);
        let _ = stdout_handle.await;
        let _ = stderr_handle.await;

        Ok(())
    }
}

struct ReplHelper {
    context: Rc<RefCell<CommandContext>>,
    cli_repl: Option<Rc<CliReplInterop>>,
}

impl ReplHelper {
    fn new(context: Rc<RefCell<CommandContext>>, cli_repl: Option<Rc<CliReplInterop>>) -> Self {
        Self { context, cli_repl }
    }
}

impl Helper for ReplHelper {}

impl Hinter for ReplHelper {
    type Hint = String;

    fn hint(&self, _line: &str, _pos: usize, _ctx: &rustyline::Context<'_>) -> Option<String> {
        None
    }
}

impl Highlighter for ReplHelper {}

impl Validator for ReplHelper {}

impl Completer for ReplHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &rustyline::Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        if let Some(cli_repl) = self.cli_repl.as_ref() {
            if let Some(result) = block_on_completion(cli_repl, line, pos) {
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

fn spawn_output_task(
    receiver: crossbeam_channel::Receiver<String>,
    printer: Option<Arc<Mutex<Box<dyn ExternalPrinter + Send>>>>,
    is_stderr: bool,
) -> tokio::task::JoinHandle<()> {
    tokio::task::spawn_blocking(move || {
        for msg in receiver.iter() {
            if let Some(printer) = printer.as_ref() {
                if let Ok(mut printer) = printer.lock() {
                    if printer.print(msg.clone()).is_ok() {
                        continue;
                    }
                }
            }

            if is_stderr {
                eprint!("{msg}");
            } else {
                print!("{msg}");
            }
        }
    })
}

fn print_exit_hint(printer: &Option<Arc<Mutex<Box<dyn ExternalPrinter + Send>>>>) {
    let message = "(To exit, press Ctrl+C again or Ctrl+D or type :quit)\n";
    if let Some(printer) = printer.as_ref() {
        if let Ok(mut printer) = printer.lock() {
            if printer.print(message.to_string()).is_ok() {
                return;
            }
        }
    }
    eprint!("{message}");
}

fn block_on_completion(
    cli_repl: &CliReplInterop,
    line: &str,
    pos: usize,
) -> Option<CompletionResult> {
    let handle = tokio::runtime::Handle::try_current().ok()?;
    handle.block_on(cli_repl.complete(line, pos))
}

fn build_cli_repl_interop(config: &ReplResolvedConfig) -> anyhow::Result<Option<Rc<CliReplInterop>>> {
    Ok(Some(Rc::new(CliReplInterop::new(config))))
}
