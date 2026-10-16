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
use std::future::Future;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, OnceLock, RwLock};
use tokio::task::JoinHandle;

use terminal_size::terminal_size;
use textwrap::WordSplitter;
use tracing::{Instrument, Span, debug};

static GLOBAL_LOG_CONTEXT: LazyLock<LogContext> = LazyLock::new(|| LogContext::new(Output::Stdout));
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
pub(crate) enum Output {
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
    pub(crate) fn new(output: Output) -> Self {
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
                match reservation_aware_output(Output::Stderr) {
                    Output::Stdout => print_line(line),
                    Output::Stderr => eprint_line(line),
                    _ => {}
                }
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
        match reservation_aware_output(output) {
            Output::Stdout => print_line(line),
            Output::Stderr => eprint_line(line),
            Output::None => {}
            Output::TracingDebug => debug!("{line}"),
            Output::BufferedUntilErr | Output::Captured => {
                inner.buffer.push(line);
            }
        }
    }
}
/// An isolated formatted-log stream used by embedded frontends such as the TUI.
pub struct LogCapture {
    context: LogContext,
}

impl LogCapture {
    pub fn new() -> Self {
        Self {
            context: LogContext::captured(),
        }
    }

    pub async fn scope<F>(&self, future: F) -> F::Output
    where
        F: Future,
    {
        self.context.scope(future).await
    }

    pub fn spawn<F>(&self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.context.spawn(future)
    }

    pub fn lines(&self) -> Vec<String> {
        self.context.buffered_lines()
    }

    pub fn take_lines(&self) -> Vec<String> {
        self.context.take_buffered_lines()
    }
}

impl Default for LogCapture {
    fn default() -> Self {
        Self::new()
    }
}

/// Controls messages held by the active command until they are needed or can be discarded.
pub(crate) struct DeferredLog;

impl DeferredLog {
    pub(crate) fn release() {
        let context = active_log_context();
        if context.output() == Output::BufferedUntilErr {
            context.set_output(Output::Stderr);
        }
    }

