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
use std::fmt::Display;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, OnceLock, RwLock};

use terminal_size::terminal_size;
use textwrap::WordSplitter;
use tracing::debug;

static LOG_STATE: LazyLock<RwLock<LogState>> = LazyLock::new(RwLock::default);
static LOG_STATE_BUFFER: LazyLock<RwLock<Vec<String>>> = LazyLock::new(RwLock::default);
static TRACING_SUPPRESSIONS: AtomicUsize = AtomicUsize::new(0);
static STDOUT_RESERVATIONS: AtomicUsize = AtomicUsize::new(0);
static STDERR_RESERVATIONS: AtomicUsize = AtomicUsize::new(0);
static TERMINAL_WIDTH: OnceLock<Option<usize>> = OnceLock::new();
/// Columns kept free at the right edge when wrapping logged text. Callers that
/// pre-format multi-line output must reserve it too, else their lines re-wrap.
pub static WRAP_PADDING: usize = 2;

/// One level of log indentation. Callers that pre-format multi-line output reuse
/// it to reproduce the same geometry (see `text::fmt::field_value_width`).
pub const INDENT: &str = "  ";

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
}

struct LogState {
    indents: Vec<Option<String>>,
    stashed_indents: Vec<Option<String>>,
    calculated_indent: String,
    max_width: Option<usize>,
    output: Output,
}

impl LogState {
    pub fn new() -> Self {
        Self {
            indents: Vec::new(),
            stashed_indents: Vec::new(),
            calculated_indent: String::new(),
            max_width: terminal_width_opt().map(|w| w - WRAP_PADDING),
            output: Output::Stdout,
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
        self.max_width =
            terminal_width_opt().map(|w| w - WRAP_PADDING - self.calculated_indent.len());
    }

    fn set_output(&mut self, output: Output) {
        let switching_from_buffered_to_err =
            self.output == Output::BufferedUntilErr && output == Output::Stderr;

        self.output = output;

        if switching_from_buffered_to_err {
            let mut buffer = LOG_STATE_BUFFER.write().unwrap();
            for line in buffer.iter() {
                match reservation_aware_output(Output::Stderr) {
                    Output::Stdout => print_line(line),
                    Output::Stderr => eprint_line(line),
                    _ => {}
                }
            }
            buffer.clear();
        }
    }
}

impl Default for LogState {
    fn default() -> Self {
        Self::new()
    }
}

pub struct LogIndent {
    stash: bool,
}

impl LogIndent {
    pub fn new() -> Self {
        LOG_STATE.write().unwrap().inc_indent(None);
        Self { stash: false }
    }

    pub fn prefix<S: AsRef<str>>(prefix: S) -> Self {
        LOG_STATE.write().unwrap().inc_indent(Some(prefix.as_ref()));
        Self { stash: false }
    }

