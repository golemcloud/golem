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

use crate::context::Context;
use crate::model::app_raw::{BuiltinServer, Server};
use crate::tui::TuiEvent;
use crate::tui::nested_cli::{CommandExit, NestedCliRuntime, NestedCliSpec, spawn_nested_cli};
use crate::tui::terminal::TerminalGuard;
use ansi_to_tui::IntoText;
use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind,
};
use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::Duration;

pub fn run(ctx: Arc<Context>) -> anyhow::Result<()> {
    let mut app = TuiApp::from_context(ctx.as_ref());
    let mut terminal = TerminalGuard::enter()?;
    let (event_tx, event_rx) = mpsc::channel::<TuiEvent>();
    spawn_terminal_event_reader(event_tx.clone());

    terminal.draw(|frame| render(frame, &app))?;

    while !app.should_quit {
        let event = event_rx.recv()?;
        app.handle_event(event, &event_tx);
        terminal.draw(|frame| render(frame, &app))?;
    }

    app.cleanup_running_command();
    Ok(())
}

fn spawn_terminal_event_reader(event_tx: Sender<TuiEvent>) {
    thread::spawn(move || {
        loop {
            match event::read() {
                Ok(event) => {
                    if event_tx.send(TuiEvent::Terminal(event)).is_err() {
                        return;
                    }
                }
                Err(_) => return,
            }
        }
    });
}

fn spawn_command_spinner(command_id: u64, event_tx: Sender<TuiEvent>, stop: Arc<AtomicBool>) {
    thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(120));
            if stop.load(Ordering::Relaxed) {
                return;
            }
            if event_tx.send(TuiEvent::SpinnerTick(command_id)).is_err() {
                return;
            }
        }
    });
}

struct TuiApp {
    should_quit: bool,
    active_view: TuiView,
    mode: TuiMode,
    palette: CommandPalette,
    command_options: CommandOptions,
    command_run: Option<CommandRun>,
    next_command_id: u64,
    context: TuiContextInfo,
}

impl TuiApp {
    fn from_context(ctx: &Context) -> Self {
        Self {
            should_quit: false,
            active_view: TuiView::Dashboard,
            mode: TuiMode::Normal,
            palette: CommandPalette::default(),
            command_options: CommandOptions::default(),
            command_run: None,
            next_command_id: 1,
            context: TuiContextInfo::from_context(ctx),
        }
    }

    fn handle_event(&mut self, event: TuiEvent, event_tx: &Sender<TuiEvent>) {
        match event {
            TuiEvent::Terminal(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                self.handle_key_with_events(key, Some(event_tx));
            }
            TuiEvent::Terminal(Event::Resize(cols, rows)) => {
                self.resize_running_command(cols, rows)
            }
            TuiEvent::Terminal(Event::Mouse(mouse)) => self.handle_mouse(mouse),
            TuiEvent::Terminal(_) => {}
            TuiEvent::CommandOutput(bytes) => self.append_command_output(&bytes),
            TuiEvent::CommandOutputClosed(error) => {
                if let Some(error) = error {
                    self.append_local_command_line(format!("output closed: {error}"));
                }
            }
            TuiEvent::CommandExited(exit) => self.finish_command(exit),
            TuiEvent::SpinnerTick(command_id) => self.handle_spinner_tick(command_id),
        }
    }

    #[cfg(test)]
    fn handle_key(&mut self, key: KeyEvent) {
        self.handle_key_with_events(key, None);
    }

    fn handle_key_with_events(&mut self, key: KeyEvent, event_tx: Option<&Sender<TuiEvent>>) {
        match self.mode {
            TuiMode::Normal => self.handle_global_key(key, event_tx),
            TuiMode::Palette => self.handle_palette_key(key, event_tx),
            TuiMode::Help => self.handle_help_key(key),
            TuiMode::CommandInteraction => self.handle_command_interaction_key(key),
        }
    }

