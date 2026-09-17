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

use crate::app::error::AppValidationError;
use crate::error::{HintError, NonSuccessfulExit, PipedExitCode};
use anyhow::anyhow;
use camino::{Utf8Path, Utf8PathBuf};
use colored::{ColoredString, Colorize};
use std::borrow::Cow;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, OnceLock, RwLock};
use tokio::task::JoinHandle;

use terminal_size::terminal_size;
use textwrap::WordSplitter;
use tracing::{Instrument, Span, debug};

static GLOBAL_LOG_CONTEXT: LazyLock<LogContext> = LazyLock::new(|| LogContext::new(Output::Stdout));
static TERMINAL_WIDTH: OnceLock<Option<usize>> = OnceLock::new();
/// Columns kept free at the right edge when wrapping logged text. Callers that
/// pre-format multi-line output must reserve it too, else their lines re-wrap.
pub static WRAP_PADDING: usize = 2;

/// One level of log indentation. Callers that pre-format multi-line output reuse
/// it to reproduce the same geometry (see `text::fmt::field_value_width`).
pub const INDENT: &str = "  ";

tokio::task_local! {
    static CURRENT_LOG_CONTEXT: LogContext;
}

/// Returns the terminal width as `Some(width)` or `None` if not detectable.
/// Cached via `OnceLock` — read once at startup for use in `LogState` text-wrapping.
fn terminal_width_opt() -> Option<usize> {
    *TERMINAL_WIDTH.get_or_init(|| terminal_size().map(|(width, _)| width.0 as usize))
}