    pub fn stash() -> Self {
        LOG_STATE.write().unwrap().stash_indent();
        Self { stash: true }
    }
}

impl Default for LogIndent {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for LogIndent {
    fn drop(&mut self) {
        let mut state = LOG_STATE.write().unwrap();
        if self.stash {
            state.pop_indent();
        } else {
            state.dec_indent();
        }
    }
}

pub struct LogOutput {
    prev_output: Output,
}

impl LogOutput {
    pub fn new(output: Output) -> Self {
        let prev_output = LOG_STATE.read().unwrap().output;
        LOG_STATE.write().unwrap().set_output(output);
        Self { prev_output }
    }
}

impl Drop for LogOutput {
    fn drop(&mut self) {
        LOG_STATE.write().unwrap().set_output(self.prev_output);
    }
}

pub struct RawOutputReservation {
    stdout: bool,
    stderr: bool,
}

impl RawOutputReservation {
    pub fn new(stdout: bool, stderr: bool) -> Self {
        if stdout {
            STDOUT_RESERVATIONS.fetch_add(1, Ordering::Relaxed);
        }
        if stderr {
            STDERR_RESERVATIONS.fetch_add(1, Ordering::Relaxed);
        }
        Self { stdout, stderr }
    }
}

impl Drop for RawOutputReservation {
    fn drop(&mut self) {
        if self.stdout {
            STDOUT_RESERVATIONS.fetch_sub(1, Ordering::Relaxed);
        }
        if self.stderr {
            STDERR_RESERVATIONS.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

pub(crate) fn reservation_aware_output(output: Output) -> Output {
    let stdout_reserved = STDOUT_RESERVATIONS.load(Ordering::Relaxed) != 0;
    let stderr_reserved = STDERR_RESERVATIONS.load(Ordering::Relaxed) != 0;
    match output {
        Output::Stdout if stdout_reserved && !stderr_reserved => Output::Stderr,
        Output::Stderr if stderr_reserved && !stdout_reserved => Output::Stdout,
        Output::Stdout if stdout_reserved => Output::None,
        Output::Stderr if stderr_reserved => Output::None,
        output => output,
    }
}

pub struct TracingSuppression;

impl TracingSuppression {
    pub fn new() -> Self {
        TRACING_SUPPRESSIONS.fetch_add(1, Ordering::Relaxed);
        Self
    }
}

impl Default for TracingSuppression {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TracingSuppression {
    fn drop(&mut self) {
        TRACING_SUPPRESSIONS.fetch_sub(1, Ordering::Relaxed);
    }
}

pub enum TracingWriter {
    Stderr(std::io::Stderr),
    Sink(std::io::Sink),
}

impl Write for TracingWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Stderr(writer) => writer.write(buffer),
            Self::Sink(writer) => writer.write(buffer),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Stderr(writer) => writer.flush(),
            Self::Sink(writer) => writer.flush(),
        }
    }
}

static STDOUT_CLOSED: AtomicBool = AtomicBool::new(false);
static STDERR_CLOSED: AtomicBool = AtomicBool::new(false);

pub fn is_closed_pipe(error: &std::io::Error) -> bool {
    error.kind() == ErrorKind::BrokenPipe
}

/// Writes the bytes and flushes, unless the reader of the stream has already closed the pipe.
/// A write that finds the pipe closed is dropped, and so are all later writes: the command goes
/// on and ends with its own exit code. Any other write error is returned.
fn write_unless_closed(
    closed: &AtomicBool,
    out: &mut impl Write,
    bytes: &[u8],
) -> std::io::Result<()> {
    if closed.load(Ordering::Relaxed) {
        return Ok(());
    }
    match out.write_all(bytes).and_then(|()| out.flush()) {
        Err(error) if is_closed_pipe(&error) => {
            closed.store(true, Ordering::Relaxed);
            Ok(())
        }
        result => result,
    }
}

/// Whether the reader of stdout has closed the pipe. A command that prints a long or endless
/// result stops when this is true, because nobody reads the rest.
pub fn stdout_closed() -> bool {
    STDOUT_CLOSED.load(Ordering::Relaxed)
}

fn line_bytes(line: impl Display) -> Vec<u8> {
    format!("{line}\n").into_bytes()
}

#[derive(Clone, Copy)]
enum StandardStream {
    Stdout,
    Stderr,
}

impl StandardStream {
    fn name(self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
        }
    }

