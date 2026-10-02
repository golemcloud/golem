// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

//! `golem ssh`: a local command prompt over an agent's bound bash tool.
//!
//! Every submitted line is one scalar `run` call, the same call `tool invoke --agent` makes. The
//! session remembers only the directory each call ended in and passes it as `--cwd` to the next;
//! no other shell state crosses calls. Reading the next line happens locally and holds no
//! invocation open on the agent.

mod contract;

use self::contract::{
    BashResult, CallFailure, InputMode, LocalCommand, NOT_RUN_EXIT, Outcome, PromptPart,
    check_run_contract, classify_invoke_error, decode_result, exit_code, global_args, input_mode,
    local_command, lookup_command, prompt, run_argv, strip_cursor_reports,
};
use crate::command_handler::Handlers;
use crate::command_handler::tool::ToolOwner;
use crate::context::Context;
use crate::error::service::{ServiceError, ServiceErrorKind};
use crate::error::{ContextInitHintError, HintError, NonSuccessfulExit, PipedExitCode};
use crate::log::{LogOutput, Output, log_anyhow_error, log_error, logln, set_log_output};
use crate::model::agent::RawAgentId;
use anyhow::anyhow;
use golem_client::model::NativeToolInvocationMode;
use golem_common::base_model::tool::ToolName;
use golem_common::model::IdempotencyKey;
use golem_common::schema::ExternalTypedSchemaValue;
use golem_common::schema::tool::Tool;
use golem_schema::tool::argv::{self, ParsedToolArguments};
use reedline::{
    Color, DefaultHinter, Prompt, PromptEditMode, PromptHistorySearch, PromptHistorySearchStatus,
    Reedline, Signal,
};
use std::borrow::Cow;
use std::io::{IsTerminal, Write};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader, Lines, Stdin};
use tracing::debug;

pub struct SshCommandHandler {
    ctx: Arc<Context>,
}

/// A connected session: the owner, the validated tool and the one piece of remembered state.
struct Session {
    owner: ToolOwner,
    /// The agent as the user named it.
    agent: String,
    tool: String,
    definition: Tool,
    timeout: Option<u32>,
    /// The directory the session started in; empty means the agent's starting directory.
    start_cwd: String,
    /// The directory the last command ended in, passed as `--cwd` to the next one.
    cwd: String,
}

enum Submission {
    Completed(BashResult),
    Failed(CallFailure),
    Interrupted,
}