    fn handle_global_key(&mut self, key: KeyEvent, event_tx: Option<&Sender<TuiEvent>>) {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true;
            }
            KeyCode::Char('b') => self.start_command(CommandKind::Build, event_tx),
            KeyCode::Char('d') => self.start_command(CommandKind::Deploy, event_tx),
            KeyCode::Char('c') => self.start_command(CommandKind::Clean, event_tx),
            KeyCode::Char('y') => self.command_options.yes = !self.command_options.yes,
            KeyCode::Char('r') => self.command_options.reset = !self.command_options.reset,
            KeyCode::PageUp => self.scroll_output_up(),
            KeyCode::PageDown => self.scroll_output_down(),
            KeyCode::Home => self.scroll_output_top(),
            KeyCode::End => self.scroll_output_bottom(),
            KeyCode::Up if self.active_view == TuiView::Output => self.scroll_output_up_by(1),
            KeyCode::Down if self.active_view == TuiView::Output => self.scroll_output_down_by(1),
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.open_palette();
            }
            KeyCode::Char(':') => self.open_palette(),
            KeyCode::Char('?') => self.mode = TuiMode::Help,
            KeyCode::Char(']') | KeyCode::Tab => self.next_view(),
            KeyCode::Char('[') | KeyCode::BackTab => self.previous_view(),
            KeyCode::Char('1') => self.active_view = TuiView::Dashboard,
            KeyCode::Char('2') => self.active_view = TuiView::Environments,
            KeyCode::Char('3') => self.active_view = TuiView::Components,
            KeyCode::Char('4') => self.active_view = TuiView::Agents,
            KeyCode::Char('5') => self.active_view = TuiView::Output,
            _ => {}
        }
    }

    fn handle_palette_key(&mut self, key: KeyEvent, event_tx: Option<&Sender<TuiEvent>>) {
        match key.code {
            KeyCode::Esc => self.close_palette(),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.close_palette();
            }
            KeyCode::Backspace => self.palette.backspace(),
            KeyCode::Down | KeyCode::Tab => {
                let action_count = filtered_actions(&self.palette.query).len();
                self.palette.next(action_count);
            }
            KeyCode::Up | KeyCode::BackTab => {
                let action_count = filtered_actions(&self.palette.query).len();
                self.palette.previous(action_count);
            }
            KeyCode::Enter => {
                if let Some(action) = filtered_actions(&self.palette.query)
                    .get(self.palette.selected)
                    .copied()
                {
                    self.execute_action(action.kind, event_tx);
                }
            }
            KeyCode::Char(character)
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                self.palette.push(character);
            }
            _ => {}
        }
    }

    fn handle_help_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('?') => self.mode = TuiMode::Normal,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.mode = TuiMode::Normal;
            }
            _ => {}
        }
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) {
        if self.active_view != TuiView::Output {
            return;
        }

        match mouse.kind {
            MouseEventKind::ScrollUp => self.scroll_output_up_by(3),
            MouseEventKind::ScrollDown => self.scroll_output_down_by(3),
            _ => {}
        }
    }

    fn execute_action(&mut self, action: TuiActionKind, event_tx: Option<&Sender<TuiEvent>>) {
        match action {
            TuiActionKind::SelectView(view) => {
                self.active_view = view;
                self.close_palette();
            }
            TuiActionKind::Build => {
                self.close_palette();
                self.start_command(CommandKind::Build, event_tx);
            }
            TuiActionKind::Deploy => {
                self.close_palette();
                self.start_command(CommandKind::Deploy, event_tx);
            }
            TuiActionKind::Clean => {
                self.close_palette();
                self.start_command(CommandKind::Clean, event_tx);
            }
            TuiActionKind::ToggleYes => {
                self.command_options.yes = !self.command_options.yes;
                self.close_palette();
            }
            TuiActionKind::ToggleReset => {
                self.command_options.reset = !self.command_options.reset;
                self.close_palette();
            }
            TuiActionKind::ShowHelp => {
                self.palette.reset();
                self.mode = TuiMode::Help;
            }
            TuiActionKind::Quit => {
                self.close_palette();
                self.should_quit = true;
            }
        }
    }

    fn open_palette(&mut self) {
        self.palette.reset();
        self.mode = TuiMode::Palette;
    }

    fn close_palette(&mut self) {
        self.palette.reset();
        self.mode = self.default_mode();
    }

    fn default_mode(&self) -> TuiMode {
        if self.command_is_running() {
            TuiMode::CommandInteraction
        } else {
            TuiMode::Normal
        }
    }

    fn next_view(&mut self) {
        self.active_view = TuiView::from_index((self.active_view.index() + 1) % TuiView::ALL.len());
    }

    fn previous_view(&mut self) {
        self.active_view = TuiView::from_index(
            (self.active_view.index() + TuiView::ALL.len() - 1) % TuiView::ALL.len(),
        );
    }

    fn handle_command_interaction_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.cancel_or_force_kill_command(),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cancel_or_force_kill_command();
            }
            KeyCode::PageUp => self.scroll_output_up(),
            KeyCode::PageDown => self.scroll_output_down(),
            KeyCode::Home => self.scroll_output_top(),
            KeyCode::End => self.scroll_output_bottom(),
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.open_palette();
            }
            KeyCode::Char('?') => self.mode = TuiMode::Help,
            _ => {
                if let Some(bytes) = encode_key_for_child(key)
                    && let Some(run) = self.command_run.as_mut()
                    && let Some(runtime) = run.runtime.as_mut()
                    && let Err(error) = runtime.write_all(&bytes)
                {
                    run.output
                        .append_local_line(format!("failed to send input: {error:#}"));
                }
            }
        }
    }

    fn start_command(&mut self, kind: CommandKind, event_tx: Option<&Sender<TuiEvent>>) {
        if self.command_is_running() {
            self.append_local_command_line("command already running");
            self.active_view = TuiView::Output;
            return;
        }

        let args = self.command_args(kind);
        let command_id = self.next_command_id;
        self.next_command_id += 1;
        self.active_view = TuiView::Output;
        self.mode = TuiMode::CommandInteraction;
        self.command_run = Some(CommandRun::new(
            command_id,
            kind,
            args.clone(),
            self.command_options,
        ));
        self.append_local_command_line(format!("$ {}", command_display(&args)));

        let Some(event_tx) = event_tx else {
            return;
        };

        let spec = match self.command_spec(args) {
            Ok(spec) => spec,
            Err(error) => {
                self.append_local_command_line(format!("failed to prepare command: {error:#}"));
                self.set_command_status(CommandStatus::Failed);
                self.mode = TuiMode::Normal;
                return;
            }
        };

        match spawn_nested_cli(spec, event_tx.clone()) {
            Ok(runtime) => {
                if let Some(run) = self.command_run.as_mut() {
                    run.runtime = Some(runtime);
                    let stop = Arc::new(AtomicBool::new(false));
                    spawn_command_spinner(command_id, event_tx.clone(), stop.clone());
                    run.spinner_stop = Some(stop);
                }
            }
            Err(error) => {
                self.append_local_command_line(format!("failed to start command: {error:#}"));
                self.set_command_status(CommandStatus::Failed);
                self.mode = TuiMode::Normal;
            }
        }
    }

    fn handle_spinner_tick(&mut self, command_id: u64) {
        if let Some(run) = self.command_run.as_mut()
            && run.id == command_id
            && matches!(
                run.status,
                CommandStatus::Running | CommandStatus::Cancelling
            )
        {
            run.spinner_frame = run.spinner_frame.wrapping_add(1);
        }
    }

    fn command_args(&self, kind: CommandKind) -> Vec<String> {
        let mut args = match kind {
            CommandKind::Build => vec!["build".to_string()],
            CommandKind::Deploy => vec!["deploy".to_string()],
            CommandKind::Clean => vec!["clean".to_string()],
        };
        if self.command_options.yes {
            args.push("--yes".to_string());
        }
        if kind == CommandKind::Deploy && self.command_options.reset {
            args.push("--reset".to_string());
        }
        args
    }

    fn command_spec(&self, args: Vec<String>) -> anyhow::Result<NestedCliSpec> {
        let mut env = HashMap::new();
        env.insert("CLICOLOR_FORCE".to_string(), "1".to_string());
        env.insert("FORCE_COLOR".to_string(), "1".to_string());
        if std::env::var("TERM").is_err() || std::env::var("TERM").is_ok_and(|term| term == "dumb")
        {
            env.insert("TERM".to_string(), "xterm-256color".to_string());
        }

        Ok(NestedCliSpec {
            program: PathBuf::from(crate::binary_path_to_string()?),
            args,
            cwd: crate::fs::current_dir_lexical()?,
            env,
        })
    }

    fn append_command_output(&mut self, bytes: &[u8]) {
        if let Some(run) = self.command_run.as_mut() {
            run.output.append(bytes);
        }
    }

    fn append_local_command_line(&mut self, line: impl AsRef<str>) {
        if let Some(run) = self.command_run.as_mut() {
            run.output.append_local_line(line);
        }
    }

    fn finish_command(&mut self, exit: CommandExit) {
        if let Some(run) = self.command_run.as_mut() {
            if run.status != CommandStatus::Killed {
                run.status = if exit.success {
                    CommandStatus::Succeeded
                } else {
                    CommandStatus::Failed
                };
            }
            run.exit_code = exit.code;
            run.runtime = None;
            run.stop_spinner();
            run.output.append_local_line(match run.status {
                CommandStatus::Succeeded => "command succeeded".to_string(),
                CommandStatus::Failed => {
                    format!("command failed with exit code {}", exit.code.unwrap_or(1))
                }
                CommandStatus::Killed => "command killed".to_string(),
                CommandStatus::Running | CommandStatus::Cancelling => "command exited".to_string(),
            });
        }
        if self.mode == TuiMode::CommandInteraction {
            self.mode = TuiMode::Normal;
        }
    }

    fn cancel_or_force_kill_command(&mut self) {
        let Some(run) = self.command_run.as_mut() else {
            return;
        };

        match run.status {
            CommandStatus::Running => {
                if let Some(runtime) = run.runtime.as_mut() {
                    if let Err(error) = runtime.send_ctrl_c() {
                        run.output
                            .append_local_line(format!("failed to send cancel: {error:#}"));
                    }
                }
                run.status = CommandStatus::Cancelling;
                run.output.append_local_line("cancel requested");
            }
            CommandStatus::Cancelling => {
                if let Some(runtime) = run.runtime.as_mut()
                    && let Err(error) = runtime.kill()
                {
                    run.output
                        .append_local_line(format!("failed to force kill: {error:#}"));
                }
                run.runtime = None;
                run.status = CommandStatus::Killed;
                run.stop_spinner();
                run.output.append_local_line("force killed");
                self.mode = TuiMode::Normal;
            }
            CommandStatus::Succeeded | CommandStatus::Failed | CommandStatus::Killed => {}
        }
    }

    fn cleanup_running_command(&mut self) {
        if let Some(run) = self.command_run.as_mut()
            && let Some(runtime) = run.runtime.as_mut()
        {
            let _ = runtime.kill();
            run.stop_spinner();
        }
    }

    fn command_is_running(&self) -> bool {
        self.command_run.as_ref().is_some_and(|run| {
            matches!(
                run.status,
                CommandStatus::Running | CommandStatus::Cancelling
            )
        })
    }

    fn set_command_status(&mut self, status: CommandStatus) {
        if let Some(run) = self.command_run.as_mut() {
            run.status = status;
            run.runtime = None;
            if !matches!(status, CommandStatus::Running | CommandStatus::Cancelling) {
                run.stop_spinner();
            }
        }
    }

    fn resize_running_command(&mut self, cols: u16, rows: u16) {
        if let Some(run) = self.command_run.as_mut()
            && let Some(runtime) = run.runtime.as_mut()
        {
            let _ = runtime.resize(cols, rows);
        }
    }

    fn scroll_output_up(&mut self) {
        self.scroll_output_up_by(10);
    }

    fn scroll_output_up_by(&mut self, amount: usize) {
        if let Some(run) = self.command_run.as_mut() {
            run.output.scroll_up(amount);
        }
    }

    fn scroll_output_down(&mut self) {
        self.scroll_output_down_by(10);
    }

    fn scroll_output_down_by(&mut self, amount: usize) {
        if let Some(run) = self.command_run.as_mut() {
            run.output.scroll_down(amount);
        }
    }

    fn scroll_output_top(&mut self) {
        if let Some(run) = self.command_run.as_mut() {
            run.output.scroll_top();
        }
    }

    fn scroll_output_bottom(&mut self) {
        if let Some(run) = self.command_run.as_mut() {
            run.output.scroll_bottom();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TuiMode {
    Normal,
    Palette,
    Help,
    CommandInteraction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandKind {
    Build,
    Deploy,
    Clean,
}

impl CommandKind {
    fn title(self) -> &'static str {
        match self {
            Self::Build => "build",
            Self::Deploy => "deploy",
            Self::Clean => "clean",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandStatus {
    Running,
    Cancelling,
    Succeeded,
    Failed,
    Killed,
}

impl CommandStatus {
    fn title(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Cancelling => "cancelling",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Killed => "killed",
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct CommandOptions {
    yes: bool,
    reset: bool,
}

struct CommandRun {
    id: u64,
    kind: CommandKind,
    status: CommandStatus,
    options: CommandOptions,
    args: Vec<String>,
    output: OutputBuffer,
    runtime: Option<NestedCliRuntime>,
    spinner_frame: usize,
    spinner_stop: Option<Arc<AtomicBool>>,
    exit_code: Option<i32>,
}

impl CommandRun {
    fn new(id: u64, kind: CommandKind, args: Vec<String>, options: CommandOptions) -> Self {
        Self {
            id,
            kind,
            status: CommandStatus::Running,
            options,
            args,
            output: OutputBuffer::default(),
            runtime: None,
            spinner_frame: 0,
            spinner_stop: None,
            exit_code: None,
        }
    }

    fn stop_spinner(&mut self) {
        if let Some(stop) = self.spinner_stop.take() {
            stop.store(true, Ordering::Relaxed);
        }
    }
}

struct OutputBuffer {
    lines: Vec<Vec<u8>>,
    pending: Vec<u8>,
    max_lines: usize,
    follow: bool,
    scroll_offset: usize,
}

impl Default for OutputBuffer {
    fn default() -> Self {
        Self {
            lines: Vec::new(),
            pending: Vec::new(),
            max_lines: 2000,
            follow: true,
            scroll_offset: 0,
        }
    }
}

impl OutputBuffer {
    fn append(&mut self, bytes: &[u8]) {
        self.pending.extend_from_slice(bytes);

        while let Some(index) = self.pending.iter().position(|byte| *byte == b'\n') {
            let mut line = self.pending.drain(..=index).collect::<Vec<_>>();
            while matches!(line.last(), Some(b'\n' | b'\r')) {
                line.pop();
            }
            self.push_line(line);
        }

        if self.follow {
            self.scroll_offset = 0;
        }
    }

    fn append_local_line(&mut self, line: impl AsRef<str>) {
        self.push_line(format!("\x1b[2m{}\x1b[0m", line.as_ref()).into_bytes());
    }

    fn visible_lines(&self, height: usize) -> Vec<Vec<u8>> {
        let mut lines = self.lines.clone();
        if !self.pending.is_empty() {
            lines.push(self.pending.clone());
        }
        let total = lines.len();
        let offset = self.scroll_offset.min(total.saturating_sub(height));
        let end = total.saturating_sub(offset);
        let start = end.saturating_sub(height);
        lines[start..end].to_vec()
    }

    fn scroll_up(&mut self, amount: usize) {
        self.follow = false;
        self.scroll_offset =
            (self.scroll_offset + amount).min(self.total_lines().saturating_sub(1));
    }

    fn scroll_down(&mut self, amount: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(amount);
        if self.scroll_offset == 0 {
            self.follow = true;
        }
    }

    fn scroll_top(&mut self) {
        self.follow = false;
        self.scroll_offset = self.total_lines().saturating_sub(1);
    }

    fn scroll_bottom(&mut self) {
        self.follow = true;
        self.scroll_offset = 0;
    }

    fn total_lines(&self) -> usize {
        self.lines.len() + usize::from(!self.pending.is_empty())
    }

    fn push_line(&mut self, line: Vec<u8>) {
        self.lines.push(line);
        if self.lines.len() > self.max_lines {
            let drain_count = self.lines.len() - self.max_lines;
            self.lines.drain(0..drain_count);
        }
    }
}

fn encode_key_for_child(key: KeyEvent) -> Option<Vec<u8>> {
    match key.code {
        KeyCode::Char(character)
            if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
        {
            let mut bytes = [0; 4];
            Some(character.encode_utf8(&mut bytes).as_bytes().to_vec())
        }
        KeyCode::Enter => Some(b"\r".to_vec()),
        KeyCode::Backspace => Some(vec![0x7f]),
        KeyCode::Tab => Some(b"\t".to_vec()),
        KeyCode::Left => Some(b"\x1b[D".to_vec()),
        KeyCode::Right => Some(b"\x1b[C".to_vec()),
        KeyCode::Up => Some(b"\x1b[A".to_vec()),
        KeyCode::Down => Some(b"\x1b[B".to_vec()),
        _ => None,
    }
}

fn command_display(args: &[String]) -> String {
    let mut parts = vec!["golem".to_string()];
    parts.extend(args.iter().cloned());
    parts.join(" ")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TuiView {
    Dashboard,
    Environments,
    Components,
    Agents,
    Output,
}

impl TuiView {
    const ALL: [Self; 5] = [
        Self::Dashboard,
        Self::Environments,
        Self::Components,
        Self::Agents,
        Self::Output,
    ];

    fn title(self) -> &'static str {
        match self {
            Self::Dashboard => "Dashboard",
            Self::Environments => "Environments",
            Self::Components => "Components",
            Self::Agents => "Agents",
            Self::Output => "Output",
        }
    }

    fn placeholder(self) -> &'static str {
        match self {
            Self::Dashboard => "Selected context and quick actions.",
            Self::Environments => "Environment switching will appear here.",
            Self::Components => "Component monitoring will appear here.",
            Self::Agents => "Agent monitoring and management will appear here.",
            Self::Output => "Nested command output will appear here.",
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|view| *view == self).unwrap_or(0)
    }

    fn from_index(index: usize) -> Self {
        Self::ALL[index]
    }
}

#[derive(Debug, Clone, Default)]
struct CommandPalette {
    query: String,
    selected: usize,
}

impl CommandPalette {
    fn reset(&mut self) {
        self.query.clear();
        self.selected = 0;
    }

    fn push(&mut self, character: char) {
        self.query.push(character);
        self.selected = 0;
    }

    fn backspace(&mut self) {
        self.query.pop();
        self.selected = 0;
    }

    fn next(&mut self, action_count: usize) {
        if action_count > 0 {
            self.selected = (self.selected + 1) % action_count;
        }
    }

    fn previous(&mut self, action_count: usize) {
        if action_count > 0 {
            self.selected = (self.selected + action_count - 1) % action_count;
        }
    }
}

#[derive(Debug, Clone)]
struct TuiContextInfo {
    application: String,
    environment: String,
    server: String,
    config_dir: String,
}

impl TuiContextInfo {
    fn from_context(ctx: &Context) -> Self {
        let manifest_environment = ctx.manifest_environment();
        let application = manifest_environment
            .map(|environment| environment.application_name.0.clone())
            .unwrap_or_else(|| "no application manifest".to_string());
        let environment = manifest_environment
            .map(|environment| environment.environment_name.0.clone())
            .unwrap_or_else(|| "profile/default".to_string());
        let server = manifest_environment
            .map(|environment| {
                environment
                    .environment
                    .server
                    .as_ref()
                    .map(format_server)
                    .unwrap_or_else(|| "local".to_string())
            })
            .unwrap_or_else(|| ctx.worker_service_url().to_string());

        Self {
            application,
            environment,
            server,
            config_dir: ctx.config_dir().display().to_string(),
        }
    }
}

fn format_server(server: &Server) -> String {
    match server {
        Server::Builtin(BuiltinServer::Local) => "local".to_string(),
        Server::Builtin(BuiltinServer::Cloud) => "cloud".to_string(),
        Server::Custom(custom) => custom.url.to_string(),
    }
}

fn render(frame: &mut Frame<'_>, app: &TuiApp) {
    let [header, tabs, separator, body, footer_area] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .areas(frame.area());

    render_header(frame, header, app);

    render_tabs(frame, tabs, app.active_view);
    render_separator(frame, separator);

    if app.active_view == TuiView::Output {
        render_output_view(frame, body, app);
    } else {
        render_surface(frame, body);
        let dashboard = Paragraph::new(view_lines(app))
            .style(surface_style())
            .wrap(Wrap { trim: false });
        frame.render_widget(dashboard, body);
    }

    let footer = Paragraph::new(footer_line(app.command_options))
        .style(footer_style())
        .alignment(Alignment::Center);
    frame.render_widget(footer, footer_area);
    render_left_rail(frame, footer_area, footer_rail_style());

    match app.mode {
        TuiMode::Normal => {}
        TuiMode::CommandInteraction => {}
        TuiMode::Palette => render_palette(frame, app),
        TuiMode::Help => render_help(frame),
    }
}

fn render_header(frame: &mut Frame<'_>, area: Rect, app: &TuiApp) {
    let line = Line::from(vec![
        Span::styled("┃ ", header_rail_style()),
        Span::styled("Golem", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("  app:"),
        Span::styled(
            app.context.application.clone(),
            Style::default().fg(Color::Cyan),
        ),
        Span::raw("  env:"),
        Span::styled(
            app.context.environment.clone(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  server:"),
        Span::styled(app.context.server.clone(), Style::default().fg(Color::Cyan)),
    ]);
    frame.render_widget(Paragraph::new(line).style(header_style()), area);
    render_left_rail(frame, area, header_rail_style());
}

fn render_separator(frame: &mut Frame<'_>, area: Rect) {
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("┃", separator_rail_style()),
            Span::raw(" ".repeat(area.width.saturating_sub(1) as usize)),
        ]))
        .style(separator_style()),
        area,
    );
    render_left_rail(frame, area, separator_rail_style());
}

fn render_surface(frame: &mut Frame<'_>, area: Rect) {
    frame.render_widget(Paragraph::new("").style(surface_style()), area);
    render_left_rail(frame, area, surface_rail_style());
}

fn render_left_rail(frame: &mut Frame<'_>, area: Rect, style: Style) {
    for y in area.y..area.y.saturating_add(area.height) {
        frame.buffer_mut()[(area.x, y)]
            .set_symbol("┃")
            .set_style(style);
    }
}

fn header_style() -> Style {
    Style::default().bg(Color::Rgb(24, 28, 35))
}

fn header_rail_style() -> Style {
    Style::default()
        .fg(Color::Cyan)
        .bg(Color::Rgb(24, 28, 35))
        .add_modifier(Modifier::BOLD)
}

fn tabs_style() -> Style {
    Style::default().bg(Color::Rgb(18, 22, 28))
}

fn tabs_rail_style() -> Style {
    Style::default()
        .fg(Color::Rgb(90, 200, 250))
        .bg(Color::Rgb(18, 22, 28))
}

fn separator_style() -> Style {
    Style::default().bg(Color::Rgb(32, 38, 48))
}

fn separator_rail_style() -> Style {
    Style::default()
        .fg(Color::Rgb(140, 180, 220))
        .bg(Color::Rgb(32, 38, 48))
}

fn surface_style() -> Style {
    Style::default().bg(Color::Rgb(13, 17, 23))
}

fn surface_rail_style() -> Style {
    Style::default()
        .fg(Color::Rgb(58, 67, 82))
        .bg(Color::Rgb(13, 17, 23))
}

fn command_status_bg_style() -> Style {
    Style::default().bg(Color::Rgb(24, 28, 35))
}

fn command_rail_style() -> Style {
    Style::default()
        .fg(Color::Yellow)
        .bg(Color::Rgb(24, 28, 35))
        .add_modifier(Modifier::BOLD)
}

fn footer_style() -> Style {
    Style::default()
        .fg(Color::DarkGray)
        .bg(Color::Rgb(18, 22, 28))
}

fn footer_rail_style() -> Style {
    Style::default()
        .fg(Color::Rgb(58, 67, 82))
        .bg(Color::Rgb(18, 22, 28))
}

fn prefixed_line(text: impl Into<String>) -> Line<'static> {
    Line::from(vec![
        Span::styled("┃ ", surface_rail_style()),
        Span::raw(text.into()),
    ])
}

fn render_help(frame: &mut Frame<'_>) {
    let area = centered_rect(70, 60, frame.area());
    let lines = vec![
        Line::from(vec![Span::styled(
            "Keyboard Shortcuts",
            Style::default().add_modifier(Modifier::BOLD),
        )]),
        Line::default(),
        Line::from("Global"),
        Line::from("  Ctrl-P / :     Command palette"),
        Line::from("  ?              Help"),
        Line::from("  b / d / c      Build / deploy / clean"),
        Line::from("  y / r          Toggle --yes / --reset"),
        Line::from("  q / Esc        Quit"),
        Line::from("  Ctrl-C         Quit"),
        Line::from("  ] / Tab        Next view"),
        Line::from("  [ / Shift-Tab  Previous view"),
        Line::from("  1..5           Jump to view"),
        Line::default(),
        Line::from("Palette"),
        Line::from("  Type           Filter commands"),
        Line::from("  Up / Down      Move selection"),
        Line::from("  Enter          Execute selected action"),
        Line::from("  Esc / Ctrl-C   Close palette"),
        Line::default(),
        Line::from("Command"),
        Line::from("  Type           Send input to command"),
        Line::from("  Esc / Ctrl-C   Cancel command, press again to force kill"),
        Line::from("  PageUp/Down    Scroll output"),
        Line::from("  Home / End     Top / latest output"),
        Line::from("  Mouse wheel    Scroll output"),
        Line::default(),
        Line::from("Output"),
        Line::from("  Up / Down      Scroll output when no command is running"),
        Line::from("  PageUp/Down    Scroll output"),
        Line::from("  Home / End     Top / latest output"),
        Line::from("  Mouse wheel    Scroll output"),
        Line::default(),
        Line::from("Press Esc to close this help."),
    ];

    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" Help ")),
        area,
    );
}

fn render_tabs(frame: &mut Frame<'_>, area: Rect, active_view: TuiView) {
    let mut spans = vec![Span::styled("┃ ", tabs_rail_style())];

    for view in TuiView::ALL {
        if spans.len() > 1 {
            spans.push(Span::raw("  "));
        }

        let title = view.title().to_string();
        let style = if view == active_view {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        spans.push(Span::styled(title, style));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)).style(tabs_style()), area);
    render_left_rail(frame, area, tabs_rail_style());
}