/// Returns the current terminal width in columns, or 80 if not detectable.
/// Called fresh each time — no caching — so watch-mode adapts to terminal resizes.
pub fn terminal_width() -> u16 {
    terminal_size().map(|(w, _)| w.0).unwrap_or(80)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Output {
    Stdout,
    Stderr,
    None,
    TracingDebug,
    BufferedUntilErr,
    Captured,
}

/// Shared logging state for one logical output stream.
///
/// Cloning a `LogContext`, or spawning multiple tasks with the same context, shares
/// the indent stack, output mode, and captured/buffered lines. Use a separate
/// context for each independently captured concurrent stream. Captured and
/// buffered contexts do not flush automatically; consumers must drain them with
/// `take_buffered_lines`.
#[derive(Clone)]
pub struct LogContext {
    inner: Arc<RwLock<LogContextState>>,
}

struct LogContextState {
    log_state: LogState,
    buffer: Vec<String>,
}

impl LogContext {
    pub fn new(output: Output) -> Self {
        Self {
            inner: Arc::new(RwLock::new(LogContextState {
                log_state: LogState::new(output),
                buffer: Vec::new(),
            })),
        }
    }

    pub fn captured() -> Self {
        Self::new(Output::Captured)
    }

    /// Runs `future` with this context as the active logging stream.
    ///
    /// The context remains shared with other scopes using the same handle.
    pub async fn scope<F>(&self, future: F) -> F::Output
    where
        F: Future,
    {
        CURRENT_LOG_CONTEXT.scope(self.clone(), future).await
    }

    /// Spawns `future` with this context and the current tracing span.
    ///
    /// The spawned task shares this context's indent stack and buffer. Create a
    /// new `LogContext` for each isolated concurrent stream.
    pub fn spawn<F>(&self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let context = self.clone();
        let span = Span::current();
        tokio::spawn(async move {
            CURRENT_LOG_CONTEXT
                .scope(context, future.instrument(span))
                .await
        })
    }

    pub fn buffered_lines(&self) -> Vec<String> {
        self.inner.read().unwrap().buffer.clone()
    }

    pub fn take_buffered_lines(&self) -> Vec<String> {
        let mut inner = self.inner.write().unwrap();
        std::mem::take(&mut inner.buffer)
    }

    fn inc_indent(&self, custom_prefix: Option<&str>) {
        self.inner
            .write()
            .unwrap()
            .log_state
            .inc_indent(custom_prefix);
    }

    fn dec_indent(&self) {
        self.inner.write().unwrap().log_state.dec_indent();
    }

    fn stash_indent(&self) {
        self.inner.write().unwrap().log_state.stash_indent();
    }

    fn pop_indent(&self) {
        self.inner.write().unwrap().log_state.pop_indent();
    }

    fn output(&self) -> Output {
        self.inner.read().unwrap().log_state.output
    }

    fn set_output(&self, output: Output) {
        let mut inner = self.inner.write().unwrap();
        let switching_from_buffered_to_err =
            inner.log_state.output == Output::BufferedUntilErr && output == Output::Stderr;

        inner.log_state.output = output;

        if switching_from_buffered_to_err {
            for line in inner.buffer.drain(..) {
                eprintln!("{}", line);
            }
        }
    }

    fn current_indent_width(&self) -> usize {
        let inner = self.inner.read().unwrap();
        strip_ansi_escapes::strip_str(&inner.log_state.calculated_indent)
            .chars()
            .count()
    }

    fn log_preformatted(&self, text: &str) {
        let mut inner = self.inner.write().unwrap();
        let indent = inner.log_state.calculated_indent.clone();
        let output = inner.log_state.output;
        for line in text.lines() {
            Self::write_line(&mut inner, output, format!("{indent}{line}"));
        }
    }

    fn logln(&self, message: &str) {
        let mut inner = self.inner.write().unwrap();

        let lines = match inner.log_state.max_width {
            Some(width) if width <= message.len() && !message.contains("\n") => {
                textwrap::wrap(
                    message,
                    textwrap::Options::new(width)
                        // deliberately 5 spaces, to make this indent different from normal ones
                        .subsequent_indent("     ")
                        .word_splitter(WordSplitter::NoHyphenation),
                )
            }
            _ => {
                vec![Cow::from(message)]
            }
        };

        let indent = inner.log_state.calculated_indent.clone();
        let output = inner.log_state.output;
        for line in lines {
            Self::write_line(&mut inner, output, format!("{indent}{line}"));
        }
    }

    fn write_line(inner: &mut LogContextState, output: Output, line: String) {
        match output {
            Output::Stdout => println!("{line}"),
            Output::Stderr => eprintln!("{line}"),
            Output::None => {}
            Output::TracingDebug => debug!("{line}"),
            Output::BufferedUntilErr | Output::Captured => {
                inner.buffer.push(line);
            }
        }
    }
}

fn active_log_context() -> LogContext {
    CURRENT_LOG_CONTEXT
        .try_with(Clone::clone)
        .unwrap_or_else(|_| GLOBAL_LOG_CONTEXT.clone())
}

struct LogState {
    indents: Vec<Option<String>>,
    stashed_indents: Vec<Option<String>>,
    calculated_indent: String,
    max_width: Option<usize>,
    output: Output,
}

impl LogState {
    pub fn new(output: Output) -> Self {
        Self {
            indents: Vec::new(),
            stashed_indents: Vec::new(),
            calculated_indent: String::new(),
            max_width: terminal_width_opt().map(|w| w.saturating_sub(WRAP_PADDING)),
            output,
        }
    }

    pub fn inc_indent(&mut self, custom_prefix: Option<&str>) {
        self.indents.push(custom_prefix.map(|p| p.to_string()));
        self.regen_indent_prefix();
    }

    pub fn dec_indent(&mut self) {
        self.indents.pop();
        self.regen_indent_prefix()
    }

    pub fn stash_indent(&mut self) {
        self.stashed_indents.clear();
        std::mem::swap(&mut self.indents, &mut self.stashed_indents);
        self.regen_indent_prefix();
    }

    pub fn pop_indent(&mut self) {
        std::mem::swap(&mut self.indents, &mut self.stashed_indents);
        self.stashed_indents.clear();
        self.regen_indent_prefix();
    }

    fn regen_indent_prefix(&mut self) {
        self.calculated_indent = String::with_capacity(self.indents.len() * INDENT.len());
        for indent in &self.indents {
            self.calculated_indent
                .push_str(indent.as_ref().map(|s| s.as_str()).unwrap_or(INDENT))
        }
        self.max_width = terminal_width_opt().map(|w| {
            w.saturating_sub(WRAP_PADDING)
                .saturating_sub(self.calculated_indent.len())
        });
    }
}

impl Default for LogState {
    fn default() -> Self {
        Self::new(Output::Stdout)
    }
}

pub struct LogIndent {
    context: LogContext,
    stash: bool,
}

impl LogIndent {
    pub fn new() -> Self {
        let context = active_log_context();
        context.inc_indent(None);
        Self {
            context,
            stash: false,
        }
    }

    pub fn prefix<S: AsRef<str>>(prefix: S) -> Self {
        let context = active_log_context();
        context.inc_indent(Some(prefix.as_ref()));
        Self {
            context,
            stash: false,
        }
    }

    pub fn stash() -> Self {
        let context = active_log_context();
        context.stash_indent();
        Self {
            context,
            stash: true,
        }
    }
}

impl Default for LogIndent {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for LogIndent {
    fn drop(&mut self) {
        if self.stash {
            self.context.pop_indent();
        } else {
            self.context.dec_indent();
        }
    }
}

pub struct LogOutput {
    context: LogContext,
    prev_output: Output,
}

impl LogOutput {
    pub fn new(output: Output) -> Self {
        let context = active_log_context();
        let prev_output = context.output();
        context.set_output(output);
        Self {
            context,
            prev_output,
        }
    }
}

impl Drop for LogOutput {
    fn drop(&mut self) {
        self.context.set_output(self.prev_output);
    }
}

pub fn set_log_output(output: Output) {
    debug!(output=?output, "set log output");
    active_log_context().set_output(output);
}

/// Renders an error for structured (JSON / YAML / TOON) output: the full context chain, with
/// any terminal styling removed. Error messages meant for the terminal (e.g. `ServiceError`)
/// are colorized whenever the CLI output is colorized, which must not leak into output that is
/// consumed as data.
pub fn error_message_for_output(error: &anyhow::Error) -> String {
    message_for_output(format_args!("{error:#}"))
}

/// Renders a message for structured output, see `error_message_for_output`.
pub fn message_for_output(message: impl std::fmt::Display) -> String {
    strip_ansi_escapes::strip_str(message.to_string())
}

pub fn log_anyhow_error(error: &anyhow::Error) {
    if error.is::<NonSuccessfulExit>() || error.is::<PipedExitCode>() {
        // NOP
    } else if error
        .downcast_ref::<Arc<anyhow::Error>>()
        .and_then(|err| err.downcast_ref::<AppValidationError>())
        .is_some()
        || error.downcast_ref::<AppValidationError>().is_some()
    {
        // App validation errors are already formatted and usually contain multiple
        // errors (and warns)
        logln("");
        logln(format!("{error:#}"));
    } else {
        log_error(format!("{error:#}"));
    }
}

pub fn log_error<S: AsRef<str>>(message: S) {
    logln(format!(
        "{} {}",
        "error:".log_color_error(),
        message.as_ref()
    ));
}

pub fn log_warn<S: AsRef<str>>(message: S) {
    logln(format!("{} {}", "warn:".log_color_warn(), message.as_ref()));
}

pub fn log_action(action: &str, subject: impl AsRef<str>) {
    logln_internal(&format!(
        "{} {}",
        action.log_color_action(),
        subject.as_ref()
    ));
}

pub fn log_finished_ok(subject: impl AsRef<str>) {
    logln_internal(&format!(
        "{} {} [{}]",
        "Finished".log_color_action(),
        subject.as_ref(),
        "OK".log_color_ok_highlight(),
    ));
}

pub fn log_finished_up_to_date(subject: impl AsRef<str>) {
    logln_internal(&format!(
        "{} {} [{}]",
        "Finished".log_color_action(),
        subject.as_ref(),
        "UP-TO-DATE".log_color_ok_highlight(),
    ));
}

pub fn log_failed_to(subject: impl AsRef<str>) {
    log_error_action(
        "Failed",
        format!("to {} [{}]", subject.as_ref(), "ERROR".log_color_error(),),
    );
}

pub fn logged_failed_to(error: anyhow::Error, subject: impl AsRef<str>) -> anyhow::Result<()> {
    log_failed_to(subject);
    if error.downcast_ref::<HintError>().is_some() {
        Err(error)
    } else {
        let _indent = LogIndent::new();
        log_anyhow_error(&error);
        Err(anyhow!(NonSuccessfulExit))
    }
}

pub fn logged_finished_or_failed_to(
    result: anyhow::Result<()>,
    finished_subject: impl AsRef<str>,
    failed_to_subject: impl AsRef<str>,
) -> anyhow::Result<()> {
    match result {
        Ok(()) => {
            log_finished_ok(finished_subject);
            Ok(())
        }
        Err(err) => logged_failed_to(err, failed_to_subject),
    }
}

pub fn log_warn_action(action: &str, subject: impl AsRef<str>) {
    logln_internal(&format!("{} {}", action.log_color_warn(), subject.as_ref(),));
}

pub fn log_error_action(action: &str, subject: impl AsRef<str>) {
    logln_internal(&format!(
        "{} {}",
        action.log_color_error(),
        subject.as_ref(),
    ));
}

pub fn logln(message: impl AsRef<str>) {
    logln_internal(message.as_ref());
}

pub fn current_indent_width() -> usize {
    active_log_context().current_indent_width()
}

/// Prints pre-formatted multi-line text inside the current log indent context.
///
/// Unlike `logln`, which would only prepend the indent to the first line of a multi-line
/// string, this function prepends the current indent to **every** line and writes directly
/// to the active output channel — bypassing `logln_internal`'s text-wrapping logic, which
/// must not be applied to pre-formatted output.
pub fn log_preformatted(text: impl AsRef<str>) {
    log_preformatted_internal(text.as_ref());
}

fn log_preformatted_internal(text: &str) {
    active_log_context().log_preformatted(text);
}

/// Prints a comfy-table correctly inside the current log indent context.
///
/// Accepts any [`std::fmt::Display`] value; delegates to [`log_preformatted`].
pub fn log_table(table: impl std::fmt::Display) {
    log_preformatted_internal(&table.to_string());
}

pub fn logln_internal(message: &str) {
    active_log_context().logln(message);
}

pub fn log_skipping_up_to_date(subject: impl AsRef<str>) {
    log_warn_action(
        "Skipping",
        format!(
            "{} [{}]",
            subject.as_ref(),
            "UP-TO-DATE".log_color_ok_highlight()
        ),
    );
}

pub trait LogColorize {
    fn as_str(&self) -> impl Colorize;

    fn log_color_action(&self) -> ColoredString {
        self.as_str().green()
    }

    fn log_color_warn(&self) -> ColoredString {
        self.as_str().yellow().bold()
    }

    fn log_color_error(&self) -> ColoredString {
        self.as_str().red().bold()
    }

    fn log_color_highlight(&self) -> ColoredString {
        self.as_str().bold()
    }

    fn log_color_help_group(&self) -> ColoredString {
        self.as_str().bold().underline()
    }

    fn log_color_error_highlight(&self) -> ColoredString {
        self.as_str().bold().red().underline()
    }

    fn log_color_ok_highlight(&self) -> ColoredString {
        self.as_str().bold().green()
    }
}

impl LogColorize for &str {
    fn as_str(&self) -> impl Colorize {
        *self
    }
}

impl LogColorize for String {
    fn as_str(&self) -> impl Colorize {
        self.as_str()
    }
}

impl LogColorize for &Path {
    fn as_str(&self) -> impl Colorize {
        ColoredString::from(self.display().to_string())
    }
}

impl LogColorize for &Utf8Path {
    fn as_str(&self) -> impl Colorize {
        ColoredString::from(self.to_string())
    }
}

impl LogColorize for PathBuf {
    fn as_str(&self) -> impl Colorize {
        ColoredString::from(self.display().to_string())
    }
}

impl LogColorize for Utf8PathBuf {
    fn as_str(&self) -> impl Colorize {
        ColoredString::from(self.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        LogContext, LogIndent, LogOutput, Output, active_log_context, current_indent_width,
        error_message_for_output, log_action, log_preformatted, log_table, logln,
        message_for_output,
    };
    use anyhow::anyhow;
    use std::sync::{LazyLock, Mutex};
    use test_r::test;

    static GLOBAL_LOG_TEST_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    fn plain(lines: Vec<String>) -> Vec<String> {
        lines
            .into_iter()
            .map(strip_ansi_escapes::strip_str)
            .collect()
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Runtime::new().expect("create tokio runtime")
    }

    #[test]
    fn captured_contexts_do_not_mix_concurrent_output() {
        runtime().block_on(async {
            let left = LogContext::captured();
            let right = LogContext::captured();

            let left_handle = left.spawn(async {
                logln("left one");
                tokio::task::yield_now().await;
                logln("left two");
            });
            let right_handle = right.spawn(async {
                logln("right one");
                tokio::task::yield_now().await;
                logln("right two");
            });

            left_handle.await.expect("left task");
            right_handle.await.expect("right task");

            assert_eq!(plain(left.buffered_lines()), ["left one", "left two"]);
            assert_eq!(plain(right.buffered_lines()), ["right one", "right two"]);
        });
    }

    #[test]
    fn indentation_is_preserved_across_await() {
        runtime().block_on(async {
            let context = LogContext::captured();

            context
                .scope(async {
                    let _indent = LogIndent::prefix("> ");
                    logln("before");
                    tokio::task::yield_now().await;
                    logln("after");
                })
                .await;

            assert_eq!(plain(context.buffered_lines()), ["> before", "> after"]);
        });
    }

    #[test]
    fn log_indent_drops_against_creation_context() {
        runtime().block_on(async {
            let first = LogContext::captured();
            let second = LogContext::captured();

            let indent = first.scope(async { LogIndent::prefix("first: ") }).await;

            second
                .scope(async {
                    logln("second before");
                    drop(indent);
                    logln("second after");
                })
                .await;

            first.scope(async { logln("first after") }).await;

            assert_eq!(plain(first.buffered_lines()), ["first after"]);
            assert_eq!(
                plain(second.buffered_lines()),
                ["second before", "second after"]
            );
        });
    }

    #[test]
    fn log_output_drops_against_creation_context() {
        runtime().block_on(async {
            let first = LogContext::captured();
            let second = LogContext::captured();

            let output = first.scope(async { LogOutput::new(Output::None) }).await;

            second
                .scope(async {
                    logln("second before");
                    drop(output);
                    logln("second after");
                })
                .await;

            first
                .scope(async {
                    logln("first captured");
                })
                .await;

            assert_eq!(plain(first.buffered_lines()), ["first captured"]);
            assert_eq!(
                plain(second.buffered_lines()),
                ["second before", "second after"]
            );
        });
    }

    #[test]
    fn captured_output_collects_log_variants() {
        runtime().block_on(async {
            let context = LogContext::captured();

            context
                .scope(async {
                    logln("plain");
                    log_action("Doing", "work");
                    log_preformatted("alpha\nbeta");
                    log_table("table");
                })
                .await;

            assert_eq!(
                plain(context.buffered_lines()),
                ["plain", "Doing work", "alpha", "beta", "table"]
            );
        });
    }

    #[test]
    fn take_buffered_lines_drains_context_buffer() {
        runtime().block_on(async {
            let context = LogContext::captured();

            context
                .scope(async {
                    logln("one");
                    logln("two");
                })
                .await;

            assert_eq!(plain(context.take_buffered_lines()), ["one", "two"]);
            assert!(context.buffered_lines().is_empty());
        });
    }

    #[test]
    fn buffered_until_err_is_context_local() {
        runtime().block_on(async {
            let first = LogContext::new(Output::BufferedUntilErr);
            let second = LogContext::new(Output::BufferedUntilErr);

            first.scope(async { logln("first") }).await;
            second.scope(async { logln("second") }).await;

            assert_eq!(plain(first.buffered_lines()), ["first"]);
            assert_eq!(plain(second.buffered_lines()), ["second"]);
        });
    }

    #[test]
    fn no_scoped_context_uses_global_fallback() {
        let _guard = GLOBAL_LOG_TEST_LOCK.lock().unwrap();
        let global = active_log_context();
        let _output = LogOutput::new(Output::Captured);

        logln("global fallback");
        assert_eq!(plain(global.take_buffered_lines()), ["global fallback"]);
    }

    #[test]
    fn current_indent_width_uses_active_context() {
        runtime().block_on(async {
            let context = LogContext::captured();

            context
                .scope(async {
                    let _indent = LogIndent::prefix("abc");
                    assert_eq!(current_indent_width(), 3);
                })
                .await;
        });
    }

    #[test]
    fn deeply_indented_logging_does_not_underflow_width() {
        runtime().block_on(async {
            let context = LogContext::captured();

            context
                .scope(async {
                    let _indent = LogIndent::prefix("x".repeat(10_000));
                    logln("still logged");
                })
                .await;

            let lines = plain(context.buffered_lines());
            assert_eq!(lines.len(), 1);
            assert!(lines[0].ends_with("still logged"));
        });
    }

    #[test]
    fn messages_for_output_have_no_terminal_styling() {
        // Styling as `colored` emits it when colors are enabled
        let styled = "\u{1b}[1;33mNot found\u{1b}[0m: agent \u{1b}[4mCounter(\"a\")\u{1b}[0m";

        assert_eq!(
            message_for_output(styled),
            "Not found: agent Counter(\"a\")"
        );
        assert_eq!(
            error_message_for_output(
                &anyhow!("{styled}").context("\u{1b}[31mfailed to delete the agent\u{1b}[0m")
            ),
            "failed to delete the agent: Not found: agent Counter(\"a\")"
        );
    }
}