impl SshCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn cmd_ssh(
        &self,
        agent: RawAgentId,
        command: Option<String>,
        tool: ToolName,
        cwd: Option<String>,
        timeout: Option<u32>,
    ) -> anyhow::Result<()> {
        // Only the script's output may reach stdout. Without a terminal to talk to (with `-c` or
        // piped input), the CLI's own messages are held back until something fails.
        let _log_output = LogOutput::new(if command.is_some() || !std::io::stdin().is_terminal() {
            Output::BufferedUntilErr
        } else {
            Output::Stderr
        });
        self.ctx.silence_app_context_init().await;

        let mut session = match self.connect(&agent, tool.as_str(), timeout).await {
            Ok((owner, definition)) => Session {
                owner,
                agent: agent.0,
                tool: tool.as_str().to_string(),
                definition,
                timeout,
                start_cwd: cwd.clone().unwrap_or_default(),
                cwd: cwd.unwrap_or_default(),
            },
            Err(error) => return Err(self.connect_failure(error)),
        };

        let status = match command {
            Some(script) => self.run_once(&session, &script).await,
            None => self.run_interactive(&mut session).await?,
        };
        match status {
            0 => Ok(()),
            status => Err(anyhow!(PipedExitCode(status))),
        }
    }

    /// Resolves the existing agent, describes the named binding once, and checks its contract
    /// before anything is submitted.
    async fn connect(
        &self,
        agent: &RawAgentId,
        tool: &str,
        timeout: Option<u32>,
    ) -> anyhow::Result<(ToolOwner, Tool)> {
        let tool_handler = self.ctx.tool_handler();
        let owner = tool_handler.resolve_tool_owner(agent.clone()).await?;
        let definition = tool_handler
            .describe_bound_tool(&owner, tool)
            .await
            .map_err(|error| describe_failure(error, &agent.0, tool))?
            .definition;
        check_run_contract(&definition, timeout.is_some()).map_err(|reason| {
            anyhow!(
                "`{tool}` on {} is not a compatible bash tool: {reason}",
                agent.0
            )
        })?;
        Ok((owner, definition))
    }

    /// Reports a failed connection and exits 255, as nothing ran. Errors the CLI reports with its
    /// own hints keep their usual handling.
    fn connect_failure(&self, error: anyhow::Error) -> anyhow::Error {
        set_log_output(Output::Stderr);
        if error.is::<HintError>() || error.is::<ContextInitHintError>() {
            return error;
        }
        if !error.is::<NonSuccessfulExit>() {
            log_anyhow_error(&error);
        }
        anyhow!(PipedExitCode(NOT_RUN_EXIT))
    }

    /// `-c`: one submission. On success the process output is exactly the script's.
    async fn run_once(&self, session: &Session, script: &str) -> u8 {
        let key = IdempotencyKey::fresh();
        match self.submit(session, script, &key).await {
            Submission::Completed(result) => {
                write_output(&result);
                exit_code(Outcome::Ran(result.exit_code))
            }
            Submission::Failed(failure) => {
                self.report_failure(session, &failure, &key);
                if matches!(failure, CallFailure::Unknown(_)) {
                    self.report_lookup(session, &key);
                }
                exit_code(Outcome::NotRun)
            }
            Submission::Interrupted => {
                self.report_interrupted(session, &key);
                exit_code(Outcome::Interrupted)
            }
        }
    }

    async fn run_interactive(&self, session: &mut Session) -> anyhow::Result<u8> {
        let mode = input_mode(
            std::io::stdin().is_terminal(),
            std::io::stdout().is_terminal(),
        );
        let terminal = mode != InputMode::Lines;
        let colorize = terminal && self.ctx.should_colorize();
        if terminal {
            logln(format!(
                "Connected to {} via `{}`. Each command runs in a fresh shell; only the directory \
                 carries over. `exit` or Ctrl+D to leave.",
                session.agent, session.tool
            ));
        }
        let mut input = match mode {
            InputMode::Editor => Input::Editor(Some(Box::new(editor(colorize)))),
            InputMode::PromptedLines => Input::lines(true),
            InputMode::Lines => Input::lines(false),
        };

        let mut last_status = 0;
        loop {
            let prompt = prompt(&session.agent, &session.cwd, last_status, |part, text| {
                paint(colorize, part, text)
            });
            let line = match input.read(prompt).await? {
                ReadLine::Line(line) => line,
                ReadLine::Cancelled => continue,
                ReadLine::End => return Ok(last_status),
            };
            if line.trim().is_empty() {
                continue;
            }
            if let Some(LocalCommand::Exit(status)) = local_command(&line) {
                return Ok(status.unwrap_or(last_status));
            }

            let key = IdempotencyKey::fresh();
            match self.submit(session, &line, &key).await {
                Submission::Completed(result) => {
                    write_output(&result);
                    if terminal {
                        end_line(&result, colorize);
                    }
                    last_status = result.exit_code;
                    session.cwd = result.cwd;
                }
                Submission::Failed(
                    failure @ (CallFailure::Undecodable(_) | CallFailure::Unknown(_)),
                ) => {
                    // The directory the command ended in is unknown; never continue from a stale one.
                    self.report_failure(session, &failure, &key);
                    if matches!(failure, CallFailure::Unknown(_)) {
                        logln(
                            "The directory the command ended in is unknown, so the session ends.",
                        );
                        self.report_lookup(session, &key);
                    }
                    return Ok(exit_code(Outcome::NotRun));
                }
                Submission::Failed(failure) => {
                    self.report_failure(session, &failure, &key);
                    if matches!(&failure, CallFailure::Named { name, .. } if name == "invalid-cwd")
                    {
                        self.reset_cwd(session);
                    }
                    last_status = exit_code(Outcome::NotRun);
                }
                Submission::Interrupted => {
                    self.report_interrupted(session, &key);
                    return Ok(exit_code(Outcome::Interrupted));
                }
            }
        }
    }

    /// Submits one script from the remembered directory as a scalar `run` call.
    async fn submit(&self, session: &Session, script: &str, key: &IdempotencyKey) -> Submission {
        let argv = run_argv(&session.cwd, session.timeout, script);
        let (command_path, input) = match argv::parse(&session.definition, &argv) {
            Ok(ParsedToolArguments::Invoke {
                command_path,
                input,
            }) => match ExternalTypedSchemaValue::try_from(*input) {
                Ok(input) => (command_path, input),
                Err(error) => {
                    return Submission::Failed(CallFailure::Rpc(format!(
                        "cannot build the call: {error}"
                    )));
                }
            },
            Ok(ParsedToolArguments::Help(_)) => {
                return Submission::Failed(CallFailure::Rpc(
                    "cannot build the call: the tool answered with its help".to_string(),
                ));
            }
            Err(error) => {
                return Submission::Failed(CallFailure::Rpc(format!(
                    "cannot build the call: {error}"
                )));
            }
        };
        let tool_handler = self.ctx.tool_handler();
        let call = tool_handler.invoke_tool_scalar(
            &session.owner,
            &session.tool,
            command_path,
            Some(input),
            key,
            NativeToolInvocationMode::Await,
            None,
        );
        tokio::select! {
            response = call => match response {
                Ok(response) => match decode_result(response.result) {
                    Ok(result) => Submission::Completed(result),
                    Err(failure) => Submission::Failed(failure),
                },
                Err(error) => Submission::Failed(invoke_failure(error)),
            },
            _ = tokio::signal::ctrl_c() => Submission::Interrupted,
        }
    }

    /// Bash refused the remembered directory before running anything: go back to where the
    /// session started, or to the agent's starting directory if that is the one refused.
    fn reset_cwd(&self, session: &mut Session) {
        session.cwd = if session.cwd == session.start_cwd {
            String::new()
        } else {
            session.start_cwd.clone()
        };
        let place = if session.cwd.is_empty() {
            "the agent's starting directory".to_string()
        } else {
            session.cwd.clone()
        };
        logln(format!("Continuing from {place}; the command was not run."));
    }

    fn report_failure(&self, session: &Session, failure: &CallFailure, key: &IdempotencyKey) {
        set_log_output(Output::Stderr);
        match failure {
            CallFailure::Named { name, detail } => log_error(format!(
                "`{}` refused the command with `{name}`: {detail}",
                session.tool
            )),
            CallFailure::Denied(message) => log_error(format!("the command was denied: {message}")),
            CallFailure::Rpc(message) => log_error(format!("the tool call failed: {message}")),
            CallFailure::Unknown(message) => {
                log_error(format!("the tool call's outcome is unknown: {message}"));
            }
            CallFailure::Undecodable(message) => {
                log_error(format!(
                    "unexpected result from `{}`: {message}",
                    session.tool
                ));
                logln(format!(
                    "The directory the command ended in is unknown, so the session ends. \
                     Idempotency key: {}",
                    key.value
                ));
            }
        }
    }

    fn report_interrupted(&self, session: &Session, key: &IdempotencyKey) {
        set_log_output(Output::Stderr);
        log_error(format!(
            "interrupted; the command may still be running on {}",
            session.agent
        ));
        self.report_lookup(session, key);
    }

    /// Tells the user how to fetch the result of a command whose outcome this session did not
    /// see, without running it again.
    fn report_lookup(&self, session: &Session, key: &IdempotencyKey) {
        logln(format!(
            "To see its result later, run this from the same directory, with any other global \
             options this session used:\n  {}",
            lookup_command(
                &crate::command_name(),
                &self.global_args(),
                &session.agent,
                &key.value,
                &session.tool,
            )
        ));
    }

    /// The global options that select this session's agent.
    fn global_args(&self) -> Vec<String> {
        let config_dir = self.ctx.config_dir();
        let default_config_dir = dirs::home_dir().map(|home| home.join(".golem"));
        global_args(
            (default_config_dir.as_deref() != Some(config_dir)).then_some(config_dir),
            self.ctx
                .explicit_profile_name()
                .map(|profile| profile.0.as_str()),
            self.ctx.global_environment_selector(),
        )
    }
}