fn view_lines(app: &TuiApp) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(vec![
            Span::styled("┃ ", surface_rail_style()),
            Span::styled(
                app.active_view.title(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::default(),
        Line::from(vec![
            Span::styled("┃ ", surface_rail_style()),
            Span::raw(app.active_view.placeholder()),
        ]),
    ];

    if app.active_view == TuiView::Dashboard {
        lines.extend([
            Line::default(),
            prefixed_line(format!("Application : {}", app.context.application)),
            prefixed_line(format!("Environment : {}", app.context.environment)),
            prefixed_line(format!("Server      : {}", app.context.server)),
            prefixed_line(format!("Config dir  : {}", app.context.config_dir)),
            Line::default(),
            prefixed_line("Scaffold ready. Next steps: command execution and live data."),
        ]);
    }

    lines
}

fn render_output_view(frame: &mut Frame<'_>, area: Rect, app: &TuiApp) {
    render_surface(frame, area);
    let has_input = show_command_input(app);
    let [summary_area, output_area, input_area] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .areas(area);

    let run = app.command_run.as_ref();
    let summary = match run {
        Some(run) => command_status_line(run),
        None => Line::from(vec![
            Span::styled("┃ ", command_rail_style()),
            fixed_span("none", 7, Style::default().fg(Color::DarkGray)),
            Span::raw(" "),
            fixed_span("idle", 10, Style::default().fg(Color::DarkGray)),
            Span::raw(" | "),
            flag_span("yes", app.command_options.yes, false),
            Span::raw(" "),
            flag_span("reset", app.command_options.reset, true),
            Span::raw(" | "),
            fixed_span("b build / d deploy / c clean", 30, Style::default()),
            Span::raw(" | "),
            fixed_span("ready", 17, Style::default().fg(Color::DarkGray)),
        ]),
    };
    frame.render_widget(
        Paragraph::new(summary).style(command_status_bg_style()),
        summary_area,
    );
    render_left_rail(frame, summary_area, command_rail_style());

    let output_lines = run
        .map(|run| render_output_lines(&run.output, output_area.height as usize))
        .unwrap_or_else(|| vec![prefixed_line("Output will appear here.")]);
    frame.render_widget(
        Paragraph::new(output_lines)
            .style(surface_style())
            .wrap(Wrap { trim: false }),
        output_area,
    );
    render_left_rail(frame, output_area, surface_rail_style());
    if let Some(run) = run {
        render_output_scrollbar(frame, output_area, &run.output);
    }

    if has_input {
        let prompt = "stdin ";
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("┃ ", command_rail_style()),
                Span::raw(prompt),
            ]))
            .style(surface_style()),
            input_area,
        );
        render_left_rail(frame, input_area, command_rail_style());
        frame.set_cursor_position((input_area.x + 2 + prompt.len() as u16, input_area.y));
    }
}

