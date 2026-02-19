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
use crate::evcxr_repl::log::{log, logln, set_output, OutputMode};
use crate::fs;
use crate::process::{with_hidden_output_unless_error, HiddenOutput};
use anyhow::{anyhow, bail};
use ariadne::sources;
use colored::control::SHOULD_COLORIZE;
use colored::Colorize;
use evcxr::{CommandContext, CompilationError, EvalContextOutputs, Theme};
use heck::{ToKebabCase, ToSnakeCase};
use indoc::formatdoc;
use rustyline::completion::{Completer, Pair};
use rustyline::config::Builder;
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::history::DefaultHistory;
use rustyline::validate::Validator;
use rustyline::{CompletionType, Editor, ExternalPrinter, Helper};
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;
use unicode_segmentation::UnicodeSegmentation;

pub struct Repl {
    config: ReplResolvedConfig,
    command_context: Rc<RefCell<CommandContext>>,
    cli_repl: Rc<CliReplInterop>,
}

impl Repl {
    pub fn run() -> anyhow::Result<()> {
        evcxr::runtime_hook();

        let config = ReplResolvedConfig::load()?;

        if config.script_mode() {
            set_output(OutputMode::Stderr);
        }

        Self::write_evcxr_toml(&config)?;

        let (mut command_context, outputs) = {
            let _spinner: Option<SpinnerGuard> = (!config.script_mode())
                .then(|| SpinnerGuard::start_stdout("Initializing REPL...", false));
            CommandContext::new()?
        };
        let builtin_commands = Self::collect_builtin_commands(&mut command_context)?;
        let command_context = Rc::new(RefCell::new(command_context));
        let cli_repl = Rc::new(CliReplInterop::new(&config, builtin_commands));

        let repl = Self {
            command_context,
            config,
            cli_repl,
        };

        repl.setup_command_context()?;

        if repl.config.script_mode() {
            repl.run_script(outputs)
        } else {
            repl.run_interactive(outputs)
        }
    }

    fn collect_builtin_commands(
        command_context: &mut CommandContext,
    ) -> anyhow::Result<HashMap<String, String>> {
        let help_output = command_context.execute(":help")?;
        let help_output = help_output
            .get("text/plain")
            .ok_or_else(|| anyhow!("Missing help output"))?;
        let commands = help_output
            .split('\n')
            .filter_map(|line| line.trim().split_once(' '))
            .filter_map(|(name, desc)| {
                name.strip_prefix(':')
                    .map(|name| (name.to_string(), desc.trim_start().to_string()))
            })
            .collect::<HashMap<String, String>>();
        Ok(commands)
    }

    fn build_editor(
        command_context: Rc<RefCell<CommandContext>>,
        cli_repl: Rc<CliReplInterop>,
    ) -> anyhow::Result<Editor<ReplRustyLineEditorHelper, DefaultHistory>> {
        let mut config_builder = Builder::new().history_ignore_space(true);
        config_builder = config_builder.history_ignore_dups(true)?;
        let editor_config = config_builder
            .completion_type(CompletionType::List)
            .auto_add_history(true)
            .build();
        let mut editor =
            Editor::<ReplRustyLineEditorHelper, DefaultHistory>::with_config(editor_config)?;
        editor.set_helper(Some(ReplRustyLineEditorHelper::new(
            command_context,
            cli_repl,
        )));

        Ok(editor)
    }