/// Names the failure of `describe_tool` so an absent binding, a permission denial and a missing
/// agent read differently.
fn describe_failure(error: anyhow::Error, agent: &str, tool: &str) -> anyhow::Error {
    let Some((status, rendered, messages)) = error_response(&error) else {
        return error;
    };
    let unbound = messages
        .iter()
        .any(|message| message.contains("is not bound") || message.contains("is not registered"));
    match status {
        403 => anyhow!("permission denied: {rendered}"),
        404 => anyhow!("not found: {rendered}"),
        _ if unbound => anyhow!("`{tool}` is not bound to {agent}: {rendered}"),
        _ => error,
    }
}

fn invoke_failure(error: anyhow::Error) -> CallFailure {
    match error_response(&error) {
        Some((status, rendered, _)) => classify_invoke_error(Some(status), rendered),
        None => classify_invoke_error(None, format!("{error:#}")),
    }
}

fn error_response(error: &anyhow::Error) -> Option<(u16, String, Vec<String>)> {
    let service_error = error.downcast_ref::<ServiceError>()?;
    match &service_error.kind {
        ServiceErrorKind::ErrorResponse(response) => Some((
            response.status_code,
            service_error.render(),
            response.errors.clone(),
        )),
        _ => None,
    }
}

/// Writes the script's stdout and stderr unchanged to the process's.
fn write_output(result: &BashResult) {
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(result.stdout.as_bytes());
    let _ = stdout.flush();
    let mut stderr = std::io::stderr().lock();
    let _ = stderr.write_all(result.stderr.as_bytes());
    let _ = stderr.flush();
}