fn show_command_input(app: &TuiApp) -> bool {
    app.mode == TuiMode::CommandInteraction
        && app.command_run.as_ref().is_some_and(|run| {
            matches!(
                run.status,
                CommandStatus::Running | CommandStatus::Cancelling
            ) && !run.options.yes
        })
}

fn render_output_scrollbar(frame: &mut Frame<'_>, area: Rect, output: &OutputBuffer) {
    let visible_height = area.height as usize;
    let total = output.total_lines();
    if total <= visible_height || visible_height == 0 {
        return;
    }

    let position = output_scrollbar_position(total, visible_height, output.scroll_offset);
    let mut state = ScrollbarState::new(total)
        .position(position)
        .viewport_content_length(visible_height);

    frame.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight),
        area,
        &mut state,
    );
}

fn output_scrollbar_position(total: usize, visible_height: usize, scroll_offset: usize) -> usize {
    let max_top_line = total.saturating_sub(visible_height);
    if max_top_line == 0 {
        return 0;
    }

    let offset = scroll_offset.min(max_top_line);
    let end = total.saturating_sub(offset);
    let top_line = end.saturating_sub(visible_height);

    top_line.saturating_mul(total.saturating_sub(1)) / max_top_line
}

fn footer_line(options: CommandOptions) -> Line<'static> {
    Line::from(vec![
        Span::styled("┃ ", footer_rail_style()),
        key_hint("b"),
        Span::raw(" "),
        fixed_span("Build", 7, Style::default().fg(Color::DarkGray)),
        Span::raw(" "),
        key_hint("d"),
        Span::raw(" "),
        fixed_span("Deploy", 7, Style::default().fg(Color::DarkGray)),
        Span::raw(" "),
        key_hint("c"),
        Span::raw(" "),
        fixed_span("Clean", 7, Style::default().fg(Color::DarkGray)),
        Span::raw(" "),
        key_hint("y"),
        Span::raw(" "),
        flag_span("yes", options.yes, false),
        Span::raw(" "),
        key_hint("r"),
        Span::raw(" "),
        flag_span("reset", options.reset, true),
        Span::raw("  Ctrl-P Palette  ? Help"),
    ])
}