    fn write_evcxr_toml(config: &ReplResolvedConfig) -> anyhow::Result<()> {
        let mut dependencies = String::new();
        let mut prelude = String::new();
        let mut configure_calls = String::new();

        dependencies.push_str(
            "tokio = { version = \"1.44\", features = [\"macros\", \"rt-multi-thread\", \"sync\", \"io-std\", \"net\", \"tracing\", \"process\"] }\n",
        );
        dependencies.push_str(&format!(
            "golem-client = {{ {} }}\n",
            config.base_config.golem_client_dependency
        ));

        let server_config = match &config.client_config.server {
            crate::evcxr_repl::config::GolemServer::Local => {
                "golem_client::bridge::GolemServer::Local".to_string()
            }
            crate::evcxr_repl::config::GolemServer::Cloud { .. } => {
                "golem_client::bridge::GolemServer::Cloud { token: std::env::var(\"GOLEM_REPL_SERVER_TOKEN\").expect(\"Missing GOLEM_REPL_SERVER_TOKEN\") }".to_string()
            }
            crate::evcxr_repl::config::GolemServer::Custom { url, .. } => {
                let url = toml_string_literal(url)?;
                format!(
                    "golem_client::bridge::GolemServer::Custom {{ url: {url}.parse().expect(\"Invalid GOLEM_REPL_SERVER_CUSTOM_URL\"), token: std::env::var(\"GOLEM_REPL_SERVER_TOKEN\").expect(\"Missing GOLEM_REPL_SERVER_TOKEN\") }}"
                )
            }
        };

        for (agent_type_name, agent) in &config.repl_metadata.agents {
            // TODO: from meta?
            let agent_package_name = format!("{}-client", agent_type_name.0.to_kebab_case());
            let agent_client_mod_name = format!("{}_client", agent_type_name.0.to_snake_case());
            let agent_client_name = &agent_type_name;

            let path = toml_string_literal(fs::path_to_str(
                &PathBuf::from(&config.base_config.app_main_dir).join(&agent.client_dir),
            )?)?;

            dependencies.push_str(&format!("{agent_package_name} = {{ path = {path} }}\n"));
            if !config.cli_args.disable_auto_imports {
                prelude.push_str(&format!("use {agent_client_mod_name};\n"));
                prelude.push_str(&format!(
                    "use {agent_client_mod_name}::{agent_client_name};\n"
                ));
            }
            configure_calls.push_str(&format!(
                "    {{ {agent_client_mod_name}::configure({server_config}, &app_name, &env_name); }}\n"
            ));
        }

        prelude.push_str(
            r#"
                fn golem_repl_configure_clients() {
            "#,
        );
        prelude.push_str("    let app_name = ");
        prelude.push_str(&toml_string_literal(&config.client_config.application)?);
        prelude.push_str(";\n");
        prelude.push_str("    let env_name = ");
        prelude.push_str(&toml_string_literal(&config.client_config.environment)?);
        prelude.push_str(";\n");
        prelude.push_str(&configure_calls);
        prelude.push_str("}\n");

        fs::write_str(
            "evcxr.toml",
            formatdoc! { r#"
                [evcxr]
                tmpdir = "."
                prelude = """
                {prelude}
                """

                [dependencies]
                {dependencies}
            "#,},
        )?;

        Ok(())
    }

    fn setup_command_context(&self) -> anyhow::Result<()> {
        let mut command_context = self.command_context.borrow_mut();

        command_context.set_opt_level("0")?;

        let _spinner: Option<SpinnerGuard> = (!self.config.script_mode())
            .then(|| SpinnerGuard::start_tty("Building dependencies...", false))
            .flatten();
        with_hidden_output_unless_error(HiddenOutput::All, || {
            command_context
                .execute(":load_config --quiet")
                .map_err(|err| anyhow!(err))
        })?;

        Ok(())
    }

    fn run_interactive(&self, outputs: EvalContextOutputs) -> anyhow::Result<()> {
        let mut editor = Self::build_editor(self.command_context.clone(), self.cli_repl.clone())?;

        let history_path = PathBuf::from(&self.config.base_config.history_file);
        editor.load_history(&history_path)?;

        spawn_printer_output_task(outputs.stdout, editor.create_external_printer()?);
        spawn_printer_output_task(outputs.stderr, editor.create_external_printer()?);
        let mut printer = editor.create_external_printer()?;

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
            let line = editor.readline(&prompt);
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

                    let result = {
                        let is_builtin = line.trim_start().starts_with(':');

                        let command = if is_builtin {
                            line.clone()
                        } else {
                            format!("golem_repl_configure_clients();\n{}", line)
                        };

                        let _spinner = SpinnerGuard::start_printer(
                            editor.create_external_printer()?,
                            "Waiting for result...",
                            true,
                        );
                        self.command_context.borrow_mut().execute(&command)
                    };

                    match result {
                        Ok(output) => {
                            if let Some(text) = output.get("text/plain") {
                                logln(text);
                            }
                            if let Some(duration) = output.timing {
                                logln(format!("Took {}ms", duration.as_millis()).blue());

                                for phase in output.phases {
                                    logln(
                                        format!(
                                            "  {}: {}ms",
                                            phase.name,
                                            phase.duration.as_millis()
                                        )
                                        .blue(),
                                    );
                                }
                            }
                        }
                        Err(evcxr::Error::CompilationErrors(errors)) => {
                            Self::log_errors(prompt.len(), &line, errors);
                        }
                        Err(err) => {
                            logln(err.to_string().bright_red());
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

        editor.save_history(&history_path)?;

        Ok(())
    }

    pub fn run_script(&self, outputs: EvalContextOutputs) -> anyhow::Result<()> {
        let _stdout_task = spawn_logger_output_task(outputs.stdout);
        let _stderr_task = spawn_logger_output_task(outputs.stderr);

        let script = if let Some(script_file) = &self.config.cli_args.script_file {
            fs::read_to_string(script_file)?
        } else if let Some(script) = &self.config.cli_args.script {
            script.clone()
        } else {
            bail!("Missing script source");
        };

        let script = format!("golem_repl_configure_clients();\n{}", script);

        let output = self.command_context.borrow_mut().execute(&script)?;
        if let Some(output) = output.get("text/plain") {
            println!("{}", output);
        }

        Ok(())
    }

    // From https://github.com/evcxr/evcxr/blob/94d18a5cc9c5351123f2d428da7074ee42fc32e0/evcxr_repl/src/bin/evcxr.rs#L133
    // with printing changed to use our logging
    fn log_errors(prompt_len: usize, source: &str, errors: Vec<CompilationError>) {
        /// Returns a 0-based grapheme index corresponding to the supplied 0-based character column.
        fn character_column_to_grapheme_number(character_column: usize, line: &str) -> usize {
            let mut characters_remaining = character_column;
            let mut grapheme_index = 0;
            for (_byte_offset, chars) in UnicodeSegmentation::grapheme_indices(line, true) {
                let num_chars = chars.chars().count();
                if characters_remaining < num_chars {
                    break;
                }
                characters_remaining -= num_chars;
                grapheme_index += 1;
            }
            grapheme_index
        }

        let mut last_span_lines: &Vec<String> = &vec![];
        for error in &errors {
            if error.is_from_user_code() {
                if let Some(report) =
                    error.build_report("command".to_string(), source.to_string(), Theme::Dark)
                {
                    report
                        .print(sources([("command".to_string(), source.to_string())]))
                        .unwrap();
                    continue;
                }
                for spanned_message in error.spanned_messages() {
                    if let Some(span) = &spanned_message.span {
                        let mut start_column = character_column_to_grapheme_number(
                            span.start_column - 1,
                            &spanned_message.lines[0],
                        );
                        let mut end_column = character_column_to_grapheme_number(
                            span.end_column - 1,
                            spanned_message.lines.last().unwrap(),
                        );
                        // Considering spans can cover multiple lines, it could be that end_column
                        // is less than start_column.
                        if end_column < start_column {
                            std::mem::swap(&mut start_column, &mut end_column);
                        }
                        if source.lines().count() > 1 {
                            // for multi line source code, print the lines
                            if last_span_lines != &spanned_message.lines {
                                for line in &spanned_message.lines {
                                    logln(line);
                                }
                            }
                            last_span_lines = &spanned_message.lines;
                        } else {
                            log(" ".repeat(prompt_len));
                        }
                        log(" ".repeat(start_column));

                        // Guaranteed not to underflow since if they were out-of-order, we swapped
                        // them above.
                        let span_diff = end_column - start_column;
                        let carrots = "^".repeat(span_diff);
                        log(carrots.bright_red());
                        logln(format!(" {}", spanned_message.label.bright_blue()));
                    } else {
                        // Our error originates from both user-code and generated
                        // code.
                        logln(spanned_message.label.bright_blue());
                    }
                }
                logln(error.message().bright_red());
                for help in error.help() {
                    logln(format!("{} {help}", "help:".bold()));
                }
                if let Some(extra_hint) = error.evcxr_extra_hint() {
                    logln(extra_hint);
                }
            } else {
                logln(format!(
                    "A compilation error was found in code we generated.\n\
                     Ideally this shouldn't happen. Type :last_error_json to see details.\n{}",
                    error.rendered()
                ));
            }
        }
    }
}

struct SpinnerGuard {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl SpinnerGuard {
    fn start_printer<T>(printer: T, label: &str, with_initial_delay: bool) -> Self
    where
        T: ExternalPrinter + Send + 'static,
    {
        let mut printer = printer;
        Self::start_with_writer(
            move |text| {
                let _ = printer.print(text);
            },
            label,
            with_initial_delay,
        )
    }

    fn start_stdout(label: &str, with_initial_delay: bool) -> Self {
        Self::start_with_writer(
            |text| {
                let mut out = io::stdout();
                let _ = out.write_all(text.as_bytes());
                let _ = out.flush();
            },
            label,
            with_initial_delay,
        )
    }

    fn start_tty(label: &str, with_initial_delay: bool) -> Option<Self> {
        let mut tty = open_tty_writer()?;
        Some(Self::start_with_writer(
            move |text| {
                tty(text);
            },
            label,
            with_initial_delay,
        ))
    }

    fn start_with_writer<F>(mut write: F, label: &str, with_initial_delay: bool) -> Self
    where
        F: FnMut(String) + Send + 'static,
    {
        let missing_elem = "?".to_string();
        let label = label.dimmed();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_clone = Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            let frames: Vec<String> = {
                if SHOULD_COLORIZE.should_colorize() {
                    vec!["⣾", "⣽", "⣻", "⢿", "⡿", "⣟", "⣯", "⣷"]
                        .into_iter()
                        .map(|s| s.dimmed().to_string())
                        .collect()
                } else {
                    vec!["|", "/", "-", "\\"]
                        .into_iter()
                        .map(|s| s.to_string())
                        .collect()
                }
            };
            let mut index = 0usize;
            let mut shown = false;
            let mut last_len = 0usize;
            let start = std::time::Instant::now();
            while !stop_clone.load(Ordering::Relaxed) {
                if !shown {
                    if with_initial_delay && start.elapsed() < Duration::from_millis(300) {
                        std::thread::sleep(Duration::from_millis(50));
                        continue;
                    }
                    shown = true;
                }

                let frame = frames.get(index % frames.len()).unwrap_or(&missing_elem);
                let text = format!("{frame} {label}");
                last_len = text.len();
                write(format!("\r{text}"));
                index = index.wrapping_add(1);
                std::thread::sleep(Duration::from_millis(120));
            }
            if shown && last_len > 0 {
                write(format!("\r{:width$}\r", "", width = last_len));
            }
        });
        Self {
            stop,
            handle: Some(handle),
        }
    }
}

impl Drop for SpinnerGuard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn open_tty_writer() -> Option<Box<dyn FnMut(String) + Send + 'static>> {
    #[cfg(unix)]
    let file = OpenOptions::new().write(true).open("/dev/tty").ok()?;
    #[cfg(windows)]
    let file = OpenOptions::new().write(true).open("CONOUT$").ok()?;
    #[cfg(not(any(unix, windows)))]
    {
        return None;
    }

    let mut file = file;
    Some(Box::new(move |text: String| {
        let _ = file.write_all(text.as_bytes());
        let _ = file.flush();
    }))
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

        let completions = {
            let _spinner = SpinnerGuard::start_stdout("Completing...", true);
            match self.context.borrow_mut().completions(line, pos) {
                Ok(completions) => completions,
                Err(_) => return Ok((pos, Vec::new())),
            }
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
fn spawn_printer_output_task(
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

fn spawn_logger_output_task(
    receiver: crossbeam_channel::Receiver<String>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        for line in receiver.iter() {
            logln(line);
        }
    })
}

fn toml_string_literal(s: impl AsRef<str>) -> anyhow::Result<String> {
    Ok(toml::Value::String(s.as_ref().to_string()).to_string())
}
