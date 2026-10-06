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

mod backdrop;
mod completion;
mod contract;
mod editor;
mod git;
mod highlight;
mod history;
mod look;
mod progress;
mod syntax;

use self::completion::{Completions, Fetch};
pub(crate) use self::contract::NOT_RUN_EXIT;
use self::contract::{
    BashResult, CallFailure, CancelOutcome, Gathered, INTERRUPTED_EXIT, InputMode, LocalCommand,
    Outcome, PromptPart, RUN, banner, check_run_contract, classify_cancel, classify_invoke_error,
    decode_result, dimmed, exit_code, failed_agent_notice, global_args, help_text, input_mode,
    interrupted_message, local_command, lookup_command, prompt, run_argv, runs_nothing,
    strip_background_reply, strip_cursor_reports, time_limit, tools_listing,
};
use self::editor::{PLAIN_CONTINUATION, SshPrompt};
use self::history::{SessionHistory, history_file};
use self::look::{Loader, Palette, Readiness, shown, shown_message};
use self::progress::Ticker;
use crate::command_handler::Handlers;
use crate::command_handler::tool::ToolOwner;
use crate::context::Context;
use crate::error::service::{MapServiceError, ServiceError, ServiceErrorKind};
use crate::error::{ContextInitHintError, HintError, NonSuccessfulExit, PipedExitCode};
use crate::log::{
    LogOutput, Output, discard_held_log, log_anyhow_error, log_error, log_preformatted, log_warn,
    logln, set_log_output,
};
use crate::model::agent::RawAgentId;
use anyhow::anyhow;
use golem_client::api::WorkerClient;
use golem_client::model::NativeToolInvocationMode;
use golem_common::base_model::tool::ToolName;
use golem_common::model::{AgentStatus, IdempotencyKey};
use golem_common::schema::ExternalTypedSchemaValue;
use golem_common::schema::tool::Tool;
use golem_schema::tool::argv::{self, ParsedToolArguments};
use reedline::{Color, EditCommand, Reedline, Signal};
use std::io::{IsTerminal, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, BufReader, Lines, Stdin};
use tracing::debug;
use uuid::Uuid;

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
    /// Whether `run` takes `--timeout`, which then bounds the editor's helper scripts.
    helper_timeout: bool,
    /// Which agent answered first. One deleted and made again under the same name answers
    /// with another fingerprint.
    fingerprint: Option<Uuid>,
    /// Set while the agent is known to be busy, when a helper script would only wait behind
    /// what it is doing and be given up on.
    busy: Arc<AtomicBool>,
}

/// How an interactive session reads and draws, read off the terminal before it connects.
struct Terminal {
    mode: InputMode,
    /// Colours at all, which give the session its block look.
    colorize: bool,
    palette: Palette,
    loader: Loader,
    /// The palette for what is written on stderr, when stderr is a terminal too.
    styled: Option<Palette>,
    /// The question about the terminal's background, open until the first prompt.
    background: Option<backdrop::Query>,
}

enum Submission {
    Completed(BashResult),
    Failed(CallFailure),
    /// Ctrl+C stopped waiting; this is what the cancel request then did.
    Interrupted(CancelOutcome),
}

/// How long the editor waits for a helper script before it gives up on it.
const HELPER_WAIT: Duration = Duration::from_secs(3);

/// How long Ctrl+C waits for the answer to its cancel request.
const CANCEL_WAIT: Duration = Duration::from_secs(5);

/// How long a prompt waits to learn what the agent is doing before it is drawn without that.
const STATUS_WAIT: Duration = Duration::from_secs(1);

/// How long the request that takes a timed-out helper script out of the agent's queue may
/// take before the editor moves on without its answer.
const HELPER_CANCEL_WAIT: Duration = Duration::from_secs(1);

/// The tool's own limit on a helper script, in seconds.
const HELPER_TIME_LIMIT: u32 = 5;

/// How long `tools` waits for the list before it says there was no answer.
const TOOLS_WAIT: Duration = Duration::from_secs(15);

/// How often a waiting prompt checks that its terminal is still there.
const TERMINAL_CHECK: Duration = Duration::from_millis(500);

/// Runs the editor's helper scripts on the agent. The editor calls it from its blocking
/// thread, never from an async task.
struct AgentFetcher {
    ctx: Arc<Context>,
    owner: ToolOwner,
    tool: String,
    definition: Tool,
    with_timeout: bool,
    runtime: tokio::runtime::Handle,
    /// Shared with the session: the agent is busy, so a script would only wait.
    busy: Arc<AtomicBool>,
}