fn command_status_line(run: &CommandRun) -> Line<'static> {
    Line::from(vec![
        Span::styled("┃ ", command_rail_style()),
        fixed_span(
            run.kind.title(),
            7,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        fixed_span(
            command_status_display(run),
            10,
            command_status_style(run.status),
        ),
        Span::raw(" | "),
        flag_span("yes", run.options.yes, false),
        Span::raw(" "),
        flag_span("reset", run.options.reset, true),
        Span::raw(" | "),
        fixed_span(command_display(&run.args), 30, Style::default()),
        Span::raw(" | "),
        if matches!(
            run.status,
            CommandStatus::Running | CommandStatus::Cancelling
        ) {
            fixed_span("Esc/Ctrl-C cancel", 17, Style::default().fg(Color::Yellow))
        } else {
            fixed_span("b/d/c run again", 17, Style::default().fg(Color::DarkGray))
        },
    ])
}

fn command_status_style(status: CommandStatus) -> Style {
    match status {
        CommandStatus::Running | CommandStatus::Cancelling => Style::default().fg(Color::Yellow),
        CommandStatus::Succeeded => Style::default().fg(Color::Green),
        CommandStatus::Failed | CommandStatus::Killed => Style::default().fg(Color::Red),
    }
    .add_modifier(Modifier::BOLD)
}

