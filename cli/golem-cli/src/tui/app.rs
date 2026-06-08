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
use crate::tui::nested_cli::{
    CommandExit, NestedCliRuntime, NestedCliSpec, NestedCliTarget, spawn_nested_cli,
};
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
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};
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

fn spawn_server_spinner(server_id: u64, event_tx: Sender<TuiEvent>, stop: Arc<AtomicBool>) {
    thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(120));
            if stop.load(Ordering::Relaxed) {
                return;
            }
            if event_tx
                .send(TuiEvent::ServerSpinnerTick(server_id))
                .is_err()
            {
                return;
            }
        }
    });
}

fn spawn_agent_auto_refresh(event_tx: Sender<TuiEvent>, stop: Arc<AtomicBool>) {
    thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_secs(5));
            if stop.load(Ordering::Relaxed) {
                return;
            }
            if event_tx.send(TuiEvent::AgentRefreshTick).is_err() {
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
    server: ServerState,
    agents: AgentsState,
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
            server: ServerState::default(),
            agents: AgentsState::default(),
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
            TuiEvent::ServerOutput(bytes) => self.server.run.output.append(&bytes),
            TuiEvent::ServerOutputClosed(error) => {
                if let Some(error) = error {
                    self.server
                        .run
                        .output
                        .append_local_line(format!("server output closed: {error}"));
                }
            }
            TuiEvent::ServerExited(exit) => self.finish_server(exit, event_tx),
            TuiEvent::ServerSpinnerTick(server_id) => self.handle_server_spinner_tick(server_id),
            TuiEvent::AgentRefreshTick => self.refresh_agents(Some(event_tx)),
            TuiEvent::AgentRefreshFinished { generation, result } => {
                self.finish_agent_refresh(generation, result)
            }
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
            TuiMode::AgentFilter => self.handle_agent_filter_key(key, event_tx),
            TuiMode::CommandInteraction => self.handle_command_interaction_key(key),
        }
    }

    fn handle_global_key(&mut self, key: KeyEvent, event_tx: Option<&Sender<TuiEvent>>) {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.active_view == TuiView::Server && self.server.run.is_running() {
                    self.stop_server();
                } else {
                    self.should_quit = true;
                }
            }
            KeyCode::Char('b') => self.start_command(CommandKind::Build, event_tx),
            KeyCode::Char('d') => self.start_command(CommandKind::Deploy, event_tx),
            KeyCode::Char('c') => self.start_command(CommandKind::Clean, event_tx),
            KeyCode::Char('y') => self.command_options.yes = !self.command_options.yes,
            KeyCode::Char('r') => self.command_options.reset = !self.command_options.reset,
            KeyCode::Char('u') if self.active_view == TuiView::Agents => {
                self.refresh_agents(event_tx)
            }
            KeyCode::Char('a') if self.active_view == TuiView::Agents => {
                self.toggle_agent_auto_refresh(event_tx)
            }
            KeyCode::Char('m') if self.active_view == TuiView::Agents => {
                self.cycle_agent_mode(event_tx)
            }
            KeyCode::Char('i') if self.active_view == TuiView::Agents => {
                self.agents.detail_visible = !self.agents.detail_visible
            }
            KeyCode::Char('/') if self.active_view == TuiView::Agents => {
                self.mode = TuiMode::AgentFilter
            }
            KeyCode::Char('s') if self.active_view == TuiView::Server => {
                self.toggle_server(event_tx)
            }
            KeyCode::Char('R') if self.active_view == TuiView::Server => {
                self.restart_server(ServerStartMode::Current, event_tx)
            }
            KeyCode::Char('x') if self.active_view == TuiView::Server => {
                self.server.clean = !self.server.clean
            }
            KeyCode::Char('C') if self.active_view == TuiView::Server => {
                self.restart_server(ServerStartMode::Clean, event_tx)
            }
            KeyCode::PageUp if self.active_view == TuiView::Server => self.scroll_server_up_by(10),
            KeyCode::PageDown if self.active_view == TuiView::Server => {
                self.scroll_server_down_by(10)
            }
            KeyCode::Home if self.active_view == TuiView::Server => {
                self.server.run.output.scroll_top()
            }
            KeyCode::End if self.active_view == TuiView::Server => {
                self.server.run.output.scroll_bottom()
            }
            KeyCode::PageUp => self.scroll_output_up(),
            KeyCode::PageDown => self.scroll_output_down(),
            KeyCode::Home => self.scroll_output_top(),
            KeyCode::End => self.scroll_output_bottom(),
            KeyCode::Up if self.active_view == TuiView::Output => self.scroll_output_up_by(1),
            KeyCode::Down if self.active_view == TuiView::Output => self.scroll_output_down_by(1),
            KeyCode::Up if self.active_view == TuiView::Server => self.scroll_server_up_by(1),
            KeyCode::Down if self.active_view == TuiView::Server => self.scroll_server_down_by(1),
            KeyCode::Up if self.active_view == TuiView::Agents => self.select_previous_agent(),
            KeyCode::Down if self.active_view == TuiView::Agents => self.select_next_agent(),
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.open_palette();
            }
            KeyCode::Char(':') => self.open_palette(),
            KeyCode::Char('?') => self.mode = TuiMode::Help,
            KeyCode::Char(']') | KeyCode::Tab => self.next_view(),
            KeyCode::Char('[') | KeyCode::BackTab => self.previous_view(),
            KeyCode::Char('1') => self.active_view = TuiView::Dashboard,
            KeyCode::Char('2') => self.open_agents_view(event_tx),
            KeyCode::Char('3') => self.active_view = TuiView::Output,
            KeyCode::Char('4') => self.active_view = TuiView::Server,
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

    fn handle_agent_filter_key(&mut self, key: KeyEvent, event_tx: Option<&Sender<TuiEvent>>) {
        match key.code {
            KeyCode::Esc | KeyCode::Enter => self.mode = TuiMode::Normal,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.mode = TuiMode::Normal;
            }
            KeyCode::Backspace => {
                self.agents.query.pop();
                self.agents.selected = 0;
            }
            KeyCode::Up => self.select_previous_agent(),
            KeyCode::Down => self.select_next_agent(),
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.agents.query.clear();
                self.agents.selected = 0;
            }
            KeyCode::Char(character)
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                self.agents.query.push(character);
                self.agents.selected = 0;
            }
            KeyCode::Char('m') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cycle_agent_mode(event_tx);
            }
            _ => {}
        }
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) {
        match (self.active_view, mouse.kind) {
            (TuiView::Output, MouseEventKind::ScrollUp) => self.scroll_output_up_by(3),
            (TuiView::Output, MouseEventKind::ScrollDown) => self.scroll_output_down_by(3),
            (TuiView::Server, MouseEventKind::ScrollUp) => self.scroll_server_up_by(3),
            (TuiView::Server, MouseEventKind::ScrollDown) => self.scroll_server_down_by(3),
            _ => {}
        }
    }

    fn execute_action(&mut self, action: TuiActionKind, event_tx: Option<&Sender<TuiEvent>>) {
        match action {
            TuiActionKind::SelectView(view) => {
                self.close_palette();
                if view == TuiView::Agents {
                    self.open_agents_view(event_tx);
                } else {
                    self.active_view = view;
                }
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
            TuiActionKind::StartServer => {
                self.close_palette();
                self.start_server(ServerStartMode::Current, event_tx);
            }
            TuiActionKind::StopServer => {
                self.close_palette();
                self.stop_server();
            }
            TuiActionKind::RestartServer => {
                self.close_palette();
                self.restart_server(ServerStartMode::Current, event_tx);
            }
            TuiActionKind::CleanRestartServer => {
                self.close_palette();
                self.restart_server(ServerStartMode::Clean, event_tx);
            }
            TuiActionKind::ToggleServerClean => {
                self.server.clean = !self.server.clean;
                self.close_palette();
            }
            TuiActionKind::RefreshAgents => {
                self.close_palette();
                self.refresh_agents(event_tx);
            }
            TuiActionKind::ToggleAgentAutoRefresh => {
                self.toggle_agent_auto_refresh(event_tx);
                self.close_palette();
            }
            TuiActionKind::ToggleAgentDetails => {
                self.agents.detail_visible = !self.agents.detail_visible;
                self.close_palette();
            }
            TuiActionKind::CycleAgentMode => {
                self.cycle_agent_mode(event_tx);
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

    fn open_agents_view(&mut self, event_tx: Option<&Sender<TuiEvent>>) {
        self.active_view = TuiView::Agents;
        if self.agents.agents.is_empty() && !self.agents.refresh_running {
            self.refresh_agents(event_tx);
        }
    }

    fn select_next_agent(&mut self) {
        let count = self.filtered_agents().len();
        if count > 0 {
            self.agents.selected = (self.agents.selected + 1).min(count - 1);
        }
    }

    fn select_previous_agent(&mut self) {
        self.agents.selected = self.agents.selected.saturating_sub(1);
    }

    fn filtered_agents(&self) -> Vec<&AgentListItem> {
        self.agents.filtered_agents()
    }

    fn refresh_agents(&mut self, event_tx: Option<&Sender<TuiEvent>>) {
        if self.agents.refresh_running {
            return;
        }

        self.agents.refresh_generation += 1;
        let generation = self.agents.refresh_generation;
        self.agents.refresh_running = true;
        self.agents.last_error = None;

        let Some(event_tx) = event_tx.cloned() else {
            return;
        };
        let mode = self.agents.mode;
        thread::spawn(move || {
            let result = run_agent_refresh(mode).map_err(|error| format!("{error:#}"));
            let _ = event_tx.send(TuiEvent::AgentRefreshFinished { generation, result });
        });
    }

    fn finish_agent_refresh(&mut self, generation: u64, result: Result<String, String>) {
        if generation != self.agents.refresh_generation {
            return;
        }

        self.agents.refresh_running = false;
        match result {
            Ok(output) => match parse_agent_list_output(&output) {
                Ok(agents) => {
                    self.agents.agents = agents;
                    self.agents.last_error = None;
                    self.agents.clamp_selection();
                }
                Err(error) => {
                    self.agents.last_error = Some(format!("{error:#}"));
                }
            },
            Err(error) => {
                self.agents.last_error = Some(error);
            }
        }
    }

    fn toggle_agent_auto_refresh(&mut self, event_tx: Option<&Sender<TuiEvent>>) {
        self.agents.auto_refresh = !self.agents.auto_refresh;
        if self.agents.auto_refresh {
            if self.agents.auto_refresh_stop.is_none() {
                if let Some(event_tx) = event_tx.cloned() {
                    let stop = Arc::new(AtomicBool::new(false));
                    spawn_agent_auto_refresh(event_tx, stop.clone());
                    self.agents.auto_refresh_stop = Some(stop);
                }
            }
            self.refresh_agents(event_tx);
        } else if let Some(stop) = self.agents.auto_refresh_stop.take() {
            stop.store(true, Ordering::Relaxed);
        }
    }

    fn cycle_agent_mode(&mut self, event_tx: Option<&Sender<TuiEvent>>) {
        self.agents.mode = self.agents.mode.next();
        self.agents.selected = 0;
        self.refresh_agents(event_tx);
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

        match spawn_nested_cli(spec, event_tx.clone(), NestedCliTarget::Command) {
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
        self.cleanup_server();
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

    fn toggle_server(&mut self, event_tx: Option<&Sender<TuiEvent>>) {
        if self.server.run.is_running() {
            self.stop_server();
        } else {
            self.start_server(ServerStartMode::Current, event_tx);
        }
    }

    fn start_server(&mut self, mode: ServerStartMode, event_tx: Option<&Sender<TuiEvent>>) {
        if self.server.run.is_running() {
            self.server
                .run
                .output
                .append_local_line("server already running");
            self.active_view = TuiView::Server;
            return;
        }

        let clean = match mode {
            ServerStartMode::Current => self.server.clean,
            ServerStartMode::Clean => true,
        };
        let args = server_args(clean);
        self.active_view = TuiView::Server;
        self.server.run = ServerRun::new(self.server.next_id, clean, args.clone());
        self.server.next_id += 1;
        self.server
            .run
            .output
            .append_local_line(format!("$ {}", command_display(&args)));

        let Some(event_tx) = event_tx else {
            return;
        };

        let spec = match self.server_spec(args) {
            Ok(spec) => spec,
            Err(error) => {
                self.server
                    .run
                    .output
                    .append_local_line(format!("failed to prepare server: {error:#}"));
                self.server.run.status = ServerStatus::Failed;
                return;
            }
        };

        match spawn_nested_cli(spec, event_tx.clone(), NestedCliTarget::Server) {
            Ok(runtime) => {
                self.server.run.runtime = Some(runtime);
                let stop = Arc::new(AtomicBool::new(false));
                spawn_server_spinner(self.server.run.id, event_tx.clone(), stop.clone());
                self.server.run.spinner_stop = Some(stop);
            }
            Err(error) => {
                self.server
                    .run
                    .output
                    .append_local_line(format!("failed to start server: {error:#}"));
                self.server.run.status = ServerStatus::Failed;
            }
        }
    }

    fn server_spec(&self, args: Vec<String>) -> anyhow::Result<NestedCliSpec> {
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

    fn stop_server(&mut self) {
        match self.server.run.status {
            ServerStatus::Starting | ServerStatus::Running => {
                if let Some(runtime) = self.server.run.runtime.as_mut() {
                    if let Err(error) = runtime.send_ctrl_c() {
                        self.server
                            .run
                            .output
                            .append_local_line(format!("failed to stop server: {error:#}"));
                    }
                }
                self.server.run.status = ServerStatus::Stopping;
                self.server
                    .run
                    .output
                    .append_local_line("server stop requested");
            }
            ServerStatus::Stopping => {
                if let Some(runtime) = self.server.run.runtime.as_mut()
                    && let Err(error) = runtime.kill()
                {
                    self.server
                        .run
                        .output
                        .append_local_line(format!("failed to force kill server: {error:#}"));
                }
                self.server.run.runtime = None;
                self.server.run.status = ServerStatus::Stopped;
                self.server.run.stop_spinner();
                self.server
                    .run
                    .output
                    .append_local_line("server force killed");
            }
            ServerStatus::Stopped | ServerStatus::Failed => {}
        }
    }

    fn restart_server(&mut self, mode: ServerStartMode, event_tx: Option<&Sender<TuiEvent>>) {
        if self.server.run.is_running() {
            self.server.run.restart_after_stop = Some(mode);
            self.stop_server();
        } else {
            self.start_server(mode, event_tx);
        }
    }

    fn finish_server(&mut self, exit: CommandExit, event_tx: &Sender<TuiEvent>) {
        let pending_restart = self.server.run.restart_after_stop.take();
        self.server.run.runtime = None;
        self.server.run.stop_spinner();
        self.server.run.exit_code = exit.code;
        self.server.run.status = if pending_restart.is_some() {
            ServerStatus::Stopped
        } else if exit.success || matches!(self.server.run.status, ServerStatus::Stopping) {
            ServerStatus::Stopped
        } else {
            ServerStatus::Failed
        };
        self.server
            .run
            .output
            .append_local_line(match self.server.run.status {
                ServerStatus::Stopped => "server stopped".to_string(),
                ServerStatus::Failed => {
                    format!("server exited with code {}", exit.code.unwrap_or(1))
                }
                ServerStatus::Starting | ServerStatus::Running | ServerStatus::Stopping => {
                    "server exited".to_string()
                }
            });

        if let Some(mode) = pending_restart {
            self.start_server(mode, Some(event_tx));
        }
    }

    fn handle_server_spinner_tick(&mut self, server_id: u64) {
        if self.server.run.id == server_id && self.server.run.is_running() {
            self.server.run.spinner_frame = self.server.run.spinner_frame.wrapping_add(1);
            if self.server.run.status == ServerStatus::Starting {
                self.server.run.status = ServerStatus::Running;
            }
        }
    }

    fn cleanup_server(&mut self) {
        if let Some(runtime) = self.server.run.runtime.as_mut() {
            let _ = runtime.kill();
            self.server.run.stop_spinner();
        }
    }

    fn scroll_server_up_by(&mut self, amount: usize) {
        self.server.run.output.scroll_up(amount);
    }

    fn scroll_server_down_by(&mut self, amount: usize) {
        self.server.run.output.scroll_down(amount);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TuiMode {
    Normal,
    Palette,
    Help,
    AgentFilter,
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

struct ServerState {
    clean: bool,
    next_id: u64,
    run: ServerRun,
}

struct AgentsState {
    mode: AgentModeFilter,
    query: String,
    selected: usize,
    detail_visible: bool,
    auto_refresh: bool,
    refresh_running: bool,
    refresh_generation: u64,
    auto_refresh_stop: Option<Arc<AtomicBool>>,
    last_error: Option<String>,
    agents: Vec<AgentListItem>,
}

impl Default for AgentsState {
    fn default() -> Self {
        Self {
            mode: AgentModeFilter::Durable,
            query: String::new(),
            selected: 0,
            detail_visible: true,
            auto_refresh: false,
            refresh_running: false,
            refresh_generation: 0,
            auto_refresh_stop: None,
            last_error: None,
            agents: Vec::new(),
        }
    }
}

impl AgentsState {
    fn filtered_agents(&self) -> Vec<&AgentListItem> {
        let query = self.query.trim();
        if query.is_empty() {
            return self.agents.iter().collect();
        }

        let matcher = SkimMatcherV2::default();
        let mut matches = self
            .agents
            .iter()
            .filter_map(|agent| {
                matcher
                    .fuzzy_match(&agent.search_text(), query)
                    .map(|score| (score, agent))
            })
            .collect::<Vec<_>>();
        matches.sort_by(|(left, _), (right, _)| right.cmp(left));
        matches.into_iter().map(|(_, agent)| agent).collect()
    }

    fn clamp_selection(&mut self) {
        let count = self.filtered_agents().len();
        if count == 0 {
            self.selected = 0;
        } else {
            self.selected = self.selected.min(count - 1);
        }
    }

    fn selected_agent<'a>(&self, filtered: &'a [&'a AgentListItem]) -> Option<&'a AgentListItem> {
        filtered.get(self.selected).copied()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentModeFilter {
    Durable,
    Ephemeral,
    All,
}

impl AgentModeFilter {
    fn next(self) -> Self {
        match self {
            Self::Durable => Self::Ephemeral,
            Self::Ephemeral => Self::All,
            Self::All => Self::Durable,
        }
    }

    fn as_cli_value(self) -> &'static str {
        match self {
            Self::Durable => "durable",
            Self::Ephemeral => "ephemeral",
            Self::All => "all",
        }
    }

    fn label(self) -> &'static str {
        self.as_cli_value()
    }
}

#[derive(Clone, Debug)]
struct AgentListItem {
    name: String,
    component: Option<String>,
    agent_type: Option<String>,
    status: Option<String>,
    raw: Value,
}

impl AgentListItem {
    fn search_text(&self) -> String {
        [
            Some(self.name.as_str()),
            self.component.as_deref(),
            self.agent_type.as_deref(),
            self.status.as_deref(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ")
    }
}

impl Default for ServerState {
    fn default() -> Self {
        Self {
            clean: false,
            next_id: 1,
            run: ServerRun::default(),
        }
    }
}

struct ServerRun {
    id: u64,
    status: ServerStatus,
    clean: bool,
    args: Vec<String>,
    output: OutputBuffer,
    runtime: Option<NestedCliRuntime>,
    spinner_frame: usize,
    spinner_stop: Option<Arc<AtomicBool>>,
    restart_after_stop: Option<ServerStartMode>,
    exit_code: Option<i32>,
}

impl Default for ServerRun {
    fn default() -> Self {
        Self {
            id: 0,
            status: ServerStatus::Stopped,
            clean: false,
            args: server_args(false),
            output: OutputBuffer::default(),
            runtime: None,
            spinner_frame: 0,
            spinner_stop: None,
            restart_after_stop: None,
            exit_code: None,
        }
    }
}

impl ServerRun {
    fn new(id: u64, clean: bool, args: Vec<String>) -> Self {
        Self {
            id,
            status: ServerStatus::Starting,
            clean,
            args,
            output: OutputBuffer::default(),
            runtime: None,
            spinner_frame: 0,
            spinner_stop: None,
            restart_after_stop: None,
            exit_code: None,
        }
    }

    fn is_running(&self) -> bool {
        matches!(
            self.status,
            ServerStatus::Starting | ServerStatus::Running | ServerStatus::Stopping
        )
    }

    fn stop_spinner(&mut self) {
        if let Some(stop) = self.spinner_stop.take() {
            stop.store(true, Ordering::Relaxed);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServerStatus {
    Stopped,
    Starting,
    Running,
    Stopping,
    Failed,
}

impl ServerStatus {
    fn title(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Stopping => "stopping",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServerStartMode {
    Current,
    Clean,
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

fn server_args(clean: bool) -> Vec<String> {
    let mut args = vec!["server".to_string(), "run".to_string()];
    if clean {
        args.push("--clean".to_string());
    }
    args
}

fn run_agent_refresh(mode: AgentModeFilter) -> anyhow::Result<String> {
    let output = Command::new(crate::binary_path_to_string()?)
        .args([
            "agent",
            "list",
            "--format",
            "json",
            "--mode",
            mode.as_cli_value(),
        ])
        .current_dir(crate::fs::current_dir_lexical()?)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let message = if stderr.trim().is_empty() {
            stdout
        } else {
            stderr
        };
        anyhow::bail!(message.trim().to_string())
    }
}

fn parse_agent_list_output(output: &str) -> anyhow::Result<Vec<AgentListItem>> {
    let value: Value = serde_json::from_str(output.trim())?;
    let values = match &value {
        Value::Array(values) => values.as_slice(),
        Value::Object(object) => object
            .get("values")
            .or_else(|| object.get("agents"))
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]),
        _ => &[],
    };

    Ok(values.iter().map(agent_item_from_value).collect())
}

fn agent_item_from_value(value: &Value) -> AgentListItem {
    let name = first_string_value(
        value,
        &[
            &["name"],
            &["agentName"],
            &["agentId"],
            &["id"],
            &["agent", "name"],
            &["agent", "id"],
        ],
    )
    .unwrap_or_else(|| "<unknown>".to_string());
    let component = first_string_value(
        value,
        &[
            &["component"],
            &["componentName"],
            &["componentId"],
            &["agent", "component"],
        ],
    );
    let agent_type = first_string_value(
        value,
        &[
            &["agentType"],
            &["agentTypeName"],
            &["type"],
            &["typeName"],
            &["agent", "type"],
        ],
    );
    let status = first_string_value(value, &[&["status"], &["state"], &["agent", "status"]]);

    AgentListItem {
        name,
        component,
        agent_type,
        status,
        raw: value.clone(),
    }
}

fn first_string_value(value: &Value, paths: &[&[&str]]) -> Option<String> {
    paths.iter().find_map(|path| {
        let mut current = value;
        for segment in *path {
            current = current.get(*segment)?;
        }
        match current {
            Value::String(value) => Some(value.clone()),
            Value::Number(value) => Some(value.to_string()),
            Value::Bool(value) => Some(value.to_string()),
            _ => None,
        }
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TuiView {
    Dashboard,
    Agents,
    Output,
    Server,
}

impl TuiView {
    const ALL: [Self; 4] = [Self::Dashboard, Self::Agents, Self::Output, Self::Server];

    fn title(self) -> &'static str {
        match self {
            Self::Dashboard => "Dashboard",
            Self::Agents => "Agents",
            Self::Output => "Output",
            Self::Server => "Server",
        }
    }

    fn placeholder(self) -> &'static str {
        match self {
            Self::Dashboard => "Selected context and quick actions.",
            Self::Agents => "Agent monitoring and management will appear here.",
            Self::Output => "Nested command output will appear here.",
            Self::Server => "Local server logs will appear here.",
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

    if app.active_view == TuiView::Agents {
        render_agents_view(frame, body, app);
    } else if app.active_view == TuiView::Output {
        render_output_view(frame, body, app);
    } else if app.active_view == TuiView::Server {
        render_server_view(frame, body, app);
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
        TuiMode::AgentFilter => {}
        TuiMode::CommandInteraction => {}
        TuiMode::Palette => render_palette(frame, app),
        TuiMode::Help => render_help(frame),
    }
}

fn render_server_view(frame: &mut Frame<'_>, area: Rect, app: &TuiApp) {
    render_surface(frame, area);
    let [summary_area, output_area] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .areas(area);

    frame.render_widget(
        Paragraph::new(server_status_line(&app.server)).style(command_status_bg_style()),
        summary_area,
    );
    render_left_rail(frame, summary_area, command_rail_style());

    let output_lines = if app.server.run.output.total_lines() == 0 {
        vec![prefixed_line(
            "Server logs will appear here. Press s to start.",
        )]
    } else {
        render_output_lines(&app.server.run.output, output_area.height as usize)
    };
    frame.render_widget(
        Paragraph::new(output_lines)
            .style(surface_style())
            .wrap(Wrap { trim: false }),
        output_area,
    );
    render_left_rail(frame, output_area, surface_rail_style());
    render_output_scrollbar(frame, output_area, &app.server.run.output);
}

fn render_agents_view(frame: &mut Frame<'_>, area: Rect, app: &TuiApp) {
    render_surface(frame, area);
    let [summary_area, content_area] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .areas(area);

    frame.render_widget(
        Paragraph::new(agent_status_line(&app.agents)).style(command_status_bg_style()),
        summary_area,
    );
    render_left_rail(frame, summary_area, command_rail_style());

    let areas = if app.agents.detail_visible {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(content_area)
            .to_vec()
    } else {
        vec![content_area]
    };

    let filtered = app.filtered_agents();
    render_agent_list(frame, areas[0], &app.agents, &filtered);
    if app.agents.detail_visible {
        render_agent_details(frame, areas[1], app.agents.selected_agent(&filtered));
    }
}

fn render_agent_list(
    frame: &mut Frame<'_>,
    area: Rect,
    agents: &AgentsState,
    filtered: &[&AgentListItem],
) {
    render_surface(frame, area);
    let height = area.height as usize;
    let mut lines = Vec::new();

    if let Some(error) = &agents.last_error {
        lines.push(prefixed_line(format!("Refresh failed: {error}")));
    } else if agents.refresh_running && agents.agents.is_empty() {
        lines.push(prefixed_line("Refreshing agents..."));
    } else if filtered.is_empty() {
        lines.push(prefixed_line("No agents match the current filter."));
    } else {
        let selected = agents.selected.min(filtered.len().saturating_sub(1));
        let start = selected.saturating_sub(height.saturating_sub(1));
        for (index, agent) in filtered.iter().enumerate().skip(start).take(height) {
            let selected = index == selected;
            let marker = if selected { ">" } else { " " };
            let style = if selected {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            lines.push(Line::from(vec![
                Span::styled("┃ ", surface_rail_style()),
                Span::styled(marker, style),
                Span::raw(" "),
                fixed_span(&agent.name, 28, style),
                Span::raw(" "),
                fixed_span(agent.status.as_deref().unwrap_or("-"), 12, style),
                Span::raw(" "),
                fixed_span(agent.agent_type.as_deref().unwrap_or("-"), 20, style),
            ]));
        }
    }

    frame.render_widget(Paragraph::new(lines).style(surface_style()), area);
    render_left_rail(frame, area, surface_rail_style());
}

fn render_agent_details(frame: &mut Frame<'_>, area: Rect, agent: Option<&AgentListItem>) {
    render_surface(frame, area);
    let lines = match agent {
        Some(agent) => {
            let mut lines = vec![
                Line::from(vec![
                    Span::styled("┃ ", command_rail_style()),
                    Span::styled("Details", Style::default().add_modifier(Modifier::BOLD)),
                ]),
                prefixed_line(format!("Name      : {}", agent.name)),
                prefixed_line(format!(
                    "Status    : {}",
                    agent.status.as_deref().unwrap_or("-")
                )),
                prefixed_line(format!(
                    "Type      : {}",
                    agent.agent_type.as_deref().unwrap_or("-")
                )),
                prefixed_line(format!(
                    "Component : {}",
                    agent.component.as_deref().unwrap_or("-")
                )),
                Line::default(),
            ];
            let raw =
                serde_json::to_string_pretty(&agent.raw).unwrap_or_else(|_| agent.raw.to_string());
            lines.extend(
                raw.lines()
                    .take(area.height.saturating_sub(7) as usize)
                    .map(prefixed_line),
            );
            lines
        }
        None => vec![prefixed_line("Select an agent to see details.")],
    };
    frame.render_widget(Paragraph::new(lines).style(surface_style()), area);
    render_left_rail(frame, area, command_rail_style());
}

fn agent_status_line(agents: &AgentsState) -> Line<'static> {
    let count = agents.filtered_agents().len();
    let selected = if count == 0 {
        0
    } else {
        agents.selected.min(count - 1) + 1
    };
    Line::from(vec![
        Span::styled("┃ ", command_rail_style()),
        fixed_span(
            "agents",
            7,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        fixed_span(
            format!("mode:{}", agents.mode.label()),
            14,
            Style::default().fg(Color::Yellow),
        ),
        Span::raw(" | "),
        fixed_span(
            format!(
                "filter:{}",
                if agents.query.is_empty() {
                    "-"
                } else {
                    &agents.query
                }
            ),
            24,
            Style::default(),
        ),
        Span::raw(" | "),
        fixed_span(
            format!("{selected}/{count}"),
            8,
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw(" | "),
        flag_span("auto", agents.auto_refresh, false),
        Span::raw(" | "),
        fixed_span(
            if agents.refresh_running {
                "refreshing"
            } else {
                "u refresh"
            },
            12,
            if agents.refresh_running {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            },
        ),
    ])
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

fn server_status_line(server: &ServerState) -> Line<'static> {
    let run = &server.run;
    let clean = if run.is_running() {
        run.clean
    } else {
        server.clean
    };
    Line::from(vec![
        Span::styled("┃ ", command_rail_style()),
        fixed_span(
            "server",
            7,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        fixed_span(
            server_status_display(run),
            10,
            server_status_style(run.status),
        ),
        Span::raw(" | "),
        flag_span("clean", clean, true),
        Span::raw(" | "),
        fixed_span(command_display(&run.args), 34, Style::default()),
        Span::raw(" | "),
        fixed_span(server_hint(run), 21, Style::default().fg(Color::DarkGray)),
    ])
}

fn server_status_display(run: &ServerRun) -> String {
    if run.is_running() {
        format!(
            "{} {}",
            spinner_symbol(run.spinner_frame),
            run.status.title()
        )
    } else {
        run.status.title().to_string()
    }
}

fn server_status_style(status: ServerStatus) -> Style {
    match status {
        ServerStatus::Starting | ServerStatus::Running | ServerStatus::Stopping => {
            Style::default().fg(Color::Yellow)
        }
        ServerStatus::Stopped => Style::default().fg(Color::DarkGray),
        ServerStatus::Failed => Style::default().fg(Color::Red),
    }
    .add_modifier(Modifier::BOLD)
}

fn server_hint(run: &ServerRun) -> &'static str {
    match run.status {
        ServerStatus::Starting | ServerStatus::Running => "s/Ctrl-C stop",
        ServerStatus::Stopping => "s/Ctrl-C force kill",
        ServerStatus::Stopped | ServerStatus::Failed => "s start  R restart",
    }
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
    let actions = filtered_actions(&app.palette.query);
    let visible_actions = actions.iter().take(8).copied().collect::<Vec<_>>();
    let width_actions = ACTIONS.to_vec();
    let label_width = palette_label_width(&width_actions);
    let area = centered_rect_fixed(
        palette_width(
            &app.palette.query,
            &width_actions,
            label_width,
            frame.area().width,
        ),
        palette_height(width_actions.len().min(8), frame.area().height),
        frame.area(),
    );
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
        for (index, action) in visible_actions.iter().enumerate() {
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
                Span::styled(format!("{label:<label_width$}"), style),
                Span::raw("  "),
                Span::raw(action.description),
            ]));
        }
    }

    let block = Block::default().borders(Borders::ALL).title(" Search ");
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn palette_label_width(actions: &[TuiAction]) -> usize {
    actions
        .iter()
        .map(|action| palette_action_label(action, "  ").chars().count())
        .max()
        .unwrap_or("No matching commands".len())
        .max(18)
}

fn palette_action_label(action: &TuiAction, prefix: &str) -> String {
    let shortcut = action
        .shortcut
        .map(|shortcut| format!(" ({shortcut})"))
        .unwrap_or_default();
    format!("{prefix}{}{}", action.label, shortcut)
}

fn palette_width(
    query: &str,
    actions: &[TuiAction],
    label_width: usize,
    terminal_width: u16,
) -> u16 {
    let action_width = actions
        .iter()
        .map(|action| label_width + 2 + action.description.chars().count())
        .max()
        .unwrap_or("No matching commands".len());
    let content_width = action_width
        .max("Command Palette".len())
        .max(query.chars().count() + 2);
    let max_width = terminal_width.saturating_sub(4).max(20) as usize;
    let min_width = 36.min(max_width);
    (content_width + 2).clamp(min_width, max_width) as u16
}

fn palette_height(action_count: usize, terminal_height: u16) -> u16 {
    let content_height = 3 + action_count.max(1);
    let max_height = terminal_height.saturating_sub(4).max(6) as usize;
    (content_height + 2).clamp(6, max_height) as u16
}

fn centered_rect_fixed(width: u16, height: u16, area: Rect) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
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
    StartServer,
    StopServer,
    RestartServer,
    CleanRestartServer,
    ToggleServerClean,
    RefreshAgents,
    ToggleAgentAutoRefresh,
    CycleAgentMode,
    ToggleAgentDetails,
    ShowHelp,
    Quit,
}

const ACTIONS: [TuiAction; 20] = [
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
        label: "Start Server",
        description: "Run golem server run",
        shortcut: Some("s"),
        kind: TuiActionKind::StartServer,
    },
    TuiAction {
        label: "Stop Server",
        description: "Stop the local server",
        shortcut: Some("s"),
        kind: TuiActionKind::StopServer,
    },
    TuiAction {
        label: "Restart Server",
        description: "Restart the local server",
        shortcut: Some("R"),
        kind: TuiActionKind::RestartServer,
    },
    TuiAction {
        label: "Clean Restart Server",
        description: "Restart local server with --clean",
        shortcut: Some("C"),
        kind: TuiActionKind::CleanRestartServer,
    },
    TuiAction {
        label: "Toggle Server Clean",
        description: "Toggle --clean for next server start",
        shortcut: Some("x"),
        kind: TuiActionKind::ToggleServerClean,
    },
    TuiAction {
        label: "Refresh Agents",
        description: "Refresh the agent list",
        shortcut: Some("u"),
        kind: TuiActionKind::RefreshAgents,
    },
    TuiAction {
        label: "Toggle Agent Auto Refresh",
        description: "Toggle automatic agent refresh",
        shortcut: Some("a"),
        kind: TuiActionKind::ToggleAgentAutoRefresh,
    },
    TuiAction {
        label: "Cycle Agent Mode",
        description: "Cycle durable, ephemeral, all",
        shortcut: Some("m"),
        kind: TuiActionKind::CycleAgentMode,
    },
    TuiAction {
        label: "Toggle Agent Details",
        description: "Show or hide selected agent details",
        shortcut: Some("i"),
        kind: TuiActionKind::ToggleAgentDetails,
    },
    TuiAction {
        label: "Go to Dashboard",
        description: "Switch to Dashboard view",
        shortcut: Some("1"),
        kind: TuiActionKind::SelectView(TuiView::Dashboard),
    },
    TuiAction {
        label: "Go to Agents",
        description: "Switch to Agents view",
        shortcut: Some("2"),
        kind: TuiActionKind::SelectView(TuiView::Agents),
    },
    TuiAction {
        label: "Go to Output",
        description: "Switch to Output view",
        shortcut: Some("3"),
        kind: TuiActionKind::SelectView(TuiView::Output),
    },
    TuiAction {
        label: "Go to Server",
        description: "Switch to Server view",
        shortcut: Some("4"),
        kind: TuiActionKind::SelectView(TuiView::Server),
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
        assert!(frame.contains("Agents"), "{frame}");
        assert!(!frame.contains("Environments"), "{frame}");
        assert!(!frame.contains("Components"), "{frame}");
    }

    #[test]
    fn switches_tabs_with_keys() {
        let mut app = test_app();

        app.handle_key(key(KeyCode::Char(']')));
        let frame = render_app_text(&app);
        assert_eq!(app.active_view, TuiView::Agents);
        assert!(frame.contains("Agents"), "{frame}");

        app.handle_key(key(KeyCode::Char('[')));
        let frame = render_app_text(&app);
        assert_eq!(app.active_view, TuiView::Dashboard);
        assert!(frame.contains("Dashboard"), "{frame}");
    }

    #[test]
    fn jumps_to_tab_with_number() {
        let mut app = test_app();

        app.handle_key(key(KeyCode::Char('2')));
        let frame = render_app_text(&app);

        assert_eq!(app.active_view, TuiView::Agents);
        assert!(frame.contains("Agents"), "{frame}");
    }

    #[test]
    fn opens_palette() {
        let mut app = test_app();

        app.handle_key(modified_key(KeyCode::Char('p'), KeyModifiers::CONTROL));
        let frame = render_app_text(&app);

        assert!(frame.contains("Command Palette"), "{frame}");
        assert!(frame.contains("Build"), "{frame}");
    }

    #[test]
    fn palette_width_fits_visible_actions() {
        let actions = filtered_actions("");
        let visible = actions.iter().take(8).copied().collect::<Vec<_>>();
        let label_width = palette_label_width(&visible);
        let width = palette_width("", &visible, label_width, 120);

        assert!(width > 36);
        assert!(width <= 116);
    }

    #[test]
    fn palette_width_is_clamped_on_narrow_terminals() {
        let actions = filtered_actions("");
        let visible = actions.iter().take(8).copied().collect::<Vec<_>>();
        let label_width = palette_label_width(&visible);

        assert_eq!(palette_width("", &visible, label_width, 30), 26);
    }

    #[test]
    fn palette_width_does_not_shrink_when_filtering() {
        let width_actions = ACTIONS.to_vec();
        let label_width = palette_label_width(&width_actions);

        assert_eq!(
            palette_width("", &width_actions, label_width, 120),
            palette_width("comp", &width_actions, label_width, 120)
        );
    }

    #[test]
    fn palette_height_does_not_shrink_when_filtering() {
        let height = palette_height(ACTIONS.len().min(8), 40);

        assert_eq!(height, palette_height(ACTIONS.len().min(8), 40));
        assert!(height > palette_height(1, 40));
    }

    #[test]
    fn agent_filter_matches_fuzzily_and_selection_moves() {
        let mut app = test_app();
        app.agents.agents = sample_agents();

        app.mode = TuiMode::AgentFilter;
        for character in "cart".chars() {
            app.handle_key(key(KeyCode::Char(character)));
        }
        assert_eq!(app.filtered_agents().len(), 2);

        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.agents.selected, 1);
        app.handle_key(key(KeyCode::Up));
        assert_eq!(app.agents.selected, 0);
    }

    #[test]
    fn agent_mode_cycles_and_preserves_filter() {
        let mut app = test_app();
        app.active_view = TuiView::Agents;
        app.agents.query = "cart".to_string();
        app.agents.selected = 3;

        app.handle_key(key(KeyCode::Char('m')));

        assert_eq!(app.agents.mode, AgentModeFilter::Ephemeral);
        assert_eq!(app.agents.query, "cart");
        assert_eq!(app.agents.selected, 0);
    }

    #[test]
    fn agent_details_panel_toggles() {
        let mut app = test_app();
        app.active_view = TuiView::Agents;
        app.agents.agents = sample_agents();

        let frame = render_app_text(&app);
        assert!(frame.contains("Details"), "{frame}");

        app.handle_key(key(KeyCode::Char('i')));
        let frame = render_app_text(&app);
        assert!(!frame.contains("Details"), "{frame}");
    }

    #[test]
    fn agent_json_parsing_handles_values_and_plain_arrays() {
        let values = parse_agent_list_output(
            r#"{"values":[{"name":"cart-1","status":"Running","agentTypeName":"CartAgent"}]}"#,
        )
        .unwrap();
        assert_eq!(values[0].name, "cart-1");
        assert_eq!(values[0].status.as_deref(), Some("Running"));

        let array = parse_agent_list_output(
            r#"[{"agentName":"order-1","componentName":"orders","type":"OrderAgent"}]"#,
        )
        .unwrap();
        assert_eq!(array[0].name, "order-1");
        assert_eq!(array[0].component.as_deref(), Some("orders"));
    }

    #[test]
    fn agent_refresh_result_updates_agents() {
        let mut app = test_app();
        let (tx, _rx) = mpsc::channel();
        app.agents.refresh_generation = 1;
        app.agents.refresh_running = true;

        app.handle_event(
            TuiEvent::AgentRefreshFinished {
                generation: 1,
                result: Ok(r#"[{"name":"cart-1"}]"#.to_string()),
            },
            &tx,
        );

        assert!(!app.agents.refresh_running);
        assert_eq!(app.agents.agents.len(), 1);
        assert_eq!(app.agents.agents[0].name, "cart-1");
    }

    #[test]
    fn filters_palette() {
        let mut app = test_app();

        app.handle_key(modified_key(KeyCode::Char('p'), KeyModifiers::CONTROL));
        for character in "agent".chars() {
            app.handle_key(key(KeyCode::Char(character)));
        }
        let frame = render_app_text(&app);

        assert!(frame.contains("> agent"), "{frame}");
        assert!(frame.contains("Go to Agents"), "{frame}");
    }

    #[test]
    fn executes_palette_action() {
        let mut app = test_app();

        app.handle_key(modified_key(KeyCode::Char('p'), KeyModifiers::CONTROL));
        for character in "go agents".chars() {
            app.handle_key(key(KeyCode::Char(character)));
        }
        app.handle_key(key(KeyCode::Enter));

        let frame = render_app_text(&app);
        assert_eq!(app.active_view, TuiView::Agents);
        assert!(frame.contains("Agents"), "{frame}");
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

    #[test]
    fn server_tab_renders_initial_state() {
        let mut app = test_app();

        app.handle_key(key(KeyCode::Char('4')));
        let frame = render_app_text(&app);

        assert_eq!(app.active_view, TuiView::Server);
        assert!(frame.contains("server"), "{frame}");
        assert!(frame.contains("stopped"), "{frame}");
        assert!(frame.contains("clean:off"), "{frame}");
    }

    #[test]
    fn server_clean_toggle_affects_next_start() {
        let mut app = test_app();
        app.active_view = TuiView::Server;

        app.handle_key(key(KeyCode::Char('x')));
        app.handle_key(key(KeyCode::Char('s')));

        assert!(app.server.clean);
        assert!(app.server.run.clean);
        assert_eq!(app.server.run.args, vec!["server", "run", "--clean"]);
    }

    #[test]
    fn server_start_stop_and_restart_state() {
        let mut app = test_app();
        app.active_view = TuiView::Server;

        app.handle_key(key(KeyCode::Char('s')));
        assert_eq!(app.server.run.status, ServerStatus::Starting);
        assert_eq!(app.server.run.args, vec!["server", "run"]);

        app.handle_key(key(KeyCode::Char('s')));
        assert_eq!(app.server.run.status, ServerStatus::Stopping);

        app.server.run.status = ServerStatus::Running;
        app.handle_key(key(KeyCode::Char('R')));
        assert_eq!(app.server.run.status, ServerStatus::Stopping);
        assert_eq!(
            app.server.run.restart_after_stop,
            Some(ServerStartMode::Current)
        );

        app.server.run.status = ServerStatus::Running;
        app.handle_key(key(KeyCode::Char('C')));
        assert_eq!(
            app.server.run.restart_after_stop,
            Some(ServerStartMode::Clean)
        );
    }

    #[test]
    fn server_logs_are_separate_from_command_output() {
        let mut app = test_app();

        app.handle_key(key(KeyCode::Char('b')));
        app.append_command_output(b"command log\n");
        app.server.run.output.append(b"server log\n");

        app.active_view = TuiView::Output;
        let output_frame = render_app_text(&app);
        assert!(output_frame.contains("command log"), "{output_frame}");
        assert!(!output_frame.contains("server log"), "{output_frame}");

        app.active_view = TuiView::Server;
        let server_frame = render_app_text(&app);
        assert!(server_frame.contains("server log"), "{server_frame}");
        assert!(!server_frame.contains("command log"), "{server_frame}");
    }

    #[test]
    fn server_mouse_scrolls_logs() {
        let mut app = test_app();
        app.active_view = TuiView::Server;
        for index in 0..20 {
            app.server
                .run
                .output
                .append(format!("server line {index}\n").as_bytes());
        }

        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 10,
            row: 10,
            modifiers: KeyModifiers::empty(),
        });

        assert_eq!(app.server.run.output.scroll_offset, 3);
    }

    fn test_app() -> TuiApp {
        TuiApp {
            should_quit: false,
            active_view: TuiView::Dashboard,
            mode: TuiMode::Normal,
            palette: CommandPalette::default(),
            command_options: CommandOptions::default(),
            command_run: None,
            server: ServerState::default(),
            agents: AgentsState::default(),
            next_command_id: 1,
            context: TuiContextInfo {
                application: "sample-app".to_string(),
                environment: "local".to_string(),
                server: "local".to_string(),
                config_dir: "/tmp/golem-config".to_string(),
            },
        }
    }

    fn sample_agents() -> Vec<AgentListItem> {
        vec![
            AgentListItem {
                name: "cart-1".to_string(),
                component: Some("cart".to_string()),
                agent_type: Some("CartAgent".to_string()),
                status: Some("Running".to_string()),
                raw: serde_json::json!({"name":"cart-1"}),
            },
            AgentListItem {
                name: "cart-2".to_string(),
                component: Some("cart".to_string()),
                agent_type: Some("CartAgent".to_string()),
                status: Some("Idle".to_string()),
                raw: serde_json::json!({"name":"cart-2"}),
            },
            AgentListItem {
                name: "order-1".to_string(),
                component: Some("orders".to_string()),
                agent_type: Some("OrderAgent".to_string()),
                status: Some("Running".to_string()),
                raw: serde_json::json!({"name":"order-1"}),
            },
        ]
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