/// Moves to a new line when the output did not end with one, so the prompt starts on its own
/// line; a dim `⏎` marks where the newline was added.
fn end_line(result: &BashResult, colorize: bool) {
    let last = if result.stderr.is_empty() {
        &result.stdout
    } else {
        &result.stderr
    };
    if last.is_empty() || last.ends_with('\n') {
        return;
    }
    let marker = if colorize {
        Color::DarkGray.paint("\u{23ce}").to_string()
    } else {
        "\u{23ce}".to_string()
    };
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "{marker}");
    let _ = stderr.flush();
}

fn paint(colorize: bool, part: PromptPart, text: &str) -> String {
    if !colorize {
        return text.to_string();
    }
    match part {
        PromptPart::Agent => Color::Cyan.bold().paint(text),
        PromptPart::Cwd => Color::Blue.paint(text),
        PromptPart::Status | PromptPart::Marker { ok: false } => Color::Red.paint(text),
        PromptPart::Marker { ok: true } => Color::Green.paint(text),
    }
    .to_string()
}

/// The line editor: in-memory history with a dim suggestion from it. It paints on stderr.
fn editor(colorize: bool) -> Reedline {
    Reedline::create()
        .with_hinter(Box::new(
            DefaultHinter::default().with_style(Color::DarkGray.normal()),
        ))
        .with_ansi_colors(colorize)
}

enum Input {
    /// A terminal: the editor, taken out while a blocking read runs.
    Editor(Option<Box<Reedline>>),
    /// One command per line, after the prompt when `prompt` is set.
    Lines {
        lines: Lines<BufReader<Stdin>>,
        prompt: bool,
    },
}

enum ReadLine {
    Line(String),
    /// Ctrl+C cleared the line.
    Cancelled,
    /// Ctrl+D or end of input.
    End,
}

impl Input {
    fn lines(prompt: bool) -> Self {
        Input::Lines {
            lines: BufReader::new(tokio::io::stdin()).lines(),
            prompt,
        }
    }

    async fn read(&mut self, prompt: String) -> anyhow::Result<ReadLine> {
        if let Input::Editor(slot) = self {
            let mut editor = slot
                .take()
                .expect("the editor is returned after every read");
            let prompt = prompt.clone();
            let (editor, signal) = tokio::task::spawn_blocking(move || {
                let signal = editor.read_line(&SshPrompt(prompt));
                (editor, signal)
            })
            .await?;
            *slot = Some(editor);
            match signal {
                Ok(Signal::Success(line)) => return Ok(ReadLine::Line(line)),
                Ok(Signal::CtrlD) => return Ok(ReadLine::End),
                Ok(_) => return Ok(ReadLine::Cancelled),
                // Typically the terminal did not answer the editor's cursor-position query.
                // Keep the session, with plain line input from here on.
                Err(error) => {
                    debug!(error = %error, "line editor failed; reading plain lines");
                    *self = Input::lines(true);
                }
            }
        }
        let Input::Lines {
            lines,
            prompt: show_prompt,
        } = self
        else {
            unreachable!("the editor either returned or was replaced")
        };
        if *show_prompt {
            let mut stderr = std::io::stderr().lock();
            let _ = write!(stderr, "{prompt}");
            let _ = stderr.flush();
        }
        Ok(match lines.next_line().await? {
            // Without the editor, a late answer to its cursor-position query arrives as input.
            Some(line) if *show_prompt => ReadLine::Line(strip_cursor_reports(&line)),
            Some(line) => ReadLine::Line(line),
            None => ReadLine::End,
        })
    }
}

struct SshPrompt(String);

impl Prompt for SshPrompt {
    fn render_prompt_left(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.0)
    }

    fn render_prompt_right(&self) -> Cow<'_, str> {
        Cow::Borrowed("")
    }

    fn render_prompt_indicator(&self, _mode: PromptEditMode) -> Cow<'_, str> {
        Cow::Borrowed("")
    }

    fn render_prompt_multiline_indicator(&self) -> Cow<'_, str> {
        Cow::Borrowed("\u{b7} ")
    }

    fn render_prompt_history_search_indicator(
        &self,
        history_search: PromptHistorySearch,
    ) -> Cow<'_, str> {
        let failing = match history_search.status {
            PromptHistorySearchStatus::Failing => "failing ",
            PromptHistorySearchStatus::Passing => "",
        };
        Cow::Owned(format!(
            "({failing}reverse-search: {}) ",
            history_search.term
        ))
    }
}

#[cfg(test)]
mod tests {
    use crate::command::{GolemCliCommand, GolemCliSubcommand};
    use clap::Parser;
    use test_r::test;

    #[test]
    fn a_command_script_may_start_with_a_dash() {
        let parsed = GolemCliCommand::try_parse_from(["golem-cli", "ssh", "A(\"a\")", "-c", "-x"])
            .expect("a script starting with a dash is a value");
        let GolemCliSubcommand::Ssh { command, .. } = parsed.subcommand else {
            panic!("expected the ssh subcommand");
        };
        assert_eq!(command.as_deref(), Some("-x"));
    }
}