impl Fetch for AgentFetcher {
    fn run(&self, cwd: &str, script: &str) -> Option<String> {
        if self.busy.load(Ordering::Relaxed) {
            return None;
        }
        let argv = run_argv(cwd, self.with_timeout.then_some(HELPER_TIME_LIMIT), script);
        let Ok(ParsedToolArguments::Invoke {
            command_path,
            input,
        }) = argv::parse(&self.definition, &argv)
        else {
            return None;
        };
        let input = ExternalTypedSchemaValue::try_from(*input).ok()?;
        let key = IdempotencyKey::fresh();
        let tool_handler = self.ctx.tool_handler();
        let call = tool_handler.invoke_tool_scalar(
            &self.owner,
            &self.tool,
            command_path,
            Some(input),
            &key,
            NativeToolInvocationMode::Await,
            None,
        );
        let give_up = async {
            debug!("a helper script got no answer in time");
            // The next ones would wait as long, each holding the prompt, so none is sent until
            // a command has come back.
            self.busy.store(true, Ordering::Relaxed);
            // The agent is busy and the script is still queued: take it out again, without
            // holding the prompt for long if that request is slow as well.
            let cancel = cancel_call(&self.ctx, &self.owner, &key);
            let _ = tokio::time::timeout(HELPER_CANCEL_WAIT, cancel).await;
        };
        let response = self
            .runtime
            .block_on(within(HELPER_WAIT, call, give_up))?
            .inspect_err(|error| debug!(error = %error, "a helper script failed"))
            .ok()?;
        let result = decode_result(response.result).ok()?;
        (result.exit_code == 0).then_some(result.stdout)
    }
}

/// Awaits `call` for at most `wait`. When it has not answered by then, runs `give_up` and
/// answers `None`.
async fn within<T>(
    wait: Duration,
    call: impl Future<Output = T>,
    give_up: impl Future<Output = ()>,
) -> Option<T> {
    match tokio::time::timeout(wait, call).await {
        Ok(answer) => Some(answer),
        Err(_) => {
            give_up.await;
            None
        }
    }
}