fn command_status_display(run: &CommandRun) -> String {
    if matches!(
        run.status,
        CommandStatus::Running | CommandStatus::Cancelling
    ) {
        format!(
            "{} {}",
            spinner_symbol(run.spinner_frame),
            run.status.title()
        )
    } else {
        run.status.title().to_string()
    }
}

fn spinner_symbol(frame: usize) -> &'static str {
    const FRAMES: [&str; 4] = ["-", "\\", "|", "/"];
    FRAMES[frame % FRAMES.len()]
}

fn flag_span(name: &'static str, enabled: bool, warn_when_enabled: bool) -> Span<'static> {
    let style = if enabled {
        Style::default()
            .fg(if warn_when_enabled {
                Color::Yellow
            } else {
                Color::Green
            })
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let width = match name {
        "yes" => 7,
        "reset" => 9,
        _ => name.len() + 4,
    };

    fixed_span(format!("{name}:{}", flag_state(enabled)), width, style)
}

fn key_hint(key: &'static str) -> Span<'static> {
    Span::styled(
        key,
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )
}

fn fixed_span(text: impl AsRef<str>, width: usize, style: Style) -> Span<'static> {
    Span::styled(pad_text(text.as_ref(), width), style)
}

fn pad_text(text: &str, width: usize) -> String {
    let mut truncated = text.chars().take(width).collect::<String>();
    let len = truncated.chars().count();
    if len < width {
        truncated.push_str(&" ".repeat(width - len));
    }
    truncated
}

fn flag_state(enabled: bool) -> &'static str {
    if enabled { "on" } else { "off" }
}

fn render_output_lines(buffer: &OutputBuffer, height: usize) -> Vec<Line<'static>> {
    buffer
        .visible_lines(height)
        .into_iter()
        .flat_map(output_bytes_to_lines)
        .map(output_line_with_rail)
        .collect()
}

fn output_line_with_rail(line: Line<'static>) -> Line<'static> {
    let mut spans = vec![Span::styled("┃ ", surface_rail_style())];
    spans.extend(line.spans);
    Line::from(spans)
}

fn output_bytes_to_lines(bytes: Vec<u8>) -> Vec<Line<'static>> {
    match bytes.clone().into_text() {
        Ok(text) if !text.lines.is_empty() => text.lines,
        _ => {
            let stripped = strip_ansi_escapes::strip(bytes);
            vec![Line::from(String::from_utf8_lossy(&stripped).to_string())]
        }
    }
}