    fn closed(self) -> &'static AtomicBool {
        match self {
            Self::Stdout => &STDOUT_CLOSED,
            Self::Stderr => &STDERR_CLOSED,
        }
    }
}

fn try_write_to(stream: StandardStream, bytes: &[u8]) -> std::io::Result<()> {
    match stream {
        StandardStream::Stdout => {
            write_unless_closed(stream.closed(), &mut std::io::stdout().lock(), bytes)
        }
        StandardStream::Stderr => {
            write_unless_closed(stream.closed(), &mut std::io::stderr().lock(), bytes)
        }
    }
}

/// A write error other than a closed pipe panics, as `println!` does.
fn write_to(stream: StandardStream, bytes: &[u8]) {
    if let Err(error) = try_write_to(stream, bytes) {
        panic!("failed printing to {}: {error}", stream.name());
    }
}

/// Like `println!`, but drops the line when the reader of stdout has gone.
pub fn print_line(line: impl Display) {
    write_to(StandardStream::Stdout, &line_bytes(line));
}

/// Like `eprintln!`, but drops the line when the reader of stderr has gone.
pub fn eprint_line(line: impl Display) {
    write_to(StandardStream::Stderr, &line_bytes(line));
}

/// Like `print!`, but drops the text when the reader of stdout has gone.
pub fn print_text(text: impl Display) {
    write_to(StandardStream::Stdout, text.to_string().as_bytes());
}

/// Like `eprint!`, but drops the text when the reader of stderr has gone.
pub fn eprint_text(text: impl Display) {
    write_to(StandardStream::Stderr, text.to_string().as_bytes());
}

/// Writes raw bytes to stdout, dropping them when the reader has gone. Any other write error
/// panics.
pub fn print_bytes(bytes: &[u8]) {
    write_to(StandardStream::Stdout, bytes);
}

/// Writes raw bytes to stdout, dropping them when the reader has gone. Any other write error is
/// returned.
pub fn try_print_bytes(bytes: &[u8]) -> std::io::Result<()> {
    try_write_to(StandardStream::Stdout, bytes)
}

/// Handles the outcome of a library call that printed a message by itself. A closed pipe is
/// ignored, so that the command goes on to end with its own exit code. Any other write error
/// panics.
pub fn ignore_closed_pipe(result: std::io::Result<()>) {
    if let Err(error) = result
        && !is_closed_pipe(&error)
    {
        panic!("failed printing a message: {error}");
    }
}

pub fn tracing_writer() -> TracingWriter {
    if TRACING_SUPPRESSIONS.load(Ordering::Relaxed) == 0 {
        TracingWriter::Stderr(std::io::stderr())
    } else {
        TracingWriter::Sink(std::io::sink())
    }
}

pub fn set_log_output(output: Output) {
    debug!(output=?output, "set log output");
    LOG_STATE.write().unwrap().set_output(output);
}

/// Shows the messages held back by [`Output::BufferedUntilErr`] now, and stops holding later
/// ones. For when the user has to see and act on a message, such as a link to log in with.
pub fn release_held_log() {
    let mut state = LOG_STATE.write().unwrap();
    if state.output == Output::BufferedUntilErr {
        state.set_output(Output::Stderr);
    }
}

/// Drops the messages held back by [`Output::BufferedUntilErr`], for a command that ended
/// without an error of the CLI's own.
pub fn discard_held_log() {
    if LOG_STATE.read().unwrap().output == Output::BufferedUntilErr {
        LOG_STATE_BUFFER.write().unwrap().clear();
    }
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
    let state = LOG_STATE.read().unwrap();
    strip_ansi_escapes::strip_str(&state.calculated_indent)
        .chars()
        .count()
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
    let state = LOG_STATE.read().unwrap();
    let indent = &state.calculated_indent;
    for line in text.lines() {
        match reservation_aware_output(state.output) {
            Output::Stdout => print_line(format_args!("{indent}{line}")),
            Output::Stderr => eprint_line(format_args!("{indent}{line}")),
            Output::None => {}
            Output::TracingDebug => debug!("{indent}{line}"),
            Output::BufferedUntilErr => {
                LOG_STATE_BUFFER
                    .write()
                    .unwrap()
                    .push(format!("{indent}{line}"));
            }
        }
    }
}

/// Prints a comfy-table correctly inside the current log indent context.
///
/// Accepts any [`std::fmt::Display`] value; delegates to [`log_preformatted`].
pub fn log_table(table: impl std::fmt::Display) {
    log_preformatted_internal(&table.to_string());
}

pub fn logln_internal(message: &str) {
    let state = LOG_STATE.read().unwrap();

    let lines = match state.max_width {
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

    for line in lines {
        match reservation_aware_output(state.output) {
            Output::Stdout => print_line(format_args!("{}{}", state.calculated_indent, line)),
            Output::Stderr => eprint_line(format_args!("{}{}", state.calculated_indent, line)),
            Output::None => {}
            Output::TracingDebug => {
                debug!("{}{}", state.calculated_indent, line);
            }
            Output::BufferedUntilErr => {
                let mut buffer = LOG_STATE_BUFFER.write().unwrap();
                buffer.push(format!("{}{}", state.calculated_indent, line));
            }
        }
    }
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
        error_message_for_output, ignore_closed_pipe, line_bytes, message_for_output,
        write_unless_closed,
    };
    use anyhow::anyhow;
    use std::io::{Error, ErrorKind, Write};
    use std::sync::atomic::{AtomicBool, Ordering};
    use test_r::test;

    #[derive(Default)]
    struct FakeWriter {
        fail_with: Option<ErrorKind>,
        fail_on_flush: bool,
        written: Vec<u8>,
        write_calls: usize,
    }

    impl Write for FakeWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.write_calls += 1;
            match self.fail_with {
                Some(kind) if !self.fail_on_flush => Err(Error::from(kind)),
                _ => {
                    self.written.extend_from_slice(buf);
                    Ok(buf.len())
                }
            }
        }