/// Asks Golem to cancel the call under `key`. True when the call was still queued and is now
/// cancelled; Golem does not stop a call that has started.
async fn cancel_call(
    ctx: &Arc<Context>,
    owner: &ToolOwner,
    key: &IdempotencyKey,
) -> anyhow::Result<bool> {
    let agent = owner
        .agent_id
        .as_ref()
        .ok_or_else(|| anyhow!("the session has no agent"))?;
    Ok(ctx
        .golem_clients()
        .await?
        .worker
        .cancel_invocation(&agent.component_id.0, &agent.agent_id, &key.value)
        .await
        .map(|result| result.canceled)
        .map_service_error()?)
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
        // Read before connecting, so the terminal has that long to say what its background is.
        let terminal = command.is_none().then(|| self.terminal());
        self.ctx.silence_app_context_init().await;

        let connected = tokio::select! {
            connected = self.connect(&agent, tool.as_str(), timeout) => connected,
            // From here on Ctrl+C is the session's to answer. While it connects, it ends it.
            _ = tokio::signal::ctrl_c() => return Err(anyhow!(PipedExitCode(INTERRUPTED_EXIT))),
        };
        let mut session = match connected {
            Ok((owner, definition)) => Session {
                helper_timeout: check_run_contract(&definition, true).is_ok(),
                owner,
                agent: agent.0,
                tool: tool.as_str().to_string(),
                definition,
                timeout,
                start_cwd: cwd.clone().unwrap_or_default(),
                cwd: cwd.unwrap_or_default(),
                fingerprint: None,
                busy: Arc::default(),
            },
            Err(error) => return Err(self.connect_failure(error)),
        };

        let status = match command {
            Some(script) => self.run_once(&session, &script).await,
            None => {
                let terminal = terminal.unwrap_or_else(|| self.terminal());
                self.run_interactive(&mut session, terminal).await
            }
        };
        // What the CLI held back is shown only for a failure of its own, which has shown it.
        discard_held_log();
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

    /// Reads off the terminal how a session at it reads and draws, and asks it for its
    /// background when the prompts will have a band.
    fn terminal(&self) -> Terminal {
        let mode = input_mode(
            std::io::stdin().is_terminal(),
            std::io::stdout().is_terminal(),
        );
        let colorize = mode != InputMode::Lines && self.ctx.should_colorize();
        let palette = Palette::detect(|name| std::env::var(name).ok());
        let styled = (colorize && std::io::stderr().is_terminal()).then_some(palette);
        let banded = styled.is_some() && mode == InputMode::Editor && palette == Palette::Rich;
        Terminal {
            mode,
            colorize,
            palette,
            loader: Loader::detect(|name| std::env::var(name).ok()),
            styled,
            background: banded
                .then(|| backdrop::Query::send(|name| std::env::var(name).ok()))
                .flatten(),
        }
    }

    fn fetcher(&self, session: &Session) -> AgentFetcher {
        AgentFetcher {
            ctx: self.ctx.clone(),
            owner: session.owner.clone(),
            tool: session.tool.clone(),
            definition: session.definition.clone(),
            with_timeout: session.helper_timeout,
            runtime: tokio::runtime::Handle::current(),
            busy: session.busy.clone(),
        }
    }

    /// The agent's history file, or history for this session only when the file cannot be used.
    fn history(&self, session: &Session) -> SessionHistory {
        let Some(agent) = &session.owner.agent_id else {
            return SessionHistory::in_memory();
        };
        let path = history_file(
            self.ctx.config_dir(),
            self.ctx.worker_service_url().as_str(),
            &session.owner.application_name.to_string(),
            &session.owner.environment_name.to_string(),
            &agent.component_id.0.to_string(),
            &agent.agent_id,
        );
        SessionHistory::open(&path).unwrap_or_else(|error| {
            log_warn(format!(
                "the command history in {} cannot be used ({error}); this session's commands \
                 will not be kept",
                path.display()
            ));
            SessionHistory::in_memory()
        })
    }

    /// `-c`: one submission. On success the process output is exactly the script's.
    async fn run_once(&self, session: &Session, script: &str) -> u8 {
        let key = IdempotencyKey::fresh();
        match self.submit(session, script, &key).await.0 {
            Submission::Completed(result) => match write_output(&result, false) {
                Ok(()) => exit_code(Outcome::Ran(result.exit_code)),
                Err(error) => output_lost(result.exit_code, &error),
            },
            Submission::Failed(failure) => {
                self.report_failure(session, &failure, &key);
                if matches!(failure, CallFailure::Unknown(_)) {
                    self.report_unknown(session, &key).await;
                }
                exit_code(Outcome::NotRun)
            }
            Submission::Interrupted(outcome) => {
                self.report_interrupted(session, &key, &outcome, None);
                exit_code(Outcome::Interrupted)
            }
        }
    }

    async fn run_interactive(&self, session: &mut Session, terminal: Terminal) -> u8 {
        let Terminal {
            mode,
            colorize,
            palette,
            loader,
            styled,
            background,
        } = terminal;
        let at_terminal = mode != InputMode::Lines;
        // The band behind every prompt, a shade off the terminal's own background, and what was
        // typed while the session connected.
        let asked = background.is_some();
        let (band, typed_ahead) = background.map(backdrop::Query::finish).unwrap_or_default();
        // The terminal was asked and has not said: its answer may still come, as input.
        let answer_pending = asked && band.is_none();
        if let Some(palette) = styled {
            log_preformatted(look::banner(&session.agent, &session.tool, palette));
        } else if at_terminal {
            logln(banner(&session.agent, &session.tool));
        }
        let completions = Completions::new(Arc::new(self.fetcher(session)), &session.cwd);
        let mut input = match mode {
            InputMode::Editor => {
                // Fetched once and in the background, so command names are coloured from the
                // first prompt on.
                let loader = completions.clone();
                tokio::task::spawn_blocking(move || {
                    loader.load_commands();
                });
                if colorize {
                    // The list of completions marks the agent's tools among the command names.
                    let tool_handler = self.ctx.tool_handler();
                    let owner = session.owner.clone();
                    let known = completions.clone();
                    tokio::spawn(async move {
                        if let Ok(tools) = tool_handler.registered_tools(&owner).await {
                            known.set_tools(tools.iter().filter_map(|tool| {
                                tool.commands.nodes.first().map(|root| root.name.clone())
                            }));
                        }
                    });
                }
                let mut editor = editor::build(
                    colorize.then_some(palette),
                    self.history(session),
                    completions.clone(),
                );
                if !typed_ahead.is_empty() {
                    editor.run_edit_commands(&[EditCommand::InsertString(typed_ahead)]);
                }
                Input::Editor(Some(Box::new(editor)))
            }
            InputMode::PromptedLines => Input::lines(true),
            InputMode::Lines => Input::lines(false),
        };
        let mut history_failed = false;
        // The indicator belongs to the editor. It is drawn as wide as the window is at the time.
        let show_progress = mode == InputMode::Editor && std::io::stderr().is_terminal();

        let mut last_status = 0;
        let mut last_elapsed = None;
        let mut branch: Option<String> = None;
        // The branch may be another one than the one read: none was read yet, or a command ran
        // that could have changed it.
        let mut branch_stale = true;
        loop {
            let line = prompt(&session.agent, &session.cwd, last_status, |part, text| {
                paint(colorize, part, text)
            });
            let editor = if colorize && input.is_editor() {
                let (readiness, queue) = self.agent_state(session).await;
                if readiness == Readiness::Ready {
                    session.busy.store(false, Ordering::Relaxed);
                }
                if branch_stale {
                    // A branch that may be the wrong one is not shown.
                    branch = None;
                    // It is read with a command on the agent, so only when the agent has git
                    // and would run the command at once.
                    if readiness == Readiness::Ready
                        && !session.cwd.is_empty()
                        && completions
                            .commands()
                            .is_some_and(|commands| commands.contains("git"))
                    {
                        let fetcher = self.fetcher(session);
                        let cwd = session.cwd.clone();
                        branch = tokio::task::spawn_blocking(move || git::branch(&fetcher, &cwd))
                            .await
                            .unwrap_or(None);
                        branch_stale = false;
                    }
                }
                let columns =
                    crossterm::terminal::size().map_or(80, |(columns, _)| usize::from(columns));
                let (room, with_result) = look::layout(
                    columns,
                    look::result_width(last_status, last_elapsed, queue),
                );
                SshPrompt {
                    left: format!(
                        "{}\n{}",
                        look::context(
                            readiness,
                            &session.agent,
                            &session.cwd,
                            branch.as_deref(),
                            palette,
                            band.as_deref(),
                            room,
                        ),
                        look::marker(last_status == 0)
                    ),
                    right: if with_result {
                        look::result(last_status, last_elapsed, queue, palette, band.as_deref())
                    } else {
                        String::new()
                    },
                    continuation: look::CONTINUATION,
                }
            } else {
                SshPrompt::plain(line.clone())
            };
            let prompt = PromptText {
                editor,
                line,
                spaced: colorize,
            };
            let line = match input.read(prompt).await {
                Ok(ReadLine::Line(line)) if answer_pending => strip_background_reply(&line),
                Ok(ReadLine::Line(line)) => line,
                Ok(ReadLine::Cancelled) => continue,
                Ok(ReadLine::End) => return last_status,
                Ok(ReadLine::Interrupted) => {
                    // The read of the next line is still waiting on its own thread and would
                    // keep the process alive until the input ends, so the exit is made here.
                    let _ = std::io::stdout().flush();
                    std::process::exit(i32::from(exit_code(Outcome::Interrupted)));
                }
                Err(error) => {
                    // What cannot be read cannot be run, and neither can what comes after it.
                    set_log_output(Output::Stderr);
                    log_error(format!(
                        "the commands could not be read, so the rest of them were not run: \
                         {error:#}"
                    ));
                    return exit_code(Outcome::NotRun);
                }
            };
            if runs_nothing(&line) {
                continue;
            }
            match local_command(&line) {
                Some(LocalCommand::Exit(status)) => return status.unwrap_or(last_status),
                // Only a person at a terminal asks the session; piped lines all go to the tool.
                Some(LocalCommand::Help) if at_terminal => {
                    log_preformatted(help_text(&session.tool, session.timeout));
                    continue;
                }
                Some(LocalCommand::Tools) if at_terminal => {
                    self.print_tools(session).await;
                    continue;
                }
                _ => {}
            }

            let key = IdempotencyKey::fresh();
            let ticker = show_progress.then(|| {
                Ticker::start(colorize.then_some((palette, loader)), || {
                    crossterm::terminal::size().map_or(80, |(columns, _)| columns)
                })
            });
            let started = Instant::now();
            let cwd_before = session.cwd.clone();
            let (mut submission, fingerprint) = self.submit(session, &line, &key).await;
            // Erased before anything else is written.
            drop(ticker);
            // Ctrl+C came for a command that had already finished: its result is there to show.
            if at_terminal
                && matches!(submission, Submission::Interrupted(CancelOutcome::Finished))
                && let Some(result) = self.finished_result(session, &key).await
            {
                match styled {
                    Some(palette) => log_preformatted(look::finished(palette)),
                    None => logln(interrupted_message(
                        &CancelOutcome::Finished,
                        &session.agent,
                        session.timeout,
                    )),
                }
                submission = Submission::Completed(result);
            }
            last_elapsed = Some(started.elapsed());
            self.note_agent(session, fingerprint);
            match submission {
                Submission::Completed(result) => {
                    session.busy.store(false, Ordering::Relaxed);
                    if let Err(error) = write_output(&result, styled.is_some()) {
                        log_error(format!(
                            "the command's output could not be written: {error}"
                        ));
                    }
                    if at_terminal {
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
                        self.report_unknown(session, &key).await;
                    }
                    if at_terminal {
                        // Typed for the agent while the command ran; the shell this session
                        // returns to must not be handed it.
                        discard_typed_ahead();
                    }
                    return exit_code(Outcome::NotRun);
                }
                Submission::Failed(failure) => {
                    self.report_failure(session, &failure, &key);
                    last_status = exit_code(Outcome::NotRun);
                    if matches!(&failure, CallFailure::Named { name, .. } if name == "invalid-cwd")
                    {
                        // Piped commands were written for the directory they asked for, and
                        // nobody is there to read that it changed: none of them runs elsewhere.
                        if !at_terminal {
                            logln(
                                "The directory cannot be used, so the commands after this one \
                                 were not run.",
                            );
                            return last_status;
                        }
                        self.reset_cwd(session);
                    }
                }
                Submission::Interrupted(outcome) => {
                    self.report_interrupted(session, &key, &outcome, styled);
                    if !at_terminal {
                        return exit_code(Outcome::Interrupted);
                    }
                    if matches!(outcome, CancelOutcome::Running { .. }) {
                        // Helper scripts would wait behind the command that was left running.
                        session.busy.store(true, Ordering::Relaxed);
                    }
                    last_status = exit_code(Outcome::Interrupted);
                }
            }
            branch_stale |= git::changes_branch(&line, &cwd_before, &session.cwd);
            // Any command may have changed the agent's files or the session's directory.
            completions.command_finished(&session.cwd);
            if !history_failed && let Err(error) = input.sync_history() {
                history_failed = true;
                log_warn(format!("the command history could not be saved: {error}"));
            }
        }
    }

    /// What Golem says the agent is doing and how many commands wait on it. A read of the
    /// agent's metadata, not a call on the agent.
    async fn agent_state(&self, session: &Session) -> (Readiness, u64) {
        let Some(agent) = &session.owner.agent_id else {
            return (Readiness::Unknown, 0);
        };
        let lookup = async {
            let clients = self.ctx.golem_clients().await.ok()?;
            clients
                .worker
                .get_worker_metadata(&agent.component_id.0, &agent.agent_id)
                .await
                .ok()
        };
        match tokio::time::timeout(STATUS_WAIT, lookup).await {
            Ok(Some(metadata)) => (
                readiness(&metadata.status),
                metadata.pending_invocation_count,
            ),
            _ => (Readiness::Unknown, 0),
        }
    }

    /// Submits one script from the remembered directory as a scalar `run` call. With the outcome
    /// comes the fingerprint of the agent that answered, when one did.
    async fn submit(
        &self,
        session: &Session,
        script: &str,
        key: &IdempotencyKey,
    ) -> (Submission, Option<Uuid>) {
        let unbuilt = |reason: String| {
            let failure = CallFailure::Rpc(format!("cannot build the call: {reason}"));
            (Submission::Failed(failure), None)
        };
        let argv = run_argv(&session.cwd, session.timeout, script);
        let (command_path, input) = match argv::parse(&session.definition, &argv) {
            Ok(ParsedToolArguments::Invoke {
                command_path,
                input,
            }) => match ExternalTypedSchemaValue::try_from(*input) {
                Ok(input) => (command_path, input),
                Err(error) => return unbuilt(error.to_string()),
            },
            Ok(ParsedToolArguments::Help(_)) => {
                return unbuilt("the tool answered with its help".to_string());
            }
            Err(error) => return unbuilt(error),
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
        let response = tokio::select! {
            response = call => Some(response),
            _ = tokio::signal::ctrl_c() => None,
        };
        match response {
            Some(Ok(response)) => {
                let submission = match decode_result(response.result) {
                    Ok(result) => Submission::Completed(result),
                    Err(failure) => Submission::Failed(failure),
                };
                (submission, response.agent_fingerprint)
            }
            Some(Err(error)) => (Submission::Failed(invoke_failure(error)), None),
            None => (
                Submission::Interrupted(self.cancel(session, key).await),
                None,
            ),
        }
    }

    /// The result of a call that Golem says has finished, looked up under its key. `None` when
    /// it cannot be had in time, which leaves the lookup command to the user.
    async fn finished_result(&self, session: &Session, key: &IdempotencyKey) -> Option<BashResult> {
        let tool_handler = self.ctx.tool_handler();
        let lookup = tool_handler.invoke_tool_scalar(
            &session.owner,
            &session.tool,
            vec![RUN.to_string()],
            None,
            key,
            NativeToolInvocationMode::Lookup,
            None,
        );
        let response = tokio::time::timeout(CANCEL_WAIT, lookup).await.ok()?.ok()?;
        decode_result(response.result).ok()
    }

    /// Says so when another agent answers than the one the session started on: it was deleted
    /// and made again under the same name, and nothing of the old one is left.
    fn note_agent(&self, session: &mut Session, fingerprint: Option<Uuid>) {
        let Some(fingerprint) = fingerprint else {
            return;
        };
        if session
            .fingerprint
            .as_ref()
            .is_some_and(|known| *known != fingerprint)
        {
            log_warn(format!(
                "{} was deleted and made again since the last command. This is a new agent: \
                 the old one's files and directories are gone.",
                shown(&session.agent)
            ));
        }
        session.fingerprint = Some(fingerprint);
    }

    /// After an outcome nobody saw: when the agent has failed, that is why and there is nothing
    /// to look up; otherwise how to look the result up later.
    async fn report_unknown(&self, session: &Session, key: &IdempotencyKey) {
        if self.agent_state(session).await.0 == Readiness::Failed {
            log_error(failed_agent_notice(&session.agent));
        } else {
            self.report_lookup(session, key);
        }
    }

    /// Asks Golem to cancel the call Ctrl+C stopped waiting for, and reads the answer.
    async fn cancel(&self, session: &Session, key: &IdempotencyKey) -> CancelOutcome {
        let request = cancel_call(&self.ctx, &session.owner, key);
        classify_cancel(match tokio::time::timeout(CANCEL_WAIT, request).await {
            Ok(Ok(cancelled)) => Ok(cancelled),
            Ok(Err(error)) => Err(match error_response(&error) {
                Some((_, _, messages)) if !messages.is_empty() => messages.join("; "),
                Some((_, rendered, _)) => rendered,
                None => format!("{error:#}"),
            }),
            Err(_) => Err("no answer to the cancel request".to_string()),
        })
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
            shown(&session.cwd).into_owned()
        };
        logln(format!("Continuing from {place}; the command was not run."));
    }

    fn report_failure(&self, session: &Session, failure: &CallFailure, key: &IdempotencyKey) {
        set_log_output(Output::Stderr);
        // These messages are the tool's and the server's words: the session writes them, so
        // nothing in them may act on the terminal.
        let tool = shown(&session.tool);
        match failure {
            CallFailure::Named { name, detail } => log_error(format!(
                "`{tool}` refused the command with `{}`: {}",
                shown(name),
                shown_message(detail)
            )),
            CallFailure::Denied(message) => {
                log_error(format!(
                    "the command was denied: {}",
                    shown_message(message)
                ));
            }
            CallFailure::Rpc(message) => {
                log_error(format!("the tool call failed: {}", shown_message(message)));
            }
            CallFailure::Unknown(message) => {
                log_error(format!(
                    "the tool call's outcome is unknown: {}",
                    shown_message(message)
                ));
            }
            CallFailure::Undecodable(message) => {
                log_error(format!(
                    "unexpected result from `{tool}`: {}",
                    shown_message(message)
                ));
                logln(format!(
                    "The directory the command ended in is unknown, so the session ends. \
                     Idempotency key: {}",
                    key.value
                ));
            }
        }
    }

    fn report_interrupted(
        &self,
        session: &Session,
        key: &IdempotencyKey,
        outcome: &CancelOutcome,
        styled: Option<Palette>,
    ) {
        set_log_output(Output::Stderr);
        match (outcome, styled) {
            (CancelOutcome::Cancelled, Some(palette)) => {
                log_preformatted(look::cancelled(palette));
            }
            (CancelOutcome::Running { error }, Some(palette)) => {
                log_preformatted(look::detached(
                    &session.agent,
                    &time_limit(session.timeout),
                    error.as_deref(),
                    palette,
                ));
            }
            (CancelOutcome::Finished, Some(palette)) => {
                log_preformatted(look::finished(palette));
            }
            _ => logln(interrupted_message(
                outcome,
                &session.agent,
                session.timeout,
            )),
        }
        // A result this session did not print can still be had.
        if !matches!(outcome, CancelOutcome::Cancelled) {
            self.report_lookup(session, key);
        }
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

    /// `tools`: the tools bound to the session's agent.
    async fn print_tools(&self, session: &Session) {
        let listed = tokio::select! {
            listed = tokio::time::timeout(TOOLS_WAIT, self.bound_tools(session)) => listed,
            _ = tokio::signal::ctrl_c() => {
                logln("Stopped waiting for the list of tools.");
                return;
            }
        };
        match listed {
            Ok(Ok(tools)) => log_preformatted(tools_listing(&tools)),
            Ok(Err(error)) => log_error(format!(
                "the tools could not be listed: {}",
                shown_message(&format!("{error:#}"))
            )),
            Err(_) => log_error(format!(
                "the tools could not be listed: no answer in {} seconds",
                TOOLS_WAIT.as_secs()
            )),
        }
    }

    /// The name and summary of every tool registered in the agent's environment that Golem
    /// resolves for this agent, by name.
    async fn bound_tools(&self, session: &Session) -> anyhow::Result<Vec<(String, String)>> {
        let tool_handler = self.ctx.tool_handler();
        let mut names: Vec<String> = tool_handler
            .registered_tools(&session.owner)
            .await?
            .iter()
            .filter_map(|tool| tool.commands.nodes.first().map(|root| root.name.clone()))
            .collect();
        names.sort();
        names.dedup();
        let described = futures_util::future::join_all(
            names
                .iter()
                .map(|name| tool_handler.describe_bound_tool(&session.owner, name)),
        )
        .await;

        let mut tools = Vec::new();
        let mut first_error = None;
        for (name, described) in names.into_iter().zip(described) {
            match described {
                Ok(described) => {
                    let summary = described
                        .definition
                        .commands
                        .nodes
                        .first()
                        .map(|root| root.doc.summary.clone())
                        .unwrap_or_default();
                    tools.push((name, summary));
                }
                // Registered in the environment, but not bound to this agent.
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        match first_error {
            // The session's own tool is bound, so finding none means the lookups failed.
            Some(error) if tools.is_empty() => Err(error),
            _ => Ok(tools),
        }
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

/// Writes the script's stdout and stderr to the process's. They are unchanged unless
/// `dim_stderr` is set, which marks stderr at an interactive prompt.
fn write_output(result: &BashResult, dim_stderr: bool) -> std::io::Result<()> {
    let write = || {
        let mut stdout = std::io::stdout().lock();
        stdout.write_all(result.stdout.as_bytes())?;
        stdout.flush()?;
        let text = if dim_stderr {
            dimmed(&result.stderr)
        } else {
            result.stderr.as_str().into()
        };
        let mut stderr = std::io::stderr().lock();
        stderr.write_all(text.as_bytes())?;
        stderr.flush()
    };
    match write() {
        // The reader has gone, as with `| head`: nobody is left to tell.
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        written => written,
    }
}

/// The script ran, but not all of its output could be written. That is this process failing,
/// so it says so and does not exit 0.
fn output_lost(status: u8, error: &std::io::Error) -> u8 {
    set_log_output(Output::Stderr);
    log_error(format!(
        "the command's output could not be written: {error}"
    ));
    if status == 0 { 1 } else { status }
}

/// Throws away what was typed and not yet read. A session that ends by itself would otherwise
/// leave it to the shell it returns to, which would run it there.
#[cfg(unix)]
fn discard_typed_ahead() {
    let _ = rustix::termios::tcflush(std::io::stdin(), rustix::termios::QueueSelector::IFlush);
}

#[cfg(not(unix))]
fn discard_typed_ahead() {}

/// Completes once the terminal on stdin has gone. The line editor does not notice that by
/// itself and would read end-of-input in a loop.
#[cfg(unix)]
async fn terminal_gone() {
    use rustix::event::{PollFd, PollFlags, Timespec, poll};

    loop {
        tokio::time::sleep(TERMINAL_CHECK).await;
        let stdin = std::io::stdin();
        // Asking for input as well: some systems report a hang-up only with a request.
        let mut waiting = [PollFd::new(&stdin, PollFlags::IN)];
        let at_once = Timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        if poll(&mut waiting, Some(&at_once)).is_ok()
            && waiting[0]
                .revents()
                .intersects(PollFlags::HUP | PollFlags::ERR | PollFlags::NVAL)
        {
            return;
        }
    }
}

#[cfg(not(unix))]
async fn terminal_gone() {
    std::future::pending().await
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

/// What an agent's status means for the next command.
fn readiness(status: &AgentStatus) -> Readiness {
    match status {
        AgentStatus::Idle => Readiness::Ready,
        AgentStatus::Running
        | AgentStatus::Suspended
        | AgentStatus::Interrupted
        | AgentStatus::Retrying => Readiness::Busy,
        AgentStatus::Failed | AgentStatus::Exited => Readiness::Failed,
    }
}

enum Input {
    /// A terminal: the editor, taken out while a blocking read runs.
    Editor(Option<Box<Reedline>>),
    /// Plain lines, gathered into whole commands, after the prompt when `prompt` is set.
    Lines {
        lines: Lines<BufReader<Stdin>>,
        prompt: bool,
        gathered: Gathered,
        /// How many lines were read, to say where reading failed.
        read: usize,
    },
}

/// The prompt of one read.
struct PromptText {
    /// What the line editor draws.
    editor: SshPrompt,
    /// The prompt as one line, for input without the editor.
    line: String,
    /// Whether an empty line sets the editor's prompt apart from what is above it.
    spaced: bool,
}

enum ReadLine {
    Line(String),
    /// Ctrl+C cleared the line.
    Cancelled,
    /// Ctrl+D or end of input.
    End,
    /// Ctrl+C while piped input was awaited: there is no line to clear, so the session ends.
    Interrupted,
}

impl Input {
    fn lines(prompt: bool) -> Self {
        Input::Lines {
            lines: BufReader::new(tokio::io::stdin()).lines(),
            prompt,
            gathered: Gathered::default(),
            read: 0,
        }
    }

    fn is_editor(&self) -> bool {
        matches!(self, Input::Editor(_))
    }

    /// Writes the editor's history to its file.
    fn sync_history(&mut self) -> std::io::Result<()> {
        match self {
            Input::Editor(Some(editor)) => editor.sync_history(),
            _ => Ok(()),
        }
    }

    async fn read(&mut self, prompt: PromptText) -> anyhow::Result<ReadLine> {
        let PromptText {
            editor: shown,
            line: prompt,
            spaced,
        } = prompt;
        if let Input::Editor(slot) = self {
            let mut editor = slot
                .take()
                .expect("the editor is returned after every read");
            if spaced {
                let _ = writeln!(std::io::stderr().lock());
            }
            let reading = tokio::task::spawn_blocking(move || {
                let signal = editor.read_line(&shown);
                (editor, signal)
            });
            let (editor, signal) = tokio::select! {
                read = reading => read?,
                // With the terminal gone nobody can type, and the editor would spin on its
                // closed input. The exit is what the hang-up signal does when it is not ignored.
                _ = terminal_gone() => std::process::exit(129),
            };
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
            gathered,
            read,
        } = self
        else {
            unreachable!("the editor either returned or was replaced")
        };
        loop {
            if *show_prompt {
                let mut stderr = std::io::stderr().lock();
                // A command that goes on has the marker the editor would show.
                let _ = if gathered.is_empty() {
                    write!(stderr, "{prompt}")
                } else {
                    write!(stderr, "{PLAIN_CONTINUATION}")
                };
                let _ = stderr.flush();
            }
            let line = tokio::select! {
                line = lines.next_line() => line,
                // The session answers Ctrl+C itself, so waiting for a line has to as well.
                _ = tokio::signal::ctrl_c() => {
                    if !*show_prompt {
                        return Ok(ReadLine::Interrupted);
                    }
                    // At a prompt it clears the line, as in the editor.
                    *gathered = Gathered::default();
                    let _ = writeln!(std::io::stderr().lock());
                    return Ok(ReadLine::Cancelled);
                }
            };
            let line = match line {
                Ok(Some(line)) => line,
                // Input that ends inside a command: bash is given what there is and reports it.
                Ok(None) => return Ok(gathered.finish().map_or(ReadLine::End, ReadLine::Line)),
                Err(error) => return Err(anyhow!("line {}: {error}", *read + 1)),
            };
            *read += 1;
            // Without the editor, a late answer to its cursor-position query arrives as input.
            let line = if *show_prompt {
                strip_cursor_reports(&line)
            } else {
                line
            };
            if let Some(command) = gathered.push(&line) {
                return Ok(ReadLine::Line(command));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Readiness, readiness, within};
    use crate::command::{GolemCliCommand, GolemCliSubcommand};
    use clap::Parser;
    use std::cell::Cell;
    use std::time::Duration;
    use test_r::test;

    #[test]
    fn a_call_that_does_not_answer_in_time_is_given_up() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();

        let gave_up = Cell::new(false);
        let late: Option<()> = runtime.block_on(within(
            Duration::from_millis(20),
            std::future::pending(),
            async { gave_up.set(true) },
        ));
        assert_eq!(late, None);
        assert!(gave_up.get());

        let gave_up = Cell::new(false);
        let prompt = runtime.block_on(within(Duration::from_secs(30), async { 7 }, async {
            gave_up.set(true)
        }));
        assert_eq!(prompt, Some(7));
        assert!(!gave_up.get());
    }

    #[test]
    fn an_agents_status_says_whether_the_next_command_can_run() {
        use golem_common::model::AgentStatus;
        for (status, expected) in [
            (AgentStatus::Idle, Readiness::Ready),
            (AgentStatus::Running, Readiness::Busy),
            (AgentStatus::Suspended, Readiness::Busy),
            (AgentStatus::Interrupted, Readiness::Busy),
            (AgentStatus::Retrying, Readiness::Busy),
            (AgentStatus::Failed, Readiness::Failed),
            (AgentStatus::Exited, Readiness::Failed),
        ] {
            assert_eq!(readiness(&status), expected, "{status:?}");
        }
    }

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