fn render_palette(frame: &mut Frame<'_>, app: &TuiApp) {
    let area = centered_rect(70, 50, frame.area());
    let actions = filtered_actions(&app.palette.query);
    let selected = app.palette.selected.min(actions.len().saturating_sub(1));
    let mut lines = vec![
        Line::from(vec![Span::styled(
            "Command Palette",
            Style::default().add_modifier(Modifier::BOLD),
        )]),
        Line::from(format!("> {}", app.palette.query)),
        Line::default(),
    ];

    if actions.is_empty() {
        lines.push(Line::from("No matching commands"));
    } else {
        for (index, action) in actions.iter().take(8).enumerate() {
            let prefix = if index == selected { "> " } else { "  " };
            let shortcut = action
                .shortcut
                .map(|shortcut| format!(" ({shortcut})"))
                .unwrap_or_default();
            let label = format!("{prefix}{}{}", action.label, shortcut);
            let style = if index == selected {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            lines.push(Line::from(vec![
                Span::styled(format!("{label:<28}"), style),
                Span::raw(action.description),
            ]));
        }
    }

    let block = Block::default().borders(Borders::ALL).title(" Search ");
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let [_, horizontal, _] = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .areas(area);

    let [_, vertical, _] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .areas(horizontal);

    vertical
}

#[derive(Debug, Clone, Copy)]
struct TuiAction {
    label: &'static str,
    description: &'static str,
    shortcut: Option<&'static str>,
    kind: TuiActionKind,
}

#[derive(Debug, Clone, Copy)]
enum TuiActionKind {
    SelectView(TuiView),
    Build,
    Deploy,
    Clean,
    ToggleYes,
    ToggleReset,
    ShowHelp,
    Quit,
}

const ACTIONS: [TuiAction; 12] = [
    TuiAction {
        label: "Build",
        description: "Run golem build",
        shortcut: Some("b"),
        kind: TuiActionKind::Build,
    },
    TuiAction {
        label: "Deploy",
        description: "Run golem deploy",
        shortcut: Some("d"),
        kind: TuiActionKind::Deploy,
    },
    TuiAction {
        label: "Clean",
        description: "Run golem clean",
        shortcut: Some("c"),
        kind: TuiActionKind::Clean,
    },
    TuiAction {
        label: "Toggle Yes",
        description: "Toggle --yes for build/deploy",
        shortcut: Some("y"),
        kind: TuiActionKind::ToggleYes,
    },
    TuiAction {
        label: "Toggle Reset",
        description: "Toggle --reset for deploy",
        shortcut: Some("r"),
        kind: TuiActionKind::ToggleReset,
    },
    TuiAction {
        label: "Go to Dashboard",
        description: "Switch to Dashboard view",
        shortcut: Some("1"),
        kind: TuiActionKind::SelectView(TuiView::Dashboard),
    },
    TuiAction {
        label: "Go to Environments",
        description: "Switch to Environments view",
        shortcut: Some("2"),
        kind: TuiActionKind::SelectView(TuiView::Environments),
    },
    TuiAction {
        label: "Go to Components",
        description: "Switch to Components view",
        shortcut: Some("3"),
        kind: TuiActionKind::SelectView(TuiView::Components),
    },
    TuiAction {
        label: "Go to Agents",
        description: "Switch to Agents view",
        shortcut: Some("4"),
        kind: TuiActionKind::SelectView(TuiView::Agents),
    },
    TuiAction {
        label: "Go to Output",
        description: "Switch to Output view",
        shortcut: Some("5"),
        kind: TuiActionKind::SelectView(TuiView::Output),
    },
    TuiAction {
        label: "Show Help",
        description: "Show TUI shortcuts",
        shortcut: Some("?"),
        kind: TuiActionKind::ShowHelp,
    },
    TuiAction {
        label: "Quit",
        description: "Exit the TUI",
        shortcut: Some("q"),
        kind: TuiActionKind::Quit,
    },
];

fn filtered_actions(query: &str) -> Vec<TuiAction> {
    let query = query.trim();
    if query.is_empty() {
        return ACTIONS.to_vec();
    }

    let matcher = SkimMatcherV2::default();
    let mut matches = ACTIONS
        .iter()
        .filter_map(|action| {
            let haystack = format!("{} {}", action.label, action.description);
            matcher
                .fuzzy_match(&haystack, query)
                .map(|score| (score, *action))
        })
        .collect::<Vec<_>>();

    matches.sort_by(|(left, _), (right, _)| right.cmp(left));
    matches.into_iter().map(|(_, action)| action).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use test_r::test;

    #[test]
    fn renders_dashboard_frame() {
        let app = test_app();
        let frame = render_app_text(&app);

        assert!(frame.contains("Golem"), "{frame}");
        assert!(frame.contains("app:sample-app"), "{frame}");
        assert!(frame.contains("sample-app"), "{frame}");
        assert!(frame.contains("Build"), "{frame}");
        assert!(frame.contains("Deploy"), "{frame}");
        assert!(frame.contains("Clean"), "{frame}");
    }

    #[test]
    fn renders_active_tab() {
        let app = test_app();
        let frame = render_app_text(&app);

        assert!(frame.contains("Dashboard"), "{frame}");
        assert!(frame.contains("Environments"), "{frame}");
    }

    #[test]
    fn switches_tabs_with_keys() {
        let mut app = test_app();

        app.handle_key(key(KeyCode::Char(']')));
        let frame = render_app_text(&app);
        assert_eq!(app.active_view, TuiView::Environments);
        assert!(frame.contains("Environments"), "{frame}");

        app.handle_key(key(KeyCode::Char('[')));
        let frame = render_app_text(&app);
        assert_eq!(app.active_view, TuiView::Dashboard);
        assert!(frame.contains("Dashboard"), "{frame}");
    }

    #[test]
    fn jumps_to_tab_with_number() {
        let mut app = test_app();

        app.handle_key(key(KeyCode::Char('3')));
        let frame = render_app_text(&app);

        assert_eq!(app.active_view, TuiView::Components);
        assert!(frame.contains("Components"), "{frame}");
        assert!(
            frame.contains("Component monitoring will appear here."),
            "{frame}"
        );
    }

    #[test]
    fn opens_palette() {
        let mut app = test_app();

        app.handle_key(modified_key(KeyCode::Char('p'), KeyModifiers::CONTROL));
        let frame = render_app_text(&app);

        assert!(frame.contains("Command Palette"), "{frame}");
        assert!(frame.contains("Go to Dashboard"), "{frame}");
    }

    #[test]
    fn filters_palette() {
        let mut app = test_app();

        app.handle_key(modified_key(KeyCode::Char('p'), KeyModifiers::CONTROL));
        for character in "comp".chars() {
            app.handle_key(key(KeyCode::Char(character)));
        }
        let frame = render_app_text(&app);

        assert!(frame.contains("> comp"), "{frame}");
        assert!(frame.contains("Go to Components"), "{frame}");
    }

    #[test]
    fn executes_palette_action() {
        let mut app = test_app();

        app.handle_key(modified_key(KeyCode::Char('p'), KeyModifiers::CONTROL));
        for character in "comp".chars() {
            app.handle_key(key(KeyCode::Char(character)));
        }
        app.handle_key(key(KeyCode::Enter));

        let frame = render_app_text(&app);
        assert_eq!(app.active_view, TuiView::Components);
        assert!(frame.contains("Components"), "{frame}");
        assert!(!frame.contains("Command Palette"), "{frame}");
    }

    #[test]
    fn opens_help() {
        let mut app = test_app();

        app.handle_key(key(KeyCode::Char('?')));
        let frame = render_app_text(&app);

        assert!(frame.contains("Keyboard Shortcuts"), "{frame}");
        assert!(frame.contains("Ctrl-P / :"), "{frame}");
    }

    #[test]
    fn closes_help_with_escape() {
        let mut app = test_app();

        app.handle_key(key(KeyCode::Char('?')));
        app.handle_key(key(KeyCode::Esc));
        let frame = render_app_text(&app);

        assert!(!frame.contains("Keyboard Shortcuts"), "{frame}");
        assert!(frame.contains("Dashboard"), "{frame}");
    }

    #[test]
    fn opens_help_from_palette_action() {
        let mut app = test_app();

        app.handle_key(modified_key(KeyCode::Char('p'), KeyModifiers::CONTROL));
        for character in "help".chars() {
            app.handle_key(key(KeyCode::Char(character)));
        }
        app.handle_key(key(KeyCode::Enter));
        let frame = render_app_text(&app);

        assert!(frame.contains("Keyboard Shortcuts"), "{frame}");
        assert!(!frame.contains("Command Palette"), "{frame}");
    }

    #[test]
    fn toggles_command_options() {
        let mut app = test_app();

        app.handle_key(key(KeyCode::Char('y')));
        app.handle_key(key(KeyCode::Char('r')));

        assert!(app.command_options.yes);
        assert!(app.command_options.reset);
        assert_eq!(
            app.command_args(CommandKind::Deploy),
            vec!["deploy", "--yes", "--reset"]
        );
        assert_eq!(app.command_args(CommandKind::Clean), vec!["clean", "--yes"]);
    }

    #[test]
    fn renders_flags_in_footer() {
        let mut app = test_app();

        let frame = render_app_text(&app);
        assert!(frame.contains("yes:off"), "{frame}");
        assert!(frame.contains("reset:off"), "{frame}");

        app.handle_key(key(KeyCode::Char('y')));
        app.handle_key(key(KeyCode::Char('r')));
        let frame = render_app_text(&app);
        assert!(frame.contains("yes:on"), "{frame}");
        assert!(frame.contains("reset:on"), "{frame}");
    }

    #[test]
    fn build_key_switches_to_output_and_records_command() {
        let mut app = test_app();

        app.handle_key(key(KeyCode::Char('b')));

        let run = app.command_run.as_ref().expect("missing command run");
        assert_eq!(app.active_view, TuiView::Output);
        assert_eq!(app.mode, TuiMode::CommandInteraction);
        assert_eq!(run.kind, CommandKind::Build);
        assert_eq!(run.args, vec!["build"]);
    }

    #[test]
    fn clean_key_switches_to_output_and_records_command() {
        let mut app = test_app();

        app.handle_key(key(KeyCode::Char('c')));

        let run = app.command_run.as_ref().expect("missing command run");
        assert_eq!(app.active_view, TuiView::Output);
        assert_eq!(app.mode, TuiMode::CommandInteraction);
        assert_eq!(run.kind, CommandKind::Clean);
        assert_eq!(run.args, vec!["clean"]);
    }

    #[test]
    fn clean_palette_action_starts_clean() {
        let mut app = test_app();

        app.handle_key(modified_key(KeyCode::Char('p'), KeyModifiers::CONTROL));
        for character in "clean".chars() {
            app.handle_key(key(KeyCode::Char(character)));
        }
        app.handle_key(key(KeyCode::Enter));

        let run = app.command_run.as_ref().expect("missing command run");
        assert_eq!(app.active_view, TuiView::Output);
        assert_eq!(run.kind, CommandKind::Clean);
    }

    #[test]
    fn renders_compact_command_status() {
        let mut app = test_app();

        app.handle_key(key(KeyCode::Char('b')));
        let frame = render_app_text(&app);

        assert!(
            frame.contains("build")
                && frame.contains("running")
                && frame.contains("yes:off")
                && frame.contains("golem build"),
            "{frame}"
        );
    }

    #[test]
    fn fixed_width_labels_do_not_reflow() {
        assert_eq!(pad_text("yes:on", 7).len(), pad_text("yes:off", 7).len());
        assert_eq!(
            pad_text("reset:on", 9).len(),
            pad_text("reset:off", 9).len()
        );
        assert_eq!(
            pad_text("running", 10).len(),
            pad_text("succeeded", 10).len()
        );
    }

    #[test]
    fn output_input_row_is_hidden_when_idle() {
        let mut app = test_app();
        app.active_view = TuiView::Output;

        let frame = render_app_text(&app);
        assert!(!frame.contains("stdin"), "{frame}");
    }

    #[test]
    fn output_input_row_visibility_uses_current_run_yes_flag() {
        let mut app = test_app();

        app.handle_key(key(KeyCode::Char('b')));
        let frame = render_app_text(&app);
        assert!(frame.contains("stdin"), "{frame}");

        app.handle_key(key(KeyCode::Char('y')));
        let frame = render_app_text(&app);
        assert!(frame.contains("stdin"), "{frame}");
    }

    #[test]
    fn output_input_row_is_hidden_for_yes_command() {
        let mut app = test_app();

        app.handle_key(key(KeyCode::Char('y')));
        app.handle_key(key(KeyCode::Char('b')));
        let frame = render_app_text(&app);

        assert!(!frame.contains("stdin"), "{frame}");
    }

    #[test]
    fn sets_cursor_in_command_interaction() {
        let mut app = test_app();

        app.handle_key(key(KeyCode::Char('b')));
        let (_, cursor) = render_app_text_and_cursor(&app);

        assert!(cursor.x > 0, "cursor should be visible after input prompt");
        assert!(cursor.y > 0, "cursor should be visible after input prompt");
    }

    #[test]
    fn output_buffer_autofollows_and_scrolls() {
        let mut output = OutputBuffer::default();

        for index in 0..20 {
            output.append(format!("line {index}\n").as_bytes());
        }
        assert!(output.follow);
        assert_eq!(output.scroll_offset, 0);
        assert!(String::from_utf8_lossy(&output.visible_lines(3).concat()).contains("line 19"));

        output.scroll_up(5);
        assert!(!output.follow);
        assert_eq!(output.scroll_offset, 5);

        output.scroll_bottom();
        assert!(output.follow);
        assert_eq!(output.scroll_offset, 0);
    }

    #[test]
    fn output_buffer_top_scroll_fills_viewport() {
        let mut output = OutputBuffer::default();

        for index in 0..20 {
            output.append(format!("line {index}\n").as_bytes());
        }
        output.scroll_top();

        let lines = output.visible_lines(5);
        let text = String::from_utf8_lossy(&lines.concat()).to_string();
        assert_eq!(lines.len(), 5);
        assert!(text.contains("line 0"), "{text}");
        assert!(text.contains("line 4"), "{text}");
    }

    #[test]
    fn output_scrollbar_position_reaches_top_and_bottom() {
        assert_eq!(output_scrollbar_position(20, 5, 15), 0);
        assert_eq!(output_scrollbar_position(20, 5, 0), 19);
    }

    #[test]
    fn normal_output_arrow_keys_and_mouse_scroll_output() {
        let mut app = test_app();
        app.active_view = TuiView::Output;
        app.command_run = Some(CommandRun::new(
            1,
            CommandKind::Build,
            vec!["build".to_string()],
            CommandOptions::default(),
        ));
        app.set_command_status(CommandStatus::Succeeded);
        for index in 0..20 {
            app.append_command_output(format!("line {index}\n").as_bytes());
        }

        app.handle_key(key(KeyCode::Up));
        assert_eq!(
            app.command_run.as_ref().map(|run| run.output.scroll_offset),
            Some(1)
        );

        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 10,
            row: 10,
            modifiers: KeyModifiers::empty(),
        });
        assert_eq!(
            app.command_run.as_ref().map(|run| run.output.scroll_offset),
            Some(4)
        );

        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 10,
            row: 10,
            modifiers: KeyModifiers::empty(),
        });
        assert_eq!(
            app.command_run.as_ref().map(|run| run.output.scroll_offset),
            Some(1)
        );
    }

    #[test]
    fn esc_cancels_then_force_kills_running_command() {
        let mut app = test_app();

        app.handle_key(key(KeyCode::Char('b')));
        app.handle_key(key(KeyCode::Esc));
        assert_eq!(
            app.command_run.as_ref().map(|run| run.status),
            Some(CommandStatus::Cancelling)
        );

        app.handle_key(key(KeyCode::Esc));
        assert_eq!(
            app.command_run.as_ref().map(|run| run.status),
            Some(CommandStatus::Killed)
        );
        assert_eq!(app.mode, TuiMode::Normal);
    }

    #[test]
    fn spinner_tick_advances_running_command() {
        let mut app = test_app();
        let (tx, _rx) = mpsc::channel();

        app.handle_key(key(KeyCode::Char('b')));
        assert_eq!(
            app.command_run.as_ref().map(|run| run.spinner_frame),
            Some(0)
        );

        app.handle_event(TuiEvent::SpinnerTick(1), &tx);
        assert_eq!(
            app.command_run.as_ref().map(|run| run.spinner_frame),
            Some(1)
        );
        let frame = render_app_text(&app);
        assert!(
            frame.contains("\\ running")
                || frame.contains("- running")
                || frame.contains("| running")
                || frame.contains("/ running"),
            "{frame}"
        );
    }

    fn test_app() -> TuiApp {
        TuiApp {
            should_quit: false,
            active_view: TuiView::Dashboard,
            mode: TuiMode::Normal,
            palette: CommandPalette::default(),
            command_options: CommandOptions::default(),
            command_run: None,
            next_command_id: 1,
            context: TuiContextInfo {
                application: "sample-app".to_string(),
                environment: "local".to_string(),
                server: "local".to_string(),
                config_dir: "/tmp/golem-config".to_string(),
            },
        }
    }

    fn render_app_text(app: &TuiApp) -> String {
        render_app_text_and_cursor(app).0
    }

    fn render_app_text_and_cursor(app: &TuiApp) -> (String, ratatui::layout::Position) {
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| render(frame, app)).unwrap();

        (
            render_buffer_text(terminal.backend().buffer()),
            terminal.backend().cursor_position(),
        )
    }

    fn key(code: KeyCode) -> KeyEvent {
        modified_key(code, KeyModifiers::empty())
    }

    fn modified_key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn render_buffer_text(buffer: &Buffer) -> String {
        let area = buffer.area;
        let mut output = String::new();

        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                if let Some(cell) = buffer.cell((x, y)) {
                    output.push_str(cell.symbol());
                }
            }
            output.push('\n');
        }

        output
    }
}