        fn flush(&mut self) -> std::io::Result<()> {
            match self.fail_with {
                Some(kind) if self.fail_on_flush => Err(Error::from(kind)),
                _ => Ok(()),
            }
        }
    }

    #[test]
    fn a_write_to_a_closed_pipe_is_dropped_and_remembered() {
        for fail_on_flush in [false, true] {
            let closed = AtomicBool::new(false);
            let mut out = FakeWriter {
                fail_with: Some(ErrorKind::BrokenPipe),
                fail_on_flush,
                ..Default::default()
            };
            write_unless_closed(&closed, &mut out, b"line\n").unwrap();
            assert!(closed.load(Ordering::Relaxed));
        }
    }

    #[test]
    fn a_write_after_the_reader_closed_is_skipped() {
        let closed = AtomicBool::new(true);
        let mut out = FakeWriter::default();
        write_unless_closed(&closed, &mut out, b"line\n").unwrap();
        assert_eq!(out.write_calls, 0);
    }

    #[test]
    fn any_other_write_error_is_returned_and_does_not_close_the_stream() {
        for fail_on_flush in [false, true] {
            let closed = AtomicBool::new(false);
            let mut out = FakeWriter {
                fail_with: Some(ErrorKind::PermissionDenied),
                fail_on_flush,
                ..Default::default()
            };
            let error = write_unless_closed(&closed, &mut out, b"line\n").unwrap_err();
            assert_eq!(error.kind(), ErrorKind::PermissionDenied);
            assert!(!closed.load(Ordering::Relaxed));
        }
    }

    #[test]
    fn a_normal_write_produces_the_exact_line() {
        let closed = AtomicBool::new(false);
        let mut out = FakeWriter::default();
        write_unless_closed(&closed, &mut out, &line_bytes("  indented line")).unwrap();
        assert_eq!(out.written, b"  indented line\n");
        assert!(!closed.load(Ordering::Relaxed));
    }

    #[test]
    fn a_message_printed_by_a_library_ignores_only_a_closed_pipe() {
        ignore_closed_pipe(Ok(()));
        ignore_closed_pipe(Err(Error::from(ErrorKind::BrokenPipe)));
        assert!(
            std::panic::catch_unwind(|| {
                ignore_closed_pipe(Err(Error::from(ErrorKind::PermissionDenied)))
            })
            .is_err()
        );
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