    pub(crate) fn discard() {
        let context = active_log_context();
        if context.output() == Output::BufferedUntilErr {
            context.take_buffered_lines();
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

pub(crate) struct LogOutput {
    context: LogContext,
    prev_output: Output,
}

impl LogOutput {
    pub(crate) fn new(output: Output) -> Self {
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

pub(crate) struct RawOutputReservation {
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

pub(crate) struct TracingSuppression;

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

/// Exclusive access to physical standard streams for a command that emits protocol or tool bytes.
pub(crate) struct RawOutput {
    _reservation: RawOutputReservation,
    _tracing_suppression: Option<TracingSuppression>,
}

impl RawOutput {
    pub(crate) fn reserve(stdout: bool, stderr: bool) -> Self {
        Self {
            _reservation: RawOutputReservation::new(stdout, stderr),
            _tracing_suppression: stderr.then(TracingSuppression::new),
        }
    }
}

/// Process-level behavior when stdout or stderr is no longer readable.
pub(crate) struct OutputLifecycle;

impl OutputLifecycle {
    pub(crate) fn keep_running() {
        keep_running_on_closed_output();
    }

    pub(crate) fn watch_stdout() {
        watch_for_closed_stdout();
    }

    pub(crate) fn handle_print(result: std::io::Result<()>) {
        end_on_closed_pipe(result);
    }
}

pub enum TracingWriter {
    Stderr(std::io::Stderr),
    Sink(std::io::Sink),
}

impl Write for TracingWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let result = match self {
            Self::Stderr(writer) => writer.write(buffer),
            Self::Sink(writer) => writer.write(buffer),
        };
        match result {
            Err(error) if is_closed_pipe(&error) => {
                end_for_closed_output();
                Ok(buffer.len())
            }
            result => result,
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let result = match self {
            Self::Stderr(writer) => writer.flush(),
            Self::Sink(writer) => writer.flush(),
        };
        match result {
            Err(error) if is_closed_pipe(&error) => {
                end_for_closed_output();
                Ok(())
            }
            result => result,
        }
    }
}

/// Exit code that a shell shows for a process that SIGPIPE has ended.
pub const CLOSED_OUTPUT_EXIT_CODE: u8 = 141;

static KEEP_RUNNING_ON_CLOSED_OUTPUT: AtomicBool = AtomicBool::new(false);

pub fn is_closed_pipe(error: &std::io::Error) -> bool {
    error.kind() == ErrorKind::BrokenPipe
}

/// Makes a closed output drop the text instead of ending the process, for a command that
/// serves requests and must outlive the reader of its console.
pub fn keep_running_on_closed_output() {
    KEEP_RUNNING_ON_CLOSED_OUTPUT.store(true, Ordering::Relaxed);
}

/// How long a command that holds work back (see [`ClosedOutputHold`]) has to finish that work
/// after the reader of its output has gone. The work of `agent invoke` is to tell the server
/// that nobody consumes the streams any more: it waits up to 3 s for the server to answer and
/// up to 3 s more to close the session. The grace time is longer than both together, so that
/// the work is never cut short by the end of the process.
pub const CLOSED_OUTPUT_GRACE: std::time::Duration = std::time::Duration::from_secs(10);

static OUTPUT_HOLDS: AtomicUsize = AtomicUsize::new(0);
static READER_GONE_RECORDED: AtomicBool = AtomicBool::new(false);
static READER_GONE: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// What to do once the reader of stdout or stderr has gone.
#[derive(Debug, PartialEq, Eq)]
enum EndAction {
    KeepRunning,
    EndNow,
    /// Some command holds work back: let it run, and end the process after the grace time.
    Defer {
        start_backstop: bool,
    },
}

fn decide_end(keep_running: bool, holds: usize, reader_gone_recorded: bool) -> EndAction {
    if keep_running {
        EndAction::KeepRunning
    } else if holds > 0 {
        EndAction::Defer {
            start_backstop: !reader_gone_recorded,
        }
    } else {
        EndAction::EndNow
    }
}

fn end_process_for_closed_output() -> ! {
    leave_raw_mode();
    std::process::exit(CLOSED_OUTPUT_EXIT_CODE.into())
}

/// Ends the process with [`CLOSED_OUTPUT_EXIT_CODE`], as SIGPIPE would, once the reader of
/// stdout or stderr has gone. Switches off the raw mode that a prompt of the command switched
/// on. A command that holds work back gets [`CLOSED_OUTPUT_GRACE`] to finish it first: the
/// call records that the reader has gone and returns, and the writers drop their text. Returns
/// also for a command that keeps running.
pub fn end_for_closed_output() {
    match decide_end(
        KEEP_RUNNING_ON_CLOSED_OUTPUT.load(Ordering::Relaxed),
        OUTPUT_HOLDS.load(Ordering::Relaxed),
        READER_GONE_RECORDED.load(Ordering::Relaxed),
    ) {
        EndAction::KeepRunning => {}
        EndAction::EndNow => end_process_for_closed_output(),
        EndAction::Defer { start_backstop } => {
            if start_backstop && !READER_GONE_RECORDED.swap(true, Ordering::Relaxed) {
                std::thread::spawn(|| {
                    std::thread::sleep(CLOSED_OUTPUT_GRACE);
                    end_process_for_closed_output()
                });
                READER_GONE.notify_waiters();
            }
        }
    }
}

/// Held by a command that has work to do when the reader of its output has gone, such as telling
/// a server that nobody consumes a stream. While a hold exists, a closed output does not end the
/// process at once: the command learns of it through [`ClosedOutputHold::reader_gone`] and
/// finishes that work. The process ends with [`CLOSED_OUTPUT_EXIT_CODE`] when the last hold is
/// released, and after [`CLOSED_OUTPUT_GRACE`] at the latest.
pub struct ClosedOutputHold(());

impl ClosedOutputHold {
    pub fn new() -> Self {
        OUTPUT_HOLDS.fetch_add(1, Ordering::Relaxed);
        Self(())
    }

    /// Completes once the reader of stdout or stderr has gone.
    pub async fn reader_gone(&self) {
        let notified = READER_GONE.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if READER_GONE_RECORDED.load(Ordering::Relaxed) {
            return;
        }
        notified.await;
    }
}

impl Default for ClosedOutputHold {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ClosedOutputHold {
    fn drop(&mut self) {
        let remaining_holds = OUTPUT_HOLDS.fetch_sub(1, Ordering::Relaxed) - 1;
        if release_ends_process(
            remaining_holds,
            READER_GONE_RECORDED.load(Ordering::Relaxed),
        ) {
            end_process_for_closed_output();
        }
    }
}

/// Whether the release of a hold ends the process: the reader has gone while a command held work
/// back, and no other hold is left. The command can have ended on its own meanwhile, for example
/// on a rejection that it could not print, and its own exit code must not replace the one of a
/// closed output.
fn release_ends_process(remaining_holds: usize, reader_gone_recorded: bool) -> bool {
    remaining_holds == 0 && reader_gone_recorded
}

/// Switches the raw mode of the terminal off, if this process switched it on. A prompt switches
/// it on and cannot switch it off when the process ends at once. A terminal mode that another
/// program set, such as a pager that reads the output, stays as it is.
#[cfg(unix)]
fn leave_raw_mode() {
    if crossterm::terminal::is_raw_mode_enabled().unwrap_or(false) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

#[cfg(not(unix))]
fn leave_raw_mode() {}

/// What happened to bytes written to a standard stream.
#[derive(Debug, PartialEq, Eq)]
enum Written {
    Done,
    ReaderGone,
}

fn outcome_of(result: std::io::Result<()>) -> std::io::Result<Written> {
    match result {
        Ok(()) => Ok(Written::Done),
        Err(error) if is_closed_pipe(&error) => Ok(Written::ReaderGone),
        Err(error) => Err(error),
    }
}

/// Writes the bytes and flushes. A pipe whose reader has gone is reported as such; any other
/// write error is returned.
fn write_and_flush(out: &mut impl Write, bytes: &[u8]) -> std::io::Result<Written> {
    outcome_of(out.write_all(bytes).and_then(|()| out.flush()))
}

/// Finds out without a write whether the reader of stdout has gone. Only a pipe or a socket
/// whose peer has closed reports an error or a hang-up here; a terminal, a file, a pipe with a
/// live reader and a socket whose peer only stopped writing do not.
#[cfg(unix)]
pub fn stdout_reader_gone() -> bool {
    peer_gone(rustix::stdio::stdout())
}

#[cfg(unix)]
fn peer_gone(fd: std::os::fd::BorrowedFd<'_>) -> bool {
    use rustix::event::{PollFd, PollFlags, poll};

    let mut fds = [PollFd::from_borrowed_fd(fd, PollFlags::OUT)];
    matches!(poll(&mut fds, Some(&rustix::time::Timespec::default())), Ok(ready) if ready > 0)
        && fds[0].revents().intersects(PollFlags::ERR | PollFlags::HUP)
}

/// How often the watch for a closed stdout asks the operating system.
#[cfg(unix)]
const STDOUT_WATCH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

/// Ends the process when the reader of stdout has gone, also while the command writes nothing.
/// Where the operating system cannot be asked, the process ends at its next write.
#[cfg(unix)]
pub fn watch_for_closed_stdout() {
    tokio::spawn(async {
        let mut interval = tokio::time::interval(STDOUT_WATCH_INTERVAL);
        loop {
            interval.tick().await;
            if stdout_reader_gone() {
                end_for_closed_output();
                return;
            }
        }
    });
}

#[cfg(not(unix))]
pub fn watch_for_closed_stdout() {}

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
}

fn try_write_to(stream: StandardStream, bytes: &[u8]) -> std::io::Result<()> {
    let written = match stream {
        StandardStream::Stdout => write_and_flush(&mut std::io::stdout().lock(), bytes),
        StandardStream::Stderr => write_and_flush(&mut std::io::stderr().lock(), bytes),
    }?;
    if written == Written::ReaderGone {
        end_for_closed_output();
    }
    Ok(())
}

/// A write error other than a closed pipe panics, as `println!` does.
fn write_to(stream: StandardStream, bytes: &[u8]) {
    if let Err(error) = try_write_to(stream, bytes) {
        panic!("failed printing to {}: {error}", stream.name());
    }
}

/// Like `println!`, but ends the process when the reader of stdout has gone.
pub fn print_line(line: impl Display) {
    write_to(StandardStream::Stdout, &line_bytes(line));
}

/// Like `eprintln!`, but ends the process when the reader of stderr has gone.
pub fn eprint_line(line: impl Display) {
    write_to(StandardStream::Stderr, &line_bytes(line));
}

/// Like `print!`, but ends the process when the reader of stdout has gone.
pub fn print_text(text: impl Display) {
    write_to(StandardStream::Stdout, text.to_string().as_bytes());
}

/// Like `eprint!`, but ends the process when the reader of stderr has gone.
pub fn eprint_text(text: impl Display) {
    write_to(StandardStream::Stderr, text.to_string().as_bytes());
}

/// Writes raw bytes to stdout, ending the process when the reader has gone. Any other write
/// error panics.
pub fn print_bytes(bytes: &[u8]) {
    write_to(StandardStream::Stdout, bytes);
}

/// Writes raw bytes to stdout, ending the process when the reader has gone. Any other write
/// error is returned.
pub fn try_print_bytes(bytes: &[u8]) -> std::io::Result<()> {
    try_write_to(StandardStream::Stdout, bytes)
}

/// Handles the outcome of a library call that printed a message by itself: a closed pipe ends
/// the process, any other write error panics.
pub fn end_on_closed_pipe(result: std::io::Result<()>) {
    match outcome_of(result) {
        Ok(Written::Done) => {}
        Ok(Written::ReaderGone) => end_for_closed_output(),
        Err(error) => panic!("failed printing a message: {error}"),
    }
}

pub fn tracing_writer() -> TracingWriter {
    if TRACING_SUPPRESSIONS.load(Ordering::Relaxed) == 0 {
        TracingWriter::Stderr(std::io::stderr())
    } else {
        TracingWriter::Sink(std::io::sink())
    }
}

pub(crate) fn set_log_output(output: Output) {
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
