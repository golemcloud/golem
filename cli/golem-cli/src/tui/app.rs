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

use crate::command_handler::Handlers;
use crate::context::Context;
use crate::model::app_raw::{BuiltinServer, Server};
use crate::model::worker::{
    AgentListMode, AgentListRequest, AgentMetadataView, AgentsMetadataResponseView,
};
use crate::tui::TuiEvent;
use crate::tui::context_executor::{TuiContextExecutor, TuiContextId, TuiContextTaskResult};
use crate::tui::input::encode_key_for_pty;
use crate::tui::nested_cli::{
    CommandExit, NestedCliRuntime, NestedCliSpec, NestedCliTarget, spawn_nested_cli,
};
use crate::tui::terminal::TerminalGuard;
use crate::tui::terminal_screen::TerminalScreen;
use ansi_to_tui::IntoText;
use crossterm::event::{
    Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind,
};
use futures_util::StreamExt;
use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::mpsc::{self, Sender};
use tokio::time::{Duration, sleep};

const TUI_EVENT_CHANNEL_CAPACITY: usize = 1024;

type TuiEventSender = Sender<TuiEvent>;

pub async fn run(ctx: Arc<Context>) -> anyhow::Result<()> {
    let context_executor = Arc::new(TuiContextExecutor::new(ctx.clone()));
    let mut app = TuiApp::from_context(ctx.as_ref());
    app.context_executor = Some(context_executor);
    let mut terminal = TerminalGuard::enter()?;
    let (event_tx, mut event_rx) = mpsc::channel::<TuiEvent>(TUI_EVENT_CHANNEL_CAPACITY);
    spawn_terminal_event_reader(event_tx.clone());

    terminal.draw(|frame| render(frame, &app))?;

    while !app.should_quit {
        let Some(event) = event_rx.recv().await else {
            break;
        };
        app.handle_event(event, &event_tx);
        terminal.draw(|frame| render(frame, &app))?;
    }

    app.cleanup_running_command();
    Ok(())
}

fn spawn_terminal_event_reader(event_tx: TuiEventSender) {
    tokio::spawn(async move {
        let mut reader = EventStream::new();
        while let Some(event) = reader.next().await {
            match event {
                Ok(event) => {
                    if event_tx.send(TuiEvent::Terminal(event)).await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
}

fn spawn_command_spinner(command_id: u64, event_tx: TuiEventSender, stop: Arc<AtomicBool>) {
    tokio::spawn(async move {
        while !stop.load(Ordering::Relaxed) {
            sleep(Duration::from_millis(120)).await;
            if stop.load(Ordering::Relaxed) {
                return;
            }
            if event_channel_closed(event_tx.try_send(TuiEvent::SpinnerTick(command_id))) {
                return;
            }
        }
    });
}

fn spawn_server_spinner(server_id: u64, event_tx: TuiEventSender, stop: Arc<AtomicBool>) {
    tokio::spawn(async move {
        while !stop.load(Ordering::Relaxed) {
            sleep(Duration::from_millis(120)).await;
            if stop.load(Ordering::Relaxed) {
                return;
            }
            if event_channel_closed(event_tx.try_send(TuiEvent::ServerSpinnerTick(server_id))) {
                return;
            }
        }
    });
}

fn spawn_agent_auto_refresh(event_tx: TuiEventSender, stop: Arc<AtomicBool>) {
    tokio::spawn(async move {
        while !stop.load(Ordering::Relaxed) {
            sleep(Duration::from_secs(5)).await;
            if stop.load(Ordering::Relaxed) {
                return;
            }
            if event_channel_closed(event_tx.try_send(TuiEvent::AgentRefreshTick)) {
                return;
            }
        }
    });
}

fn event_channel_closed(result: Result<(), TrySendError<TuiEvent>>) -> bool {
    matches!(result, Err(TrySendError::Closed(_)))
}

struct TuiApp {
    should_quit: bool,
    active_view: TuiView,
    mode: TuiMode,
    palette: CommandPalette,
    command_options: CommandOptions,
    command_run: Option<CommandRun>,
    server: ServerState,
    repl: ReplState,
    agents: AgentsState,
    next_command_id: u64,
    context: TuiContextInfo,
    context_executor: Option<Arc<TuiContextExecutor>>,
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
            repl: ReplState::default(),
            agents: AgentsState::default(),
            next_command_id: 1,
            context: TuiContextInfo::from_context(ctx),
            context_executor: None,
        }
    }

    fn handle_event(&mut self, event: TuiEvent, event_tx: &TuiEventSender) {
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
            TuiEvent::ReplOutput(bytes) => self.append_repl_output(&bytes),
            TuiEvent::ReplOutputClosed(error) => self.handle_repl_output_closed(error),
            TuiEvent::ReplExited(exit) => self.finish_repl(exit),
            TuiEvent::AgentOplogOutput(bytes) => self.agents.inspect.oplog.output.append(&bytes),
            TuiEvent::AgentOplogOutputClosed(error) => {
                if let Some(error) = error {
                    self.agents
                        .inspect
                        .oplog
                        .output
                        .append_local_line(format!("oplog output closed: {error}"));
                }
            }
            TuiEvent::AgentOplogExited(exit) => {
                self.finish_agent_inspect_job(AgentInspectPane::Oplog, exit)
            }
            TuiEvent::AgentStreamOutput(bytes) => self.agents.inspect.stream.output.append(&bytes),
            TuiEvent::AgentStreamOutputClosed(error) => {
                if let Some(error) = error {
                    self.agents
                        .inspect
                        .stream
                        .output
                        .append_local_line(format!("stream output closed: {error}"));
                }
            }
            TuiEvent::AgentStreamExited(exit) => {
                self.finish_agent_inspect_job(AgentInspectPane::Stream, exit)
            }
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

    fn handle_key_with_events(&mut self, key: KeyEvent, event_tx: Option<&TuiEventSender>) {
        match self.mode {
            TuiMode::Normal => self.handle_global_key(key, event_tx),
            TuiMode::LeaderNormal => self.handle_leader_key(key, event_tx, TuiMode::Normal),
            TuiMode::Palette => self.handle_palette_key(key, event_tx),
            TuiMode::Help => self.handle_help_key(key),
            TuiMode::AgentFilter => self.handle_agent_filter_key(key, event_tx),
            TuiMode::CommandInteraction => self.handle_command_interaction_key(key),
            TuiMode::Repl => self.handle_repl_key(key),
            TuiMode::LeaderRepl => self.handle_leader_key(key, event_tx, TuiMode::Repl),
        }
    }

    fn handle_global_key(&mut self, key: KeyEvent, event_tx: Option<&TuiEventSender>) {
        if self.active_view == TuiView::Agents && self.agents.view_mode == AgentsViewMode::Inspect {
            if key.code == KeyCode::Char('?') {
                self.mode = TuiMode::Help;
                return;
            }
            self.handle_agent_inspect_key(key);
            return;
        }

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
            KeyCode::Char('x') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.mode = TuiMode::LeaderNormal
            }
            KeyCode::Char('r') => self.start_or_focus_repl(event_tx),
            KeyCode::Char('u') if self.active_view == TuiView::Agents => {
                self.refresh_agents(event_tx)
            }
            KeyCode::Enter if self.active_view == TuiView::Agents => {
                self.open_agent_inspect(event_tx)
            }
            KeyCode::Char('/') if self.active_view == TuiView::Agents => {
                self.mode = TuiMode::AgentFilter
            }
            KeyCode::Char('s') => self.open_and_toggle_server(event_tx),
            KeyCode::Enter if self.active_view == TuiView::Server => self.toggle_server(event_tx),
            KeyCode::Enter if self.active_view == TuiView::Repl => {
                self.start_or_focus_repl(event_tx)
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
            KeyCode::Char('5') => self.active_view = TuiView::Repl,
            _ => {}
        }
    }

    fn handle_palette_key(&mut self, key: KeyEvent, event_tx: Option<&TuiEventSender>) {
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
                    self.execute_palette_action(action.kind, event_tx);
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

    fn handle_agent_filter_key(&mut self, key: KeyEvent, event_tx: Option<&TuiEventSender>) {
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

    fn handle_leader_key(
        &mut self,
        key: KeyEvent,
        event_tx: Option<&TuiEventSender>,
        return_mode: TuiMode,
    ) {
        match key.code {
            KeyCode::Esc => self.mode = return_mode,
            KeyCode::Char('?') => self.mode = TuiMode::Help,
            KeyCode::Char('p') => self.open_palette(),
            KeyCode::Char('y') => {
                self.command_options.yes = !self.command_options.yes;
                self.mode = return_mode;
            }
            KeyCode::Char('r') => {
                self.command_options.reset = !self.command_options.reset;
                self.mode = return_mode;
            }
            KeyCode::Char('s') => {
                self.server.clean = !self.server.clean;
                self.mode = return_mode;
            }
            KeyCode::Char('a') if self.active_view == TuiView::Agents => {
                self.toggle_agent_auto_refresh(event_tx);
                self.mode = return_mode;
            }
            KeyCode::Char('d') if self.active_view == TuiView::Agents => {
                self.agents.detail_visible = !self.agents.detail_visible;
                self.mode = return_mode;
            }
            KeyCode::Char('m') if self.active_view == TuiView::Agents => {
                self.cycle_agent_mode(event_tx);
                self.mode = return_mode;
            }
            KeyCode::Char('q') if return_mode == TuiMode::Repl => self.mode = TuiMode::Normal,
            KeyCode::Char('k') if return_mode == TuiMode::Repl => {
                self.stop_repl();
                self.mode = TuiMode::Normal;
            }
            KeyCode::Char('R') if return_mode == TuiMode::Repl => {
                self.restart_repl(event_tx);
                self.mode = TuiMode::Repl;
            }
            KeyCode::Char('R') if self.active_view == TuiView::Server => {
                self.restart_server(ServerStartMode::Current, event_tx);
                self.mode = return_mode;
            }
            KeyCode::Char('C') if self.active_view == TuiView::Server => {
                self.restart_server(ServerStartMode::Clean, event_tx);
                self.mode = return_mode;
            }
            _ => self.mode = return_mode,
        }
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) {
        match (self.active_view, mouse.kind) {
            (TuiView::Output, MouseEventKind::ScrollUp) => self.scroll_output_up_by(3),
            (TuiView::Output, MouseEventKind::ScrollDown) => self.scroll_output_down_by(3),
            (TuiView::Server, MouseEventKind::ScrollUp) => self.scroll_server_up_by(3),
            (TuiView::Server, MouseEventKind::ScrollDown) => self.scroll_server_down_by(3),
            (TuiView::Agents, MouseEventKind::ScrollUp)
                if self.agents.view_mode == AgentsViewMode::Inspect =>
            {
                self.scroll_agent_inspect_up_by(3)
            }
            (TuiView::Agents, MouseEventKind::ScrollDown)
                if self.agents.view_mode == AgentsViewMode::Inspect =>
            {
                self.scroll_agent_inspect_down_by(3)
            }
            _ => {}
        }
    }

    fn execute_action(&mut self, action: TuiActionKind, event_tx: Option<&TuiEventSender>) {
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
            TuiActionKind::ToggleServer => {
                self.close_palette();
                self.open_and_toggle_server(event_tx);
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
            TuiActionKind::StartOrFocusRepl => {
                self.close_palette();
                self.start_or_focus_repl(event_tx);
            }
            TuiActionKind::FocusRepl => {
                self.close_palette();
                self.focus_repl();
            }
            TuiActionKind::LeaveRepl => {
                self.palette.reset();
                self.mode = TuiMode::Normal;
            }
            TuiActionKind::StopRepl => {
                self.close_palette();
                self.stop_repl();
            }
            TuiActionKind::RestartRepl => {
                self.close_palette();
                self.restart_repl(event_tx);
            }
            TuiActionKind::OpenPalette => {
                self.open_palette();
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

    fn execute_palette_action(&mut self, action: TuiActionKind, event_tx: Option<&TuiEventSender>) {
        let Some(metadata) = action_by_kind(action) else {
            return;
        };

        if self.action_availability(metadata).is_unavailable() {
            return;
        }

        self.execute_action(action, event_tx);
    }

    fn action_availability(&self, action: &TuiAction) -> TuiActionAvailability {
        match action.id {
            TuiActionId::Build | TuiActionId::Deploy | TuiActionId::Clean
                if self.command_is_running() =>
            {
                TuiActionAvailability::Unavailable("command already running")
            }
            TuiActionId::RefreshAgents if self.agents.refresh_running => {
                TuiActionAvailability::Unavailable("refresh already running")
            }
            TuiActionId::RefreshAgents if self.context_executor.is_none() => {
                TuiActionAvailability::Unavailable("context executor unavailable")
            }
            _ => TuiActionAvailability::Available,
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
        } else if self.active_view == TuiView::Repl && self.repl.is_running() {
            TuiMode::Repl
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

    fn open_agents_view(&mut self, event_tx: Option<&TuiEventSender>) {
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

    fn refresh_agents(&mut self, event_tx: Option<&TuiEventSender>) {
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
        let Some(context_executor) = self.context_executor.clone() else {
            self.agents.refresh_running = false;
            self.agents.last_error = Some("TUI context executor is not available".to_string());
            return;
        };
        self.agents.refresh_context_id = Some(context_executor.current_context_id());
        let mode = self.agents.mode;
        context_executor.spawn(
            event_tx,
            move |launch_context| async move {
                let request = AgentListRequest {
                    mode: mode.agent_list_mode(),
                    stable_sort: true,
                    ..AgentListRequest::default()
                };
                launch_context
                    .context()
                    .worker_handler()
                    .list_agent_metadata(request)
                    .await
            },
            move |result| TuiEvent::AgentRefreshFinished { generation, result },
        );
    }

    fn finish_agent_refresh(
        &mut self,
        generation: u64,
        result: TuiContextTaskResult<AgentsMetadataResponseView>,
    ) {
        if generation != self.agents.refresh_generation {
            return;
        }

        let (context_id, result, logs) = result.into_parts();
        if Some(context_id) != self.agents.refresh_context_id {
            return;
        }

        self.agents.refresh_running = false;
        match result {
            Ok(response) => {
                self.agents.agents = agent_items_from_metadata_response(response);
                self.agents.last_error = None;
                self.agents.clamp_selection();
            }
            Err(error) => {
                self.agents.last_error = Some(agent_refresh_error(error, logs));
            }
        }
    }

    fn toggle_agent_auto_refresh(&mut self, event_tx: Option<&TuiEventSender>) {
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

    fn cycle_agent_mode(&mut self, event_tx: Option<&TuiEventSender>) {
        self.agents.mode = self.agents.mode.next();
        self.agents.selected = 0;
        self.refresh_agents(event_tx);
    }

    fn handle_agent_inspect_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.close_agent_inspect(),
            KeyCode::Left => self.agents.inspect.focus = AgentInspectPane::Oplog,
            KeyCode::Right => self.agents.inspect.focus = AgentInspectPane::Stream,
            KeyCode::Up => self.scroll_agent_inspect_up_by(1),
            KeyCode::Down => self.scroll_agent_inspect_down_by(1),
            KeyCode::PageUp => self.scroll_agent_inspect_up_by(10),
            KeyCode::PageDown => self.scroll_agent_inspect_down_by(10),
            KeyCode::Home => self.focused_agent_inspect_job_mut().output.scroll_top(),
            KeyCode::End => self.focused_agent_inspect_job_mut().output.scroll_bottom(),
            _ => {}
        }
    }

    fn open_agent_inspect(&mut self, event_tx: Option<&TuiEventSender>) {
        let Some(agent_name) = self
            .filtered_agents()
            .get(self.agents.selected)
            .map(|agent| agent.name.clone())
        else {
            return;
        };

        self.close_agent_inspect_jobs();
        self.agents.view_mode = AgentsViewMode::Inspect;
        self.agents.inspect = AgentInspectState {
            agent_name: Some(agent_name.clone()),
            focus: AgentInspectPane::Oplog,
            oplog: InspectJob::start(vec![
                "agent".to_string(),
                "oplog".to_string(),
                agent_name.clone(),
            ]),
            stream: InspectJob::start(vec!["agent".to_string(), "stream".to_string(), agent_name]),
        };

        self.agents.inspect.oplog.output.append_local_line(format!(
            "$ {}",
            command_display(&self.agents.inspect.oplog.args)
        ));
        self.agents.inspect.stream.output.append_local_line(format!(
            "$ {}",
            command_display(&self.agents.inspect.stream.args)
        ));

        let Some(event_tx) = event_tx else {
            return;
        };

        self.start_agent_inspect_job(AgentInspectPane::Oplog, event_tx);
        self.start_agent_inspect_job(AgentInspectPane::Stream, event_tx);
    }

    fn start_agent_inspect_job(&mut self, pane: AgentInspectPane, event_tx: &TuiEventSender) {
        let args = self.agent_inspect_job(pane).args.clone();
        let spec = match self.command_spec(args) {
            Ok(spec) => spec,
            Err(error) => {
                let job = self.agent_inspect_job_mut(pane);
                job.status = InspectJobStatus::Failed;
                job.output
                    .append_local_line(format!("failed to prepare command: {error:#}"));
                return;
            }
        };

        let target = match pane {
            AgentInspectPane::Oplog => NestedCliTarget::AgentOplog,
            AgentInspectPane::Stream => NestedCliTarget::AgentStream,
        };

        match spawn_nested_cli(spec, event_tx.clone(), target) {
            Ok(runtime) => {
                let job = self.agent_inspect_job_mut(pane);
                job.status = InspectJobStatus::Running;
                job.runtime = Some(runtime);
            }
            Err(error) => {
                let job = self.agent_inspect_job_mut(pane);
                job.status = InspectJobStatus::Failed;
                job.output
                    .append_local_line(format!("failed to start command: {error:#}"));
            }
        }
    }

    fn close_agent_inspect(&mut self) {
        self.close_agent_inspect_jobs();
        self.agents.view_mode = AgentsViewMode::List;
    }

    fn close_agent_inspect_jobs(&mut self) {
        for pane in [AgentInspectPane::Oplog, AgentInspectPane::Stream] {
            let job = self.agent_inspect_job_mut(pane);
            if let Some(runtime) = job.runtime.as_mut() {
                let _ = runtime.kill();
            }
            job.runtime = None;
            if job.is_running() {
                job.status = InspectJobStatus::Stopped;
            }
        }
    }

    fn finish_agent_inspect_job(&mut self, pane: AgentInspectPane, exit: CommandExit) {
        let job = self.agent_inspect_job_mut(pane);
        job.runtime = None;
        job.exit_code = exit.code;
        job.status = if exit.success || job.status == InspectJobStatus::Stopping {
            InspectJobStatus::Stopped
        } else {
            InspectJobStatus::Failed
        };
    }

    fn scroll_agent_inspect_up_by(&mut self, amount: usize) {
        self.focused_agent_inspect_job_mut()
            .output
            .scroll_up(amount);
    }

    fn scroll_agent_inspect_down_by(&mut self, amount: usize) {
        self.focused_agent_inspect_job_mut()
            .output
            .scroll_down(amount);
    }

    fn focused_agent_inspect_job_mut(&mut self) -> &mut InspectJob {
        self.agent_inspect_job_mut(self.agents.inspect.focus)
    }

    fn agent_inspect_job(&self, pane: AgentInspectPane) -> &InspectJob {
        match pane {
            AgentInspectPane::Oplog => &self.agents.inspect.oplog,
            AgentInspectPane::Stream => &self.agents.inspect.stream,
        }
    }

    fn agent_inspect_job_mut(&mut self, pane: AgentInspectPane) -> &mut InspectJob {
        match pane {
            AgentInspectPane::Oplog => &mut self.agents.inspect.oplog,
            AgentInspectPane::Stream => &mut self.agents.inspect.stream,
        }
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
                if let Some(bytes) = encode_key_for_pty(key)
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

    fn start_command(&mut self, kind: CommandKind, event_tx: Option<&TuiEventSender>) {
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
        self.cleanup_repl();
        self.close_agent_inspect_jobs();
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
        self.resize_repl_for_terminal(cols, rows);
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

    fn toggle_server(&mut self, event_tx: Option<&TuiEventSender>) {
        if self.server.run.is_running() {
            self.stop_server();
        } else {
            self.start_server(ServerStartMode::Current, event_tx);
        }
    }

    fn open_and_toggle_server(&mut self, event_tx: Option<&TuiEventSender>) {
        self.active_view = TuiView::Server;
        self.toggle_server(event_tx);
    }

    fn start_server(&mut self, mode: ServerStartMode, event_tx: Option<&TuiEventSender>) {
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

    fn restart_server(&mut self, mode: ServerStartMode, event_tx: Option<&TuiEventSender>) {
        if self.server.run.is_running() {
            self.server.run.restart_after_stop = Some(mode);
            self.stop_server();
        } else {
            self.start_server(mode, event_tx);
        }
    }

    fn finish_server(&mut self, exit: CommandExit, event_tx: &TuiEventSender) {
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

    fn start_or_focus_repl(&mut self, event_tx: Option<&TuiEventSender>) {
        self.active_view = TuiView::Repl;
        if self.repl.is_running() {
            self.mode = TuiMode::Repl;
            return;
        }

        self.start_repl(event_tx);
    }

    fn start_repl(&mut self, event_tx: Option<&TuiEventSender>) {
        let args = vec!["repl".to_string()];
        let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
        let (screen_rows, screen_cols) = repl_screen_size_for_terminal(cols, rows);
        self.repl.run = ReplRun::new(args.clone(), screen_rows, screen_cols);
        self.mode = TuiMode::Repl;
        self.repl
            .run
            .screen
            .feed(format!("$ {}\r\n", command_display(&args)).as_bytes());

        let Some(event_tx) = event_tx else {
            return;
        };

        let spec = match self.repl_spec(args) {
            Ok(spec) => spec,
            Err(error) => {
                self.repl.run.status = ReplStatus::Failed;
                self.repl.run.last_error = Some(format!("failed to prepare REPL: {error:#}"));
                self.mode = TuiMode::Normal;
                return;
            }
        };

        match spawn_nested_cli(spec, event_tx.clone(), NestedCliTarget::Repl) {
            Ok(mut runtime) => {
                let _ = runtime.resize(screen_cols, screen_rows);
                self.repl.run.status = ReplStatus::Running;
                self.repl.run.runtime = Some(runtime);
            }
            Err(error) => {
                self.repl.run.status = ReplStatus::Failed;
                self.repl.run.last_error = Some(format!("failed to start REPL: {error:#}"));
                self.mode = TuiMode::Normal;
            }
        }
    }

    fn repl_spec(&self, args: Vec<String>) -> anyhow::Result<NestedCliSpec> {
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

    fn focus_repl(&mut self) {
        self.active_view = TuiView::Repl;
        if self.repl.is_running() {
            self.mode = TuiMode::Repl;
        }
    }

    fn handle_repl_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Char('x') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.mode = TuiMode::LeaderRepl;
            return;
        }

        if let Some(bytes) = encode_key_for_pty(key)
            && let Some(runtime) = self.repl.run.runtime.as_mut()
            && let Err(error) = runtime.write_all(&bytes)
        {
            self.repl.run.last_error = Some(format!("failed to send REPL input: {error:#}"));
        }
    }

    fn append_repl_output(&mut self, bytes: &[u8]) {
        self.repl.run.screen.feed(bytes);
        if self.repl.run.status == ReplStatus::Starting {
            self.repl.run.status = ReplStatus::Running;
        }
    }

    fn handle_repl_output_closed(&mut self, error: Option<String>) {
        if let Some(error) = error {
            self.repl.run.last_error = Some(format!("REPL output closed: {error}"));
        }
    }

    fn finish_repl(&mut self, exit: CommandExit) {
        self.repl.run.runtime = None;
        self.repl.run.exit_code = exit.code;
        self.repl.run.status = if exit.success || self.repl.run.status == ReplStatus::Stopping {
            ReplStatus::Stopped
        } else {
            ReplStatus::Failed
        };
        self.repl.run.screen.feed(
            format!(
                "\r\nREPL {}\r\n",
                if self.repl.run.status == ReplStatus::Stopped {
                    "stopped"
                } else {
                    "failed"
                }
            )
            .as_bytes(),
        );
        if matches!(self.mode, TuiMode::Repl | TuiMode::LeaderRepl) {
            self.mode = TuiMode::Normal;
        }
    }

    fn stop_repl(&mut self) {
        match self.repl.run.status {
            ReplStatus::Starting | ReplStatus::Running => {
                if let Some(runtime) = self.repl.run.runtime.as_mut()
                    && let Err(error) = runtime.send_ctrl_c()
                {
                    self.repl.run.last_error = Some(format!("failed to stop REPL: {error:#}"));
                }
                self.repl.run.status = ReplStatus::Stopping;
            }
            ReplStatus::Stopping => {
                if let Some(runtime) = self.repl.run.runtime.as_mut()
                    && let Err(error) = runtime.kill()
                {
                    self.repl.run.last_error =
                        Some(format!("failed to force kill REPL: {error:#}"));
                }
                self.repl.run.runtime = None;
                self.repl.run.status = ReplStatus::Stopped;
            }
            ReplStatus::Stopped | ReplStatus::Failed => {}
        }
    }

    fn cleanup_repl(&mut self) {
        if let Some(runtime) = self.repl.run.runtime.as_mut() {
            let _ = runtime.kill();
        }
    }

    fn restart_repl(&mut self, event_tx: Option<&TuiEventSender>) {
        if let Some(runtime) = self.repl.run.runtime.as_mut() {
            let _ = runtime.kill();
        }
        self.start_repl(event_tx);
    }

    fn resize_repl_for_terminal(&mut self, cols: u16, rows: u16) {
        let (screen_rows, screen_cols) = repl_screen_size_for_terminal(cols, rows);
        self.repl.run.screen.resize(screen_rows, screen_cols);
        if let Some(runtime) = self.repl.run.runtime.as_mut() {
            let _ = runtime.resize(screen_cols, screen_rows);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TuiMode {
    Normal,
    LeaderNormal,
    Palette,
    Help,
    AgentFilter,
    CommandInteraction,
    Repl,
    LeaderRepl,
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

struct ReplState {
    run: ReplRun,
}

struct AgentsState {
    view_mode: AgentsViewMode,
    mode: AgentModeFilter,
    query: String,
    selected: usize,
    detail_visible: bool,
    auto_refresh: bool,
    refresh_running: bool,
    refresh_generation: u64,
    refresh_context_id: Option<TuiContextId>,
    auto_refresh_stop: Option<Arc<AtomicBool>>,
    last_error: Option<String>,
    agents: Vec<AgentListItem>,
    inspect: AgentInspectState,
}

impl Default for AgentsState {
    fn default() -> Self {
        Self {
            view_mode: AgentsViewMode::List,
            mode: AgentModeFilter::Durable,
            query: String::new(),
            selected: 0,
            detail_visible: true,
            auto_refresh: false,
            refresh_running: false,
            refresh_generation: 0,
            refresh_context_id: None,
            auto_refresh_stop: None,
            last_error: None,
            agents: Vec::new(),
            inspect: AgentInspectState::default(),
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
enum AgentsViewMode {
    List,
    Inspect,
}

struct AgentInspectState {
    agent_name: Option<String>,
    focus: AgentInspectPane,
    oplog: InspectJob,
    stream: InspectJob,
}

impl Default for AgentInspectState {
    fn default() -> Self {
        Self {
            agent_name: None,
            focus: AgentInspectPane::Oplog,
            oplog: InspectJob::default(),
            stream: InspectJob::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentInspectPane {
    Oplog,
    Stream,
}

impl AgentInspectPane {
    fn label(self) -> &'static str {
        match self {
            Self::Oplog => "oplog",
            Self::Stream => "stream",
        }
    }
}

struct InspectJob {
    args: Vec<String>,
    status: InspectJobStatus,
    output: OutputBuffer,
    runtime: Option<NestedCliRuntime>,
    exit_code: Option<i32>,
}

impl Default for InspectJob {
    fn default() -> Self {
        Self {
            args: Vec::new(),
            status: InspectJobStatus::Idle,
            output: OutputBuffer::default(),
            runtime: None,
            exit_code: None,
        }
    }
}

impl InspectJob {
    fn start(args: Vec<String>) -> Self {
        Self {
            args,
            status: InspectJobStatus::Starting,
            output: OutputBuffer::default(),
            runtime: None,
            exit_code: None,
        }
    }

    fn is_running(&self) -> bool {
        matches!(
            self.status,
            InspectJobStatus::Starting | InspectJobStatus::Running | InspectJobStatus::Stopping
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InspectJobStatus {
    Idle,
    Starting,
    Running,
    Stopping,
    Stopped,
    Failed,
}

impl InspectJobStatus {
    fn title(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Stopping => "stopping",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
        }
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

    fn agent_list_mode(self) -> AgentListMode {
        match self {
            Self::Durable => AgentListMode::Durable,
            Self::Ephemeral => AgentListMode::Ephemeral,
            Self::All => AgentListMode::All,
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

impl Default for ReplState {
    fn default() -> Self {
        Self {
            run: ReplRun::default(),
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

struct ReplRun {
    status: ReplStatus,
    args: Vec<String>,
    screen: TerminalScreen,
    runtime: Option<NestedCliRuntime>,
    exit_code: Option<i32>,
    last_error: Option<String>,
}

impl Default for ReplRun {
    fn default() -> Self {
        Self {
            status: ReplStatus::Stopped,
            args: vec!["repl".to_string()],
            screen: TerminalScreen::new(20, 80),
            runtime: None,
            exit_code: None,
            last_error: None,
        }
    }
}

impl ReplRun {
    fn new(args: Vec<String>, rows: u16, cols: u16) -> Self {
        Self {
            status: ReplStatus::Starting,
            args,
            screen: TerminalScreen::new(rows, cols),
            runtime: None,
            exit_code: None,
            last_error: None,
        }
    }

    fn is_running(&self) -> bool {
        matches!(
            self.status,
            ReplStatus::Starting | ReplStatus::Running | ReplStatus::Stopping
        )
    }
}

impl ReplState {
    fn is_running(&self) -> bool {
        self.run.is_running()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReplStatus {
    Stopped,
    Starting,
    Running,
    Stopping,
    Failed,
}

impl ReplStatus {
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

fn agent_items_from_metadata_response(response: AgentsMetadataResponseView) -> Vec<AgentListItem> {
    response
        .agents
        .into_iter()
        .map(agent_item_from_metadata)
        .collect()
}

fn agent_item_from_metadata(agent: AgentMetadataView) -> AgentListItem {
    let name = agent.agent_name.0.clone();
    let component = Some(agent.component_name.0.clone());
    let agent_type = agent_type_from_agent_name(&name);
    let status = Some(format!("{:?}", agent.status));
    let raw = serde_json::to_value(agent).unwrap_or(Value::Null);

    AgentListItem {
        name,
        component,
        agent_type,
        status,
        raw,
    }
}

fn agent_type_from_agent_name(name: &str) -> Option<String> {
    name.split_once('(')
        .map(|(agent_type, _)| agent_type.trim())
        .filter(|agent_type| !agent_type.is_empty())
        .map(ToString::to_string)
}

fn agent_refresh_error(error: String, logs: Vec<String>) -> String {
    if logs.is_empty() {
        error
    } else {
        format!("{error}\n{}", logs.join("\n"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TuiView {
    Dashboard,
    Agents,
    Output,
    Server,
    Repl,
}

impl TuiView {
    const ALL: [Self; 5] = [
        Self::Dashboard,
        Self::Agents,
        Self::Output,
        Self::Server,
        Self::Repl,
    ];

    fn title(self) -> &'static str {
        match self {
            Self::Dashboard => "Dashboard",
            Self::Agents => "Agents",
            Self::Output => "Output",
            Self::Server => "Server",
            Self::Repl => "REPL",
        }
    }

    fn placeholder(self) -> &'static str {
        match self {
            Self::Dashboard => "Selected context and quick actions.",
            Self::Agents => "Agent monitoring and management will appear here.",
            Self::Output => "Nested command output will appear here.",
            Self::Server => "Local server logs will appear here.",
            Self::Repl => "Embedded REPL session will appear here.",
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

#[derive(Clone, Copy)]
struct TuiTheme {
    background: Color,
    surface: Color,
    panel: Color,
    panel_strong: Color,
    border_subtle: Color,
    border: Color,
    text: Color,
    text_secondary: Color,
    text_muted: Color,
    text_faint: Color,
    accent: Color,
    accent_hover: Color,
    marker: Color,
    success: Color,
    error: Color,
}

impl TuiTheme {
    fn golem_dark() -> Self {
        Self {
            background: Color::Rgb(10, 10, 13),
            surface: Color::Rgb(13, 13, 18),
            panel: Color::Rgb(20, 20, 27),
            panel_strong: Color::Rgb(26, 26, 34),
            border_subtle: Color::Rgb(42, 42, 53),
            border: Color::Rgb(58, 58, 72),
            text: Color::Rgb(237, 237, 240),
            text_secondary: Color::Rgb(168, 168, 180),
            text_muted: Color::Rgb(110, 110, 126),
            text_faint: Color::Rgb(74, 74, 85),
            accent: Color::Rgb(245, 176, 62),
            accent_hover: Color::Rgb(255, 197, 96),
            marker: Color::Rgb(224, 122, 61),
            success: Color::Rgb(134, 239, 172),
            error: Color::Rgb(224, 108, 117),
        }
    }
}

fn theme() -> TuiTheme {
    TuiTheme::golem_dark()
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

    render_tabs(frame, tabs, app);
    render_separator(frame, separator);

    if app.active_view == TuiView::Agents {
        render_agents_view(frame, body, app);
    } else if app.active_view == TuiView::Output {
        render_output_view(frame, body, app);
    } else if app.active_view == TuiView::Server {
        render_server_view(frame, body, app);
    } else if app.active_view == TuiView::Repl {
        render_repl_view(frame, body, app);
    } else {
        render_surface(frame, body);
        render_dashboard_logo(frame, body);
        let dashboard = Paragraph::new(view_lines(app, body.width as usize))
            .style(surface_style())
            .wrap(Wrap { trim: false });
        frame.render_widget(dashboard, body);
        render_left_rail(frame, body, surface_rail_style());
    }

    let footer = Paragraph::new(footer_line(app))
        .style(footer_style())
        .alignment(Alignment::Center);
    frame.render_widget(footer, footer_area);
    render_left_rail(frame, footer_area, footer_rail_style());

    match app.mode {
        TuiMode::Normal => {}
        TuiMode::LeaderNormal => render_leader_hint(frame, app, TuiMode::Normal),
        TuiMode::AgentFilter => {}
        TuiMode::CommandInteraction => {}
        TuiMode::Repl => {}
        TuiMode::LeaderRepl => render_leader_hint(frame, app, TuiMode::Repl),
        TuiMode::Palette => render_palette(frame, app),
        TuiMode::Help => render_help(frame, app),
    }
}

fn render_repl_view(frame: &mut Frame<'_>, area: Rect, app: &TuiApp) {
    render_surface(frame, area);
    let [summary_area, terminal_area] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .areas(area);

    frame.render_widget(
        Paragraph::new(repl_status_line(&app.repl, app.mode)).style(command_status_bg_style()),
        summary_area,
    );
    render_left_rail(frame, summary_area, command_rail_style());

    let lines = if app.repl.run.status == ReplStatus::Stopped && app.repl.run.exit_code.is_none() {
        vec![prefixed_line("Press r or enter to start `golem repl`.")]
    } else {
        repl_screen_lines(&app.repl.run, terminal_area.height as usize)
    };

    frame.render_widget(Paragraph::new(lines).style(surface_style()), terminal_area);
    render_left_rail(frame, terminal_area, surface_rail_style());

    if app.mode == TuiMode::Repl
        && let Some(cursor) = app.repl.run.screen.cursor_position(Position::new(
            terminal_area.x.saturating_add(2),
            terminal_area.y,
        ))
        && cursor.x < terminal_area.x.saturating_add(terminal_area.width)
        && cursor.y < terminal_area.y.saturating_add(terminal_area.height)
    {
        frame.set_cursor_position(cursor);
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

    if app.agents.view_mode == AgentsViewMode::Inspect {
        render_agent_inspect_view(frame, content_area, app);
        return;
    }

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

fn render_agent_inspect_view(frame: &mut Frame<'_>, area: Rect, app: &TuiApp) {
    render_surface(frame, area);
    let [oplog_area, stream_area] = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .areas(area);

    render_agent_inspect_pane(
        frame,
        oplog_area,
        "Oplog",
        AgentInspectPane::Oplog,
        &app.agents.inspect,
    );
    render_agent_inspect_pane(
        frame,
        stream_area,
        "Stream",
        AgentInspectPane::Stream,
        &app.agents.inspect,
    );
}

fn render_agent_inspect_pane(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &'static str,
    pane: AgentInspectPane,
    inspect: &AgentInspectState,
) {
    render_surface(frame, area);
    let [title_area, output_area] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .areas(area);
    let job = match pane {
        AgentInspectPane::Oplog => &inspect.oplog,
        AgentInspectPane::Stream => &inspect.stream,
    };
    let focused = inspect.focus == pane;
    frame.render_widget(
        Paragraph::new(agent_inspect_title_line(title, job, focused)).style(if focused {
            command_status_bg_style()
        } else {
            surface_style()
        }),
        title_area,
    );
    render_left_rail(
        frame,
        title_area,
        if focused {
            command_rail_style()
        } else {
            surface_rail_style()
        },
    );

    let output_lines = if job.output.total_lines() == 0 {
        vec![prefixed_line(match pane {
            AgentInspectPane::Oplog => "Oplog entries will appear here.",
            AgentInspectPane::Stream => "Waiting for agent stream...",
        })]
    } else {
        render_output_lines(&job.output, output_area.height as usize)
    };
    frame.render_widget(
        Paragraph::new(output_lines)
            .style(surface_style())
            .wrap(Wrap { trim: false }),
        output_area,
    );
    render_left_rail(frame, output_area, surface_rail_style());
    render_output_scrollbar(frame, output_area, &job.output);
}

fn agent_inspect_title_line(title: &'static str, job: &InspectJob, focused: bool) -> Line<'static> {
    let title_style = if focused {
        Style::default()
            .fg(theme().accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme().text_muted)
    };
    Line::from(vec![
        Span::styled(
            "┃ ",
            if focused {
                command_rail_style()
            } else {
                surface_rail_style()
            },
        ),
        fixed_span(title, 8, title_style),
        Span::raw(" "),
        fixed_span(job.status.title(), 10, inspect_job_status_style(job.status)),
        Span::raw(" | "),
        fixed_span(
            "left/right focus  esc back",
            30,
            Style::default().fg(theme().text_muted),
        ),
    ])
}

fn inspect_job_status_style(status: InspectJobStatus) -> Style {
    match status {
        InspectJobStatus::Idle | InspectJobStatus::Stopped => {
            Style::default().fg(theme().text_muted)
        }
        InspectJobStatus::Starting | InspectJobStatus::Running | InspectJobStatus::Stopping => {
            Style::default().fg(theme().success)
        }
        InspectJobStatus::Failed => Style::default().fg(theme().error),
    }
    .add_modifier(Modifier::BOLD)
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
                content_prefix(),
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
                    Span::styled("  ", surface_style()),
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
    render_left_rail(frame, area, surface_rail_style());
}

fn agent_status_line(agents: &AgentsState) -> Line<'static> {
    if agents.view_mode == AgentsViewMode::Inspect {
        return Line::from(vec![
            Span::styled("┃ ", command_rail_style()),
            fixed_span(
                "inspect",
                8,
                Style::default()
                    .fg(theme().accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            fixed_span(
                format!(
                    "agent:{}",
                    agents.inspect.agent_name.as_deref().unwrap_or("-")
                ),
                32,
                Style::default(),
            ),
            Span::raw(" | "),
            fixed_span(
                format!("focus:{}", agents.inspect.focus.label()),
                14,
                Style::default().fg(theme().accent),
            ),
            Span::raw(" | "),
            fixed_span(
                "left/right switch  esc back",
                30,
                Style::default().fg(theme().text_muted),
            ),
        ]);
    }

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
                .fg(theme().accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        fixed_span(
            format!("mode:{}", agents.mode.label()),
            14,
            Style::default().fg(theme().accent),
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
            Style::default().fg(theme().text_muted),
        ),
        Span::raw(" | "),
        flag_span("auto", agents.auto_refresh, false),
        Span::raw(" | "),
        fixed_span(
            if agents.refresh_running {
                "refreshing"
            } else {
                "enter inspect"
            },
            14,
            if agents.refresh_running {
                Style::default().fg(theme().accent)
            } else {
                Style::default().fg(theme().text_muted)
            },
        ),
    ])
}

fn render_header(frame: &mut Frame<'_>, area: Rect, app: &TuiApp) {
    let line = Line::from(vec![
        Span::styled(
            "┃ ",
            Style::default()
                .fg(theme().accent)
                .bg(theme().background)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            " Golem ",
            Style::default()
                .fg(theme().background)
                .bg(theme().accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" app: ", header_segment_label_style(0)),
        Span::styled(
            app.context.application.clone(),
            header_segment_value_style(0),
        ),
        Span::styled("  env: ", header_segment_label_style(1)),
        Span::styled(
            app.context.environment.clone(),
            header_segment_value_style(1).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  server: ", header_segment_label_style(2)),
        Span::styled(app.context.server.clone(), header_segment_value_style(2)),
    ]);
    frame.render_widget(Paragraph::new(line).style(header_style()), area);
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
    Style::default().fg(theme().background).bg(theme().accent)
}

fn header_segment_label_style(index: usize) -> Style {
    Style::default()
        .fg(Color::Rgb(87, 58, 0))
        .bg(header_segment_bg(index))
}

fn header_segment_value_style(index: usize) -> Style {
    Style::default()
        .fg(theme().background)
        .bg(header_segment_bg(index))
        .add_modifier(Modifier::BOLD)
}

fn header_segment_bg(_index: usize) -> Color {
    theme().accent
}

fn tabs_style() -> Style {
    Style::default()
        .fg(theme().text_secondary)
        .bg(theme().panel)
}

fn tabs_rail_style() -> Style {
    Style::default()
        .fg(theme().accent)
        .bg(theme().panel)
        .add_modifier(Modifier::BOLD)
}

fn separator_style() -> Style {
    Style::default().bg(theme().border_subtle)
}

fn separator_rail_style() -> Style {
    Style::default()
        .fg(theme().accent)
        .bg(theme().border_subtle)
}

fn surface_style() -> Style {
    Style::default().fg(theme().text).bg(theme().surface)
}

fn surface_rail_style() -> Style {
    Style::default()
        .fg(theme().border_subtle)
        .bg(theme().surface)
}

fn command_status_bg_style() -> Style {
    Style::default().fg(theme().text).bg(theme().panel_strong)
}

fn command_rail_style() -> Style {
    Style::default()
        .fg(theme().accent)
        .bg(theme().panel_strong)
        .add_modifier(Modifier::BOLD)
}

fn footer_style() -> Style {
    Style::default().fg(theme().text_muted).bg(theme().panel)
}

fn footer_rail_style() -> Style {
    Style::default().fg(theme().text_faint).bg(theme().panel)
}

fn prefixed_line(text: impl Into<String>) -> Line<'static> {
    Line::from(vec![content_prefix(), Span::raw(text.into())])
}

fn content_prefix() -> Span<'static> {
    Span::styled("  ", surface_style())
}

fn render_help(frame: &mut Frame<'_>, app: &TuiApp) {
    let area = centered_rect(70, 85, frame.area());
    let lines = help_lines(app);

    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme().border))
                .title(" Help "),
        ),
        area,
    );
}

fn help_lines(app: &TuiApp) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(vec![Span::styled(
            "Keyboard Shortcuts",
            Style::default().add_modifier(Modifier::BOLD),
        )]),
        Line::default(),
    ];

    push_action_section_for_app(
        &mut lines,
        app,
        "Global",
        &[
            TuiActionId::OpenPalette,
            TuiActionId::ShowHelp,
            TuiActionId::Build,
            TuiActionId::Deploy,
            TuiActionId::Clean,
            TuiActionId::StartOrFocusRepl,
            TuiActionId::Quit,
        ],
    );
    push_raw_help(&mut lines, "ctrl+p / :", "Open Palette");
    push_raw_help(&mut lines, "esc / ctrl+c", "Quit");
    push_raw_help(&mut lines, "] / tab", "Next view");
    push_raw_help(&mut lines, "[ / shift+tab", "Previous view");
    push_raw_help(&mut lines, "1..5", "Jump to view");

    push_section_break(&mut lines, "Palette");
    push_raw_help(&mut lines, "type", "Filter commands");
    push_raw_help(&mut lines, "up / down", "Move selection");
    push_raw_help(&mut lines, "enter", "Execute selected action");
    push_raw_help(&mut lines, "esc / ctrl+c", "Close palette");

    push_action_section_for_app(
        &mut lines,
        app,
        "Leader",
        &[
            TuiActionId::ToggleYes,
            TuiActionId::ToggleReset,
            TuiActionId::ToggleServerClean,
            TuiActionId::RestartServer,
            TuiActionId::CleanRestartServer,
        ],
    );

    if app.active_view == TuiView::Agents {
        if app.agents.view_mode == AgentsViewMode::Inspect {
            push_section_break(&mut lines, "Agent Inspect");
            push_raw_help(&mut lines, "left / right", "Switch pane focus");
            push_raw_help(&mut lines, "up / down", "Scroll focused pane");
            push_raw_help(&mut lines, "pageup / pagedown", "Scroll focused pane");
            push_raw_help(&mut lines, "home / end", "Top / latest focused pane");
            push_raw_help(&mut lines, "esc", "Return to agent list");
        } else {
            push_action_section_for_app(
                &mut lines,
                app,
                "Agents",
                &[
                    TuiActionId::RefreshAgents,
                    TuiActionId::ToggleAgentAutoRefresh,
                    TuiActionId::ToggleAgentDetails,
                    TuiActionId::CycleAgentMode,
                ],
            );
            push_raw_help(&mut lines, "/", "Filter agents");
            push_raw_help(&mut lines, "up / down", "Move selection");
            push_raw_help(&mut lines, "enter", "Inspect selected agent");
        }
    }

    if app.command_is_running() {
        push_section_break(&mut lines, "Command");
        push_raw_help(&mut lines, "type", "Send input to command");
        push_raw_help(
            &mut lines,
            "esc / ctrl+c",
            "Cancel command, press again to force kill",
        );
        push_raw_help(&mut lines, "pageup / pagedown", "Scroll output");
        push_raw_help(&mut lines, "home / end", "Top / latest output");
        push_raw_help(&mut lines, "mouse wheel", "Scroll output");
    } else if app.active_view == TuiView::Output {
        push_section_break(&mut lines, "Output");
        push_raw_help(&mut lines, "up / down", "Scroll output");
        push_raw_help(&mut lines, "pageup / pagedown", "Scroll output");
        push_raw_help(&mut lines, "home / end", "Top / latest output");
        push_raw_help(&mut lines, "mouse wheel", "Scroll output");
    }

    if app.active_view == TuiView::Repl || app.repl.is_running() {
        push_action_section_for_app(
            &mut lines,
            app,
            "REPL",
            &[
                TuiActionId::StartOrFocusRepl,
                TuiActionId::LeaveRepl,
                TuiActionId::StopRepl,
                TuiActionId::RestartRepl,
            ],
        );
        push_raw_help(&mut lines, "enter", "Start or focus REPL");
        push_raw_help(&mut lines, "ctrl+x p / ?", "Palette / help");
    }

    lines.push(Line::default());
    lines.push(Line::from("Press esc to close this help."));
    lines
}

fn push_action_section_for_app(
    lines: &mut Vec<Line<'static>>,
    app: &TuiApp,
    title: &'static str,
    ids: &[TuiActionId],
) {
    push_section_break(lines, title);
    for id in ids {
        push_action_help_for_app(lines, app, action(*id));
    }
}

fn push_section_break(lines: &mut Vec<Line<'static>>, title: &'static str) {
    if lines.last().is_some_and(|line| !line.spans.is_empty()) {
        lines.push(Line::default());
    }
    lines.push(Line::from(title));
}

fn push_action_help_for_app(lines: &mut Vec<Line<'static>>, app: &TuiApp, action: &TuiAction) {
    let shortcut = action.shortcut.unwrap_or("-");
    let suffix = match app.action_availability(action) {
        TuiActionAvailability::Available => String::new(),
        TuiActionAvailability::Unavailable(reason) => format!(" ({reason})"),
    };
    lines.push(Line::from(format!(
        "  {shortcut:<18} {}{suffix}",
        action.label
    )));
}

fn push_raw_help(lines: &mut Vec<Line<'static>>, shortcut: &'static str, label: &'static str) {
    lines.push(Line::from(format!("  {shortcut:<18} {label}")));
}

fn render_tabs(frame: &mut Frame<'_>, area: Rect, app: &TuiApp) {
    let mut spans = vec![Span::styled("┃ ", tabs_rail_style())];

    for (index, view) in TuiView::ALL.iter().copied().enumerate() {
        if spans.len() > 1 {
            spans.push(Span::raw("  "));
        }

        let tab_style = if view == app.active_view {
            Style::default()
                .fg(theme().accent_hover)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            Style::default().fg(theme().text_muted)
        };
        spans.push(shortcut_text_span(format!("[{}]", index + 1)));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(view.title(), tab_style));
        if let Some(status) = tab_status(view, app) {
            spans.push(Span::raw(" "));
            spans.push(status);
        }
    }

    frame.render_widget(Paragraph::new(Line::from(spans)).style(tabs_style()), area);
    render_left_rail(frame, area, tabs_rail_style());
}

fn tab_status(view: TuiView, app: &TuiApp) -> Option<Span<'static>> {
    let running = match view {
        TuiView::Output => app.command_is_running(),
        TuiView::Server => app.server.run.is_running(),
        TuiView::Repl => app.repl.is_running(),
        _ => return None,
    };
    let (label, style) = if running {
        (
            "●",
            Style::default()
                .fg(theme().success)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        ("○", Style::default().fg(theme().text_faint))
    };
    Some(Span::styled(label, style))
}

fn render_leader_hint(frame: &mut Frame<'_>, app: &TuiApp, return_mode: TuiMode) {
    let area = Rect {
        x: frame.area().x,
        y: frame.area().y + frame.area().height.saturating_sub(2),
        width: frame.area().width,
        height: 1,
    };
    let line = if return_mode == TuiMode::Repl {
        leader_line(&[
            (TuiActionId::LeaveRepl, "leave"),
            (TuiActionId::StopRepl, "stop"),
            (TuiActionId::RestartRepl, "restart"),
            (TuiActionId::OpenPalette, "palette"),
            (TuiActionId::ShowHelp, "help"),
        ])
    } else {
        let mut spans = vec![Span::styled("┃ ", command_rail_style())];
        push_leader_item(
            &mut spans,
            TuiActionId::ToggleYes,
            format!("yes:{}", flag_state(app.command_options.yes)),
        );
        push_leader_item(
            &mut spans,
            TuiActionId::ToggleReset,
            format!("reset:{}", flag_state(app.command_options.reset)),
        );
        push_leader_item(
            &mut spans,
            TuiActionId::ToggleServerClean,
            format!("clean:{}", flag_state(app.server.clean)),
        );
        push_leader_item(&mut spans, TuiActionId::OpenPalette, "palette");
        push_leader_item(&mut spans, TuiActionId::ShowHelp, "help");
        Line::from(spans)
    };
    frame.render_widget(Paragraph::new(line).style(command_status_bg_style()), area);
    render_left_rail(frame, area, command_rail_style());
}

fn leader_line(items: &[(TuiActionId, &'static str)]) -> Line<'static> {
    let mut spans = vec![Span::styled("┃ ", command_rail_style())];
    for (id, label) in items {
        push_leader_item(&mut spans, *id, *label);
    }
    Line::from(spans)
}

fn push_leader_item(spans: &mut Vec<Span<'static>>, id: TuiActionId, label: impl Into<String>) {
    if spans.len() > 1 {
        spans.push(Span::raw("  "));
    }
    spans.push(shortcut_span(action_shortcut(id)));
    spans.push(Span::raw(" "));
    spans.push(Span::raw(label.into()));
}

fn render_dashboard_logo(frame: &mut Frame<'_>, area: Rect) {
    const LOGO: [&str; 11] = [
        "⠀⠀⠀⠀⠀⢀⣤⣦⡀⣼⣿⣿⣷⣤⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀",
        "⠀⠀⠀⣀⢸⣿⣿⣿⣿⣿⣿⣿⣿⣿⣷⡄⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀",
        "⠀⢀⣴⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⢷⣦⡀⠀⠀⠀⠀⠀⠀⢀⣠⣤⣤⣤⣤⣤⣤⡄⠀⠀⠀⣀⣤⣤⣤⣤⣀⠀⠀⠀⠀⣤⡄⠀⠀⠀⠀⠀⠀⠀⢠⣤⣤⣤⣤⣤⣤⣤⣤⣤⠀⢠⡀⠀⠀⠀⠀⠀⠀⠀⠀⣠",
        "⢠⣿⣿⡾⣿⣿⡏⠹⣿⣿⣿⡿⠙⣿⣿⣿⣷⣿⣷⡄⠀⠀⠀⢀⣴⣿⠿⠟⠛⠛⠛⠛⠛⠃⠀⢀⣾⣿⠿⠛⠛⠿⣿⣷⡄⠀⠀⣿⡇⠀⠀⠀⠀⠀⠀⠀⢸⣿⡟⠛⠛⠛⠛⠛⠛⠛⠀⢸⣿⣶⣄⠀⠀⠀⢀⣠⣾⣿",
        "⢶⣿⣿⣇⣋⢿⣷⣶⣾⣿⣿⣶⣶⠿⣿⣬⣿⣿⣿⡶⠀⠀⠀⣾⡿⠁⠀⠀⠀⠀⠀⠀⠀⠀⢠⣿⡿⠁⠀⠀⠀⠀⠈⢻⣿⡄⠀⣿⡇⠀⠀⠀⠀⠀⠀⠀⢸⣿⣇⣀⣀⣀⣀⣀⠀⠀⠀⢸⣿⡿⣿⣷⣤⣴⣿⡿⢻⣿",
        "⣶⣿⣿⣿⣿⣷⢽⣻⢿⣿⣿⣛⢋⡼⣾⣿⣿⣿⣿⣶⠀⠀⢸⣿⡇⠀⠀⠀⣶⣶⣶⣶⣶⡆⢸⣿⡇⠀⠀⠀⠀⠀⠀⢸⣿⡇⠀⣿⡇⠀⠀⠀⠀⠀⠀⠀⢸⣿⡿⠿⠿⠿⠿⠿⠀⠀⠀⢸⣿⡇⠀⠙⢿⠟⠉⠀⢸⣿",
        "⣿⣿⠟⠊⠉⠁⠀⢻⣿⣿⣿⣿⠏⠀⠈⠉⠙⢻⣿⡏⠀⠀⠀⢿⣷⡀⠀⠀⠉⠉⠉⢹⣿⡇⠘⣿⣷⡀⠀⠀⠀⠀⠀⣼⣿⠃⠀⣿⡇⠀⠀⠀⠀⠀⠀⠀⢸⣿⡇⠀⠀⠀⠀⠀⠀⠀⠀⢸⣿⡇⠀⠀⠀⠀⠀⠀⢸⣿",
        "⠻⣿⣿⣿⣅⢀⣴⣽⣿⣿⣿⣿⣯⣦⡀⣸⣿⣿⣿⠃⠀⠀⠀⠈⠻⣿⣶⣤⣤⣤⣤⣼⣿⡇⠀⠘⢿⣿⣦⣤⣤⣴⣾⡿⠋⠀⠀⣿⣧⣤⣤⣤⣤⣤⣤⡄⢸⣿⣧⣤⣤⣤⣤⣤⣤⣤⠀⢸⣿⡇⠀⠀⠀⠀⠀⠀⢸⣿",
        "⠀⠈⠙⠛⠃⣼⣾⣾⣿⠟⠻⣿⣷⣿⣇⠙⠛⠋⠁⠀⠀⠀⠀⠀⠀⠈⠙⠛⠛⠛⠛⠛⠛⠃⠀⠀⠀⠉⠛⠛⠟⠛⠉⠀⠀⠀⠀⠛⠛⠛⠛⠛⠛⠛⠛⠃⠘⠛⠛⠛⠛⠛⠛⠛⠛⠛⠀⠘⠛⠃⠀⠀⠀⠀⠀⠀⠘⠛",
        "⠀⠀⠀⢀⣶⡾⣿⣿⣿⠀⢀⣿⣿⣿⢷⣢⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀",
        "⠀⠀⠀⠺⢿⣷⣿⡿⠿⠂⠘⠿⢿⣿⣽⡿⠗⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀",
    ];

    let logo_width = LOGO
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0) as u16;
    let logo_height = LOGO.len() as u16;
    if area.width <= logo_width.saturating_add(4) || area.height <= logo_height.saturating_add(2) {
        return;
    }

    let x = area.x + area.width.saturating_sub(logo_width) / 2;
    let y = area.y + area.height.saturating_sub(logo_height) / 2;
    let style = Style::default()
        .fg(Color::Rgb(34, 34, 42))
        .bg(theme().surface);

    for (row, line) in LOGO.iter().enumerate() {
        for (col, character) in line.chars().enumerate() {
            if character == ' ' {
                continue;
            }
            let x = x.saturating_add(col as u16);
            let y = y.saturating_add(row as u16);
            if x < area.x.saturating_add(area.width) && y < area.y.saturating_add(area.height) {
                frame.buffer_mut()[(x, y)]
                    .set_symbol(character.to_string().as_str())
                    .set_style(style);
            }
        }
    }
}

fn view_lines(app: &TuiApp, width: usize) -> Vec<Line<'static>> {
    let mut lines = vec![
        dashboard_line(
            vec![Span::styled(
                app.active_view.title(),
                Style::default().add_modifier(Modifier::BOLD),
            )],
            width,
        ),
        Line::default(),
        dashboard_line(vec![Span::raw(app.active_view.placeholder())], width),
    ];

    if app.active_view == TuiView::Dashboard {
        lines.extend([
            Line::default(),
            dashboard_line(
                vec![Span::raw(format!(
                    "Application : {}",
                    app.context.application
                ))],
                width,
            ),
            dashboard_line(
                vec![Span::raw(format!(
                    "Environment : {}",
                    app.context.environment
                ))],
                width,
            ),
            dashboard_line(
                vec![Span::raw(format!("Server      : {}", app.context.server))],
                width,
            ),
            dashboard_line(
                vec![Span::raw(format!(
                    "Config dir  : {}",
                    app.context.config_dir
                ))],
                width,
            ),
            Line::default(),
            dashboard_line(
                vec![Span::raw(
                    "Scaffold ready. Next steps: command execution and live data.",
                )],
                width,
            ),
        ]);
    }

    lines
}

fn dashboard_line(mut spans: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let content_width = spans
        .iter()
        .map(|span| span.content.chars().count())
        .sum::<usize>();
    let mut line_spans = vec![content_prefix()];
    line_spans.append(&mut spans);
    let used_width = 2 + content_width;
    if width > used_width {
        line_spans.push(Span::styled(
            " ".repeat(width - used_width),
            surface_style(),
        ));
    }
    Line::from(line_spans)
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
            fixed_span("none", 7, Style::default().fg(theme().text_muted)),
            Span::raw(" "),
            fixed_span("idle", 10, Style::default().fg(theme().text_muted)),
            Span::raw(" | "),
            flag_span("yes", app.command_options.yes, false),
            Span::raw(" "),
            flag_span("reset", app.command_options.reset, true),
            Span::raw(" | "),
            fixed_span("b build / d deploy / c clean", 30, Style::default()),
            Span::raw(" | "),
            fixed_span("ready", 17, Style::default().fg(theme().text_muted)),
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

fn footer_line(app: &TuiApp) -> Line<'static> {
    let mut spans = Vec::new();
    push_footer_action(&mut spans, TuiActionId::Build, 7);
    push_footer_action(&mut spans, TuiActionId::Deploy, 7);
    push_footer_action(&mut spans, TuiActionId::Clean, 7);
    push_footer_action(&mut spans, TuiActionId::StartOrFocusRepl, 4);
    push_footer_shortcut(&mut spans, TuiActionId::ToggleYes);
    spans.push(Span::raw(" "));
    spans.push(flag_span("yes", app.command_options.yes, false));
    spans.push(Span::raw(" "));
    push_footer_shortcut(&mut spans, TuiActionId::ToggleReset);
    spans.push(Span::raw(" "));
    spans.push(flag_span("reset", app.command_options.reset, true));
    spans.push(Span::raw("  "));
    push_footer_shortcut(&mut spans, TuiActionId::OpenPalette);
    spans.push(Span::raw(" Palette  "));
    push_footer_shortcut(&mut spans, TuiActionId::ShowHelp);
    spans.push(Span::raw(" Help"));
    Line::from(spans)
}

fn push_footer_action(spans: &mut Vec<Span<'static>>, id: TuiActionId, width: usize) {
    if !spans.is_empty() {
        spans.push(Span::raw(" "));
    }
    push_footer_shortcut(spans, id);
    spans.push(Span::raw(" "));
    spans.push(fixed_span(
        action_short_label(id),
        width,
        Style::default().fg(theme().text_muted),
    ));
}

fn push_footer_shortcut(spans: &mut Vec<Span<'static>>, id: TuiActionId) {
    spans.push(shortcut_span(action_shortcut(id)));
}

fn command_status_line(run: &CommandRun) -> Line<'static> {
    Line::from(vec![
        Span::styled("┃ ", command_rail_style()),
        fixed_span(
            run.kind.title(),
            7,
            Style::default()
                .fg(theme().accent)
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
            fixed_span("esc/ctrl+c cancel", 17, Style::default().fg(theme().accent))
        } else {
            fixed_span(
                "b/d/c run again",
                17,
                Style::default().fg(theme().text_muted),
            )
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
                .fg(theme().accent)
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
        fixed_span(
            server_hint(run),
            21,
            Style::default().fg(theme().text_muted),
        ),
    ])
}

fn repl_status_line(repl: &ReplState, mode: TuiMode) -> Line<'static> {
    let run = &repl.run;
    Line::from(vec![
        Span::styled("┃ ", command_rail_style()),
        fixed_span(
            "repl",
            7,
            Style::default()
                .fg(theme().accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        fixed_span(run.status.title(), 10, repl_status_style(run.status)),
        Span::raw(" | "),
        fixed_span(command_display(&run.args), 28, Style::default()),
        Span::raw(" | "),
        fixed_span(
            repl_mode_hint(run, mode),
            36,
            Style::default().fg(theme().text_muted),
        ),
    ])
}

fn repl_status_style(status: ReplStatus) -> Style {
    match status {
        ReplStatus::Starting | ReplStatus::Running | ReplStatus::Stopping => {
            Style::default().fg(theme().accent)
        }
        ReplStatus::Stopped => Style::default().fg(theme().text_muted),
        ReplStatus::Failed => Style::default().fg(theme().error),
    }
    .add_modifier(Modifier::BOLD)
}

fn repl_mode_hint(run: &ReplRun, mode: TuiMode) -> &'static str {
    match mode {
        TuiMode::Repl => "ctrl+x q leave  ctrl+x k stop",
        TuiMode::LeaderRepl => "q leave  k stop  shift+r restart",
        _ if run.is_running() => "r focus  ctrl+x k stop",
        _ => "r/enter start",
    }
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
            Style::default().fg(theme().accent)
        }
        ServerStatus::Stopped => Style::default().fg(theme().text_muted),
        ServerStatus::Failed => Style::default().fg(theme().error),
    }
    .add_modifier(Modifier::BOLD)
}

fn server_hint(run: &ServerRun) -> &'static str {
    match run.status {
        ServerStatus::Starting | ServerStatus::Running => "s/ctrl+c stop",
        ServerStatus::Stopping => "s/ctrl+c force kill",
        ServerStatus::Stopped | ServerStatus::Failed => "s start  ctrl+x shift+r",
    }
}

fn command_status_style(status: CommandStatus) -> Style {
    match status {
        CommandStatus::Running | CommandStatus::Cancelling => Style::default().fg(theme().accent),
        CommandStatus::Succeeded => Style::default().fg(theme().success),
        CommandStatus::Failed | CommandStatus::Killed => Style::default().fg(theme().error),
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
                theme().marker
            } else {
                theme().success
            })
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme().text_muted)
    };

    let width = match name {
        "yes" => 7,
        "reset" => 9,
        _ => name.len() + 4,
    };

    fixed_span(format!("{name}:{}", flag_state(enabled)), width, style)
}

fn shortcut_span(key: &'static str) -> Span<'static> {
    shortcut_text_span(key.to_string())
}

fn shortcut_text_span(key: String) -> Span<'static> {
    Span::styled(
        key,
        Style::default()
            .fg(theme().accent_hover)
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

fn repl_screen_lines(run: &ReplRun, height: usize) -> Vec<Line<'static>> {
    let mut lines = run
        .screen
        .lines()
        .into_iter()
        .take(height)
        .map(output_line_with_rail)
        .collect::<Vec<_>>();

    if let Some(error) = &run.last_error
        && !lines.is_empty()
    {
        lines[0] = Line::from(vec![
            content_prefix(),
            Span::styled(error.clone(), Style::default().fg(theme().error)),
        ]);
    }

    lines
}

fn repl_screen_size_for_terminal(cols: u16, rows: u16) -> (u16, u16) {
    let screen_rows = rows.saturating_sub(5).max(1);
    let screen_cols = cols.saturating_sub(2).max(1);
    (screen_rows, screen_cols)
}

fn output_line_with_rail(line: Line<'static>) -> Line<'static> {
    let mut spans = vec![content_prefix()];
    spans.extend(line.spans.into_iter().map(span_with_surface_bg));
    Line::from(spans)
}

fn span_with_surface_bg(mut span: Span<'static>) -> Span<'static> {
    span.style = span.style.bg(theme().surface);
    span
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
    let width_actions = palette_actions();
    let label_width = palette_label_width(&width_actions);
    let reason_width = palette_reason_width(app, &width_actions);
    let area = centered_rect_fixed(
        palette_width(
            &app.palette.query,
            &width_actions,
            label_width,
            reason_width,
            frame.area().width,
        ),
        palette_height(width_actions.len().min(8), frame.area().height),
        frame.area(),
    );
    let selected = app.palette.selected.min(actions.len().saturating_sub(1));
    let mut lines = vec![
        palette_line(vec![Span::styled(
            "Command Palette",
            Style::default().add_modifier(Modifier::BOLD),
        )]),
        palette_line(vec![Span::raw(format!("> {}", app.palette.query))]),
        palette_line(vec![]),
    ];

    if actions.is_empty() {
        lines.push(palette_line(vec![Span::raw("No matching commands")]));
    } else {
        for (index, action) in visible_actions.iter().enumerate() {
            let prefix = if index == selected { "> " } else { "  " };
            let plain_label = palette_action_label(action, prefix);
            let availability = app.action_availability(action);
            let style = match (index == selected, availability.is_unavailable()) {
                (true, true) => Style::default()
                    .fg(theme().text_muted)
                    .add_modifier(Modifier::REVERSED),
                (true, false) => Style::default().add_modifier(Modifier::REVERSED),
                (false, true) => Style::default().fg(theme().text_muted),
                (false, false) => Style::default(),
            };
            let mut label_spans = vec![Span::styled(format!("{prefix}{}", action.label), style)];
            if let Some(shortcut) = action.shortcut {
                label_spans.push(Span::raw(" ("));
                label_spans.push(shortcut_span(shortcut));
                label_spans.push(Span::raw(")"));
            }
            let padding = label_width.saturating_sub(plain_label.chars().count());
            let mut line_spans = label_spans;
            line_spans.push(Span::raw(" ".repeat(padding)));
            line_spans.push(Span::raw("  "));
            line_spans.push(Span::styled(action.description, style));
            if let TuiActionAvailability::Unavailable(reason) = availability {
                line_spans.push(Span::styled(format!(" - {reason}"), style));
            }
            lines.push(Line::from(line_spans));
        }
    }

    frame.render_widget(Clear, area);
    let content_area = Rect {
        x: area.x.saturating_add(2),
        y: area.y,
        width: area.width.saturating_sub(2),
        height: area.height,
    };
    frame.render_widget(Paragraph::new(lines), content_area);
    render_left_rail(frame, area, Style::default().fg(theme().accent));
}

fn palette_line(mut spans: Vec<Span<'static>>) -> Line<'static> {
    Line::from(std::mem::take(&mut spans))
}

fn palette_label_width(actions: &[TuiAction]) -> usize {
    actions
        .iter()
        .map(|action| palette_action_label(action, "  ").chars().count())
        .max()
        .unwrap_or("No matching commands".len())
        .max(18)
}

fn palette_reason_width(app: &TuiApp, actions: &[TuiAction]) -> usize {
    actions
        .iter()
        .filter_map(|action| match app.action_availability(action) {
            TuiActionAvailability::Available => None,
            TuiActionAvailability::Unavailable(reason) => Some(3 + reason.chars().count()),
        })
        .max()
        .unwrap_or(0)
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
    reason_width: usize,
    terminal_width: u16,
) -> u16 {
    let action_width = actions
        .iter()
        .map(|action| label_width + 2 + action.description.chars().count() + reason_width)
        .max()
        .unwrap_or("No matching commands".len());
    let content_width = action_width
        .max("Command Palette".len())
        .max(query.chars().count() + 2)
        + 2;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum TuiActionId {
    Build,
    Deploy,
    Clean,
    ToggleYes,
    ToggleReset,
    ToggleServer,
    RestartServer,
    CleanRestartServer,
    ToggleServerClean,
    RefreshAgents,
    ToggleAgentAutoRefresh,
    CycleAgentMode,
    ToggleAgentDetails,
    SelectDashboard,
    SelectAgents,
    SelectOutput,
    SelectServer,
    SelectRepl,
    StartOrFocusRepl,
    FocusRepl,
    LeaveRepl,
    StopRepl,
    RestartRepl,
    OpenPalette,
    ShowHelp,
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TuiActionCategory {
    Navigation,
    Dev,
    Ops,
    Settings,
    Repl,
    System,
}

impl TuiActionCategory {
    fn label(self) -> &'static str {
        match self {
            Self::Navigation => "navigation",
            Self::Dev => "dev",
            Self::Ops => "ops",
            Self::Settings => "settings",
            Self::Repl => "repl",
            Self::System => "system",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TuiActionScope {
    Global,
    Leader,
    Agents,
    Repl,
    ReplLeader,
    System,
}

impl TuiActionScope {
    fn label(self) -> &'static str {
        match self {
            Self::Global => "Global",
            Self::Leader => "Leader",
            Self::Agents => "Agents",
            Self::Repl => "REPL",
            Self::ReplLeader => "REPL Leader",
            Self::System => "System",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TuiActionExecutionKind {
    Internal,
    Direct,
    NestedCli,
    ViewNavigation,
}

impl TuiActionExecutionKind {
    fn label(self) -> &'static str {
        match self {
            Self::Internal => "internal",
            Self::Direct => "direct",
            Self::NestedCli => "nested-cli",
            Self::ViewNavigation => "navigation",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct TuiAction {
    id: TuiActionId,
    label: &'static str,
    description: &'static str,
    shortcut: Option<&'static str>,
    category: TuiActionCategory,
    scope: TuiActionScope,
    execution_kind: TuiActionExecutionKind,
    palette_visible: bool,
    kind: TuiActionKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TuiActionAvailability {
    Available,
    Unavailable(&'static str),
}

impl TuiActionAvailability {
    fn is_unavailable(self) -> bool {
        matches!(self, Self::Unavailable(_))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TuiActionKind {
    SelectView(TuiView),
    Build,
    Deploy,
    Clean,
    ToggleYes,
    ToggleReset,
    ToggleServer,
    RestartServer,
    CleanRestartServer,
    ToggleServerClean,
    RefreshAgents,
    ToggleAgentAutoRefresh,
    CycleAgentMode,
    ToggleAgentDetails,
    StartOrFocusRepl,
    FocusRepl,
    LeaveRepl,
    StopRepl,
    RestartRepl,
    OpenPalette,
    ShowHelp,
    Quit,
}

const ACTIONS: [TuiAction; 26] = [
    TuiAction {
        id: TuiActionId::Build,
        label: "Build",
        description: "Run golem build",
        shortcut: Some("b"),
        category: TuiActionCategory::Dev,
        scope: TuiActionScope::Global,
        execution_kind: TuiActionExecutionKind::NestedCli,
        palette_visible: true,
        kind: TuiActionKind::Build,
    },
    TuiAction {
        id: TuiActionId::Deploy,
        label: "Deploy",
        description: "Run golem deploy",
        shortcut: Some("d"),
        category: TuiActionCategory::Dev,
        scope: TuiActionScope::Global,
        execution_kind: TuiActionExecutionKind::NestedCli,
        palette_visible: true,
        kind: TuiActionKind::Deploy,
    },
    TuiAction {
        id: TuiActionId::Clean,
        label: "Clean",
        description: "Run golem clean",
        shortcut: Some("c"),
        category: TuiActionCategory::Dev,
        scope: TuiActionScope::Global,
        execution_kind: TuiActionExecutionKind::NestedCli,
        palette_visible: true,
        kind: TuiActionKind::Clean,
    },
    TuiAction {
        id: TuiActionId::ToggleYes,
        label: "Toggle Yes",
        description: "Toggle --yes for build/deploy",
        shortcut: Some("ctrl+x y"),
        category: TuiActionCategory::Settings,
        scope: TuiActionScope::Leader,
        execution_kind: TuiActionExecutionKind::Internal,
        palette_visible: true,
        kind: TuiActionKind::ToggleYes,
    },
    TuiAction {
        id: TuiActionId::ToggleReset,
        label: "Toggle Reset",
        description: "Toggle --reset for deploy",
        shortcut: Some("ctrl+x r"),
        category: TuiActionCategory::Settings,
        scope: TuiActionScope::Leader,
        execution_kind: TuiActionExecutionKind::Internal,
        palette_visible: true,
        kind: TuiActionKind::ToggleReset,
    },
    TuiAction {
        id: TuiActionId::ToggleServer,
        label: "Start/Stop Server",
        description: "Switch to Server and toggle it",
        shortcut: Some("s"),
        category: TuiActionCategory::Dev,
        scope: TuiActionScope::Global,
        execution_kind: TuiActionExecutionKind::NestedCli,
        palette_visible: true,
        kind: TuiActionKind::ToggleServer,
    },
    TuiAction {
        id: TuiActionId::RestartServer,
        label: "Restart Server",
        description: "Restart the local server",
        shortcut: Some("ctrl+x shift+r"),
        category: TuiActionCategory::Dev,
        scope: TuiActionScope::Leader,
        execution_kind: TuiActionExecutionKind::NestedCli,
        palette_visible: true,
        kind: TuiActionKind::RestartServer,
    },
    TuiAction {
        id: TuiActionId::CleanRestartServer,
        label: "Clean Restart Server",
        description: "Restart local server with --clean",
        shortcut: Some("ctrl+x shift+c"),
        category: TuiActionCategory::Dev,
        scope: TuiActionScope::Leader,
        execution_kind: TuiActionExecutionKind::NestedCli,
        palette_visible: true,
        kind: TuiActionKind::CleanRestartServer,
    },
    TuiAction {
        id: TuiActionId::ToggleServerClean,
        label: "Toggle Server Clean",
        description: "Toggle --clean for next server start",
        shortcut: Some("ctrl+x s"),
        category: TuiActionCategory::Settings,
        scope: TuiActionScope::Leader,
        execution_kind: TuiActionExecutionKind::Internal,
        palette_visible: true,
        kind: TuiActionKind::ToggleServerClean,
    },
    TuiAction {
        id: TuiActionId::RefreshAgents,
        label: "Refresh Agents",
        description: "Refresh the agent list",
        shortcut: Some("u"),
        category: TuiActionCategory::Ops,
        scope: TuiActionScope::Agents,
        execution_kind: TuiActionExecutionKind::Direct,
        palette_visible: true,
        kind: TuiActionKind::RefreshAgents,
    },
    TuiAction {
        id: TuiActionId::ToggleAgentAutoRefresh,
        label: "Toggle Agent Auto Refresh",
        description: "Toggle automatic agent refresh",
        shortcut: Some("ctrl+x a"),
        category: TuiActionCategory::Ops,
        scope: TuiActionScope::Agents,
        execution_kind: TuiActionExecutionKind::Internal,
        palette_visible: true,
        kind: TuiActionKind::ToggleAgentAutoRefresh,
    },
    TuiAction {
        id: TuiActionId::CycleAgentMode,
        label: "Cycle Agent Mode",
        description: "Cycle durable, ephemeral, all",
        shortcut: Some("ctrl+x m"),
        category: TuiActionCategory::Ops,
        scope: TuiActionScope::Agents,
        execution_kind: TuiActionExecutionKind::Internal,
        palette_visible: true,
        kind: TuiActionKind::CycleAgentMode,
    },
    TuiAction {
        id: TuiActionId::ToggleAgentDetails,
        label: "Toggle Agent Details",
        description: "Show or hide selected agent details",
        shortcut: Some("ctrl+x d"),
        category: TuiActionCategory::Ops,
        scope: TuiActionScope::Agents,
        execution_kind: TuiActionExecutionKind::Internal,
        palette_visible: true,
        kind: TuiActionKind::ToggleAgentDetails,
    },
    TuiAction {
        id: TuiActionId::SelectDashboard,
        label: "Go to Dashboard",
        description: "Switch to Dashboard view",
        shortcut: Some("1"),
        category: TuiActionCategory::Navigation,
        scope: TuiActionScope::Global,
        execution_kind: TuiActionExecutionKind::ViewNavigation,
        palette_visible: true,
        kind: TuiActionKind::SelectView(TuiView::Dashboard),
    },
    TuiAction {
        id: TuiActionId::SelectAgents,
        label: "Go to Agents",
        description: "Switch to Agents view",
        shortcut: Some("2"),
        category: TuiActionCategory::Navigation,
        scope: TuiActionScope::Global,
        execution_kind: TuiActionExecutionKind::ViewNavigation,
        palette_visible: true,
        kind: TuiActionKind::SelectView(TuiView::Agents),
    },
    TuiAction {
        id: TuiActionId::SelectOutput,
        label: "Go to Output",
        description: "Switch to Output view",
        shortcut: Some("3"),
        category: TuiActionCategory::Navigation,
        scope: TuiActionScope::Global,
        execution_kind: TuiActionExecutionKind::ViewNavigation,
        palette_visible: true,
        kind: TuiActionKind::SelectView(TuiView::Output),
    },
    TuiAction {
        id: TuiActionId::SelectServer,
        label: "Go to Server",
        description: "Switch to Server view",
        shortcut: Some("4"),
        category: TuiActionCategory::Navigation,
        scope: TuiActionScope::Global,
        execution_kind: TuiActionExecutionKind::ViewNavigation,
        palette_visible: true,
        kind: TuiActionKind::SelectView(TuiView::Server),
    },
    TuiAction {
        id: TuiActionId::SelectRepl,
        label: "Go to REPL",
        description: "Switch to REPL view",
        shortcut: Some("5"),
        category: TuiActionCategory::Navigation,
        scope: TuiActionScope::Global,
        execution_kind: TuiActionExecutionKind::ViewNavigation,
        palette_visible: true,
        kind: TuiActionKind::SelectView(TuiView::Repl),
    },
    TuiAction {
        id: TuiActionId::StartOrFocusRepl,
        label: "Start or Focus REPL",
        description: "Run golem repl and send input there",
        shortcut: Some("r"),
        category: TuiActionCategory::Repl,
        scope: TuiActionScope::Global,
        execution_kind: TuiActionExecutionKind::NestedCli,
        palette_visible: true,
        kind: TuiActionKind::StartOrFocusRepl,
    },
    TuiAction {
        id: TuiActionId::FocusRepl,
        label: "Focus REPL",
        description: "Send keyboard input to the running REPL",
        shortcut: Some("r"),
        category: TuiActionCategory::Repl,
        scope: TuiActionScope::Repl,
        execution_kind: TuiActionExecutionKind::Internal,
        palette_visible: false,
        kind: TuiActionKind::FocusRepl,
    },
    TuiAction {
        id: TuiActionId::LeaveRepl,
        label: "Leave REPL Mode",
        description: "Return keyboard input to the TUI",
        shortcut: Some("ctrl+x q"),
        category: TuiActionCategory::Repl,
        scope: TuiActionScope::ReplLeader,
        execution_kind: TuiActionExecutionKind::Internal,
        palette_visible: true,
        kind: TuiActionKind::LeaveRepl,
    },
    TuiAction {
        id: TuiActionId::StopRepl,
        label: "Stop REPL",
        description: "Send ctrl+c to the embedded REPL",
        shortcut: Some("ctrl+x k"),
        category: TuiActionCategory::Repl,
        scope: TuiActionScope::ReplLeader,
        execution_kind: TuiActionExecutionKind::Internal,
        palette_visible: true,
        kind: TuiActionKind::StopRepl,
    },
    TuiAction {
        id: TuiActionId::RestartRepl,
        label: "Restart REPL",
        description: "Restart the embedded REPL",
        shortcut: Some("ctrl+x shift+r"),
        category: TuiActionCategory::Repl,
        scope: TuiActionScope::ReplLeader,
        execution_kind: TuiActionExecutionKind::NestedCli,
        palette_visible: true,
        kind: TuiActionKind::RestartRepl,
    },
    TuiAction {
        id: TuiActionId::OpenPalette,
        label: "Open Palette",
        description: "Open the command palette",
        shortcut: Some("ctrl+p"),
        category: TuiActionCategory::System,
        scope: TuiActionScope::System,
        execution_kind: TuiActionExecutionKind::Internal,
        palette_visible: false,
        kind: TuiActionKind::OpenPalette,
    },
    TuiAction {
        id: TuiActionId::ShowHelp,
        label: "Show Help",
        description: "Show TUI shortcuts",
        shortcut: Some("?"),
        category: TuiActionCategory::System,
        scope: TuiActionScope::System,
        execution_kind: TuiActionExecutionKind::Internal,
        palette_visible: true,
        kind: TuiActionKind::ShowHelp,
    },
    TuiAction {
        id: TuiActionId::Quit,
        label: "Quit",
        description: "Exit the TUI",
        shortcut: Some("q"),
        category: TuiActionCategory::System,
        scope: TuiActionScope::Global,
        execution_kind: TuiActionExecutionKind::Internal,
        palette_visible: true,
        kind: TuiActionKind::Quit,
    },
];

fn action(id: TuiActionId) -> &'static TuiAction {
    ACTIONS
        .iter()
        .find(|action| action.id == id)
        .expect("registered action")
}

fn action_by_kind(kind: TuiActionKind) -> Option<&'static TuiAction> {
    ACTIONS.iter().find(|action| action.kind == kind)
}

fn action_shortcut(id: TuiActionId) -> &'static str {
    action(id).shortcut.expect("action shortcut")
}

fn action_short_label(id: TuiActionId) -> &'static str {
    match id {
        TuiActionId::StartOrFocusRepl => "REPL",
        _ => action(id).label,
    }
}

fn palette_actions() -> Vec<TuiAction> {
    ACTIONS
        .iter()
        .copied()
        .filter(|action| action.palette_visible)
        .collect()
}

fn filtered_actions(query: &str) -> Vec<TuiAction> {
    let query = query.trim();
    if query.is_empty() {
        return palette_actions();
    }

    let matcher = SkimMatcherV2::default();
    let mut matches = ACTIONS
        .iter()
        .filter(|action| action.palette_visible)
        .filter_map(|action| {
            let haystack = format!(
                "{} {} {} {} {}",
                action.label,
                action.description,
                action.category.label(),
                action.scope.label(),
                action.execution_kind.label()
            );
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
    use std::collections::{BTreeMap, BTreeSet, HashSet};
    use test_r::test;

    #[test]
    fn renders_dashboard_frame() {
        let app = test_app();
        let frame = render_app_text(&app);

        assert!(
            frame
                .lines()
                .next()
                .is_some_and(|line| line.starts_with('┃')),
            "{frame}"
        );
        assert!(frame.contains("Golem"), "{frame}");
        assert!(frame.contains("app: sample-app"), "{frame}");
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
        assert!(frame.contains("[1] Dashboard"), "{frame}");
        assert!(frame.contains("[5] REPL"), "{frame}");
        assert!(!frame.contains("Environments"), "{frame}");
        assert!(!frame.contains("Components"), "{frame}");
    }

    #[test]
    fn tabs_show_running_indicators_for_output_server_and_repl() {
        let mut app = test_app();
        app.command_run = Some(CommandRun::new(
            1,
            CommandKind::Build,
            vec!["build".to_string()],
            CommandOptions::default(),
        ));
        app.server.run.status = ServerStatus::Running;
        app.repl.run.status = ReplStatus::Running;

        let frame = render_app_text(&app);
        let indicator_count = frame.chars().filter(|character| *character == '●').count();

        assert!(indicator_count >= 3, "{frame}");
    }

    #[test]
    fn tabs_show_idle_indicators_for_output_server_and_repl() {
        let app = test_app();
        let frame = render_app_text(&app);
        let indicator_count = frame.chars().filter(|character| *character == '○').count();

        assert!(indicator_count >= 3, "{frame}");
    }

    #[test]
    fn dashboard_renders_braille_logo_background() {
        let app = test_app();
        let frame = render_app_text(&app);

        assert!(frame.contains("⢶⣿⣿"), "{frame}");
        assert!(frame.contains("⠺⢿⣷"), "{frame}");
    }

    #[test]
    fn dashboard_body_keeps_single_left_rail_on_every_row() {
        let app = test_app();
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| render(frame, &app)).unwrap();

        let buffer = terminal.backend().buffer();
        for y in 3..23 {
            let symbol = buffer.cell((0, y)).expect("missing cell").symbol();
            assert_eq!(symbol, "┃", "missing rail at row {y}");
        }
    }

    #[test]
    fn leader_hint_renders_settings_shortcuts() {
        let mut app = test_app();

        app.handle_key(modified_key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        let frame = render_app_text(&app);

        assert_eq!(app.mode, TuiMode::LeaderNormal);
        assert!(frame.contains("ctrl+x y"), "{frame}");
        assert!(frame.contains("ctrl+x r"), "{frame}");
        assert!(frame.contains("clean:off"), "{frame}");
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
        let width = palette_width("", &visible, label_width, 0, 120);

        assert!(width > 36);
        assert!(width <= 116);
    }

    #[test]
    fn palette_width_is_clamped_on_narrow_terminals() {
        let actions = filtered_actions("");
        let visible = actions.iter().take(8).copied().collect::<Vec<_>>();
        let label_width = palette_label_width(&visible);

        assert_eq!(palette_width("", &visible, label_width, 0, 30), 26);
    }

    #[test]
    fn palette_width_does_not_shrink_when_filtering() {
        let width_actions = palette_actions();
        let label_width = palette_label_width(&width_actions);

        assert_eq!(
            palette_width("", &width_actions, label_width, 0, 120),
            palette_width("comp", &width_actions, label_width, 0, 120)
        );
    }

    #[test]
    fn palette_height_does_not_shrink_when_filtering() {
        let height = palette_height(palette_actions().len().min(8), 40);

        assert_eq!(height, palette_height(palette_actions().len().min(8), 40));
        assert!(height > palette_height(1, 40));
    }

    #[test]
    fn action_registry_has_unique_ids() {
        let mut ids = HashSet::new();

        for action in ACTIONS {
            assert!(
                ids.insert(action.id),
                "duplicate action id: {:?}",
                action.id
            );
        }
    }

    #[test]
    fn action_shortcuts_use_lowercase_notation() {
        for action in ACTIONS {
            if let Some(shortcut) = action.shortcut {
                assert_eq!(
                    shortcut,
                    shortcut.to_lowercase(),
                    "shortcut should use lowercase notation: {shortcut}"
                );
            }
        }
    }

    #[test]
    fn refresh_agents_action_metadata_is_direct() {
        assert_eq!(
            action(TuiActionId::RefreshAgents).execution_kind,
            TuiActionExecutionKind::Direct
        );
        assert!(
            filtered_actions("nested-cli")
                .iter()
                .all(|action| action.id != TuiActionId::RefreshAgents)
        );
        assert!(
            filtered_actions("direct")
                .iter()
                .any(|action| action.id == TuiActionId::RefreshAgents)
        );
    }

    #[test]
    fn palette_uses_visible_actions_and_category_search() {
        assert!(
            filtered_actions("")
                .iter()
                .all(|action| action.palette_visible)
        );
        assert!(
            filtered_actions("")
                .iter()
                .all(|action| action.id != TuiActionId::OpenPalette)
        );

        let ops_actions = filtered_actions("ops");
        assert!(
            ops_actions
                .iter()
                .any(|action| action.id == TuiActionId::RefreshAgents),
            "{ops_actions:?}"
        );
    }

    #[test]
    fn footer_uses_registered_shortcuts() {
        let app = test_app();
        let footer = footer_line(&app)
            .spans
            .iter()
            .fold(String::new(), |mut text, span| {
                text.push_str(&span.content);
                text
            });

        assert!(footer.contains(action_shortcut(TuiActionId::Build)));
        assert!(footer.contains(action(TuiActionId::Build).label));
        assert!(footer.contains(action_shortcut(TuiActionId::Deploy)));
        assert!(footer.contains(action_shortcut(TuiActionId::Clean)));
        assert!(footer.contains(action_shortcut(TuiActionId::StartOrFocusRepl)));
        assert!(footer.contains(action_shortcut(TuiActionId::ShowHelp)));
    }

    #[test]
    fn leader_hints_use_registered_actions() {
        let mut app = test_app();
        let driver = TuiTestDriver::new(120, 32);

        app.handle_key(modified_key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        driver.assert_visible(&app, action_shortcut(TuiActionId::ToggleYes));
        driver.assert_visible(&app, action_shortcut(TuiActionId::ToggleReset));
        driver.assert_visible(&app, action_shortcut(TuiActionId::ToggleServerClean));
        driver.assert_visible(&app, action_shortcut(TuiActionId::OpenPalette));

        app.handle_key(key(KeyCode::Esc));
        app.handle_key(key(KeyCode::Char('r')));
        app.handle_key(modified_key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        driver.assert_visible(&app, action_shortcut(TuiActionId::LeaveRepl));
        driver.assert_visible(&app, action_shortcut(TuiActionId::StopRepl));
        driver.assert_visible(&app, action_shortcut(TuiActionId::RestartRepl));
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

        app.handle_key(modified_key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        app.handle_key(key(KeyCode::Char('m')));

        assert_eq!(app.agents.mode, AgentModeFilter::Ephemeral);
        assert_eq!(app.agents.query, "cart");
        assert_eq!(app.agents.selected, 0);
    }

    #[test]
    fn agent_mode_maps_to_worker_list_mode() {
        assert_eq!(
            AgentModeFilter::Durable.agent_list_mode(),
            AgentListMode::Durable
        );
        assert_eq!(
            AgentModeFilter::Ephemeral.agent_list_mode(),
            AgentListMode::Ephemeral
        );
        assert_eq!(AgentModeFilter::All.agent_list_mode(), AgentListMode::All);
    }

    #[test]
    fn agent_details_panel_toggles() {
        let mut app = test_app();
        app.active_view = TuiView::Agents;
        app.agents.agents = sample_agents();

        let frame = render_app_text(&app);
        assert!(frame.contains("Details"), "{frame}");

        app.handle_key(modified_key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        app.handle_key(key(KeyCode::Char('d')));
        let frame = render_app_text(&app);
        assert!(!frame.contains("Details"), "{frame}");
    }

    #[test]
    fn enter_on_agent_opens_inspect_view() {
        let mut app = test_app();
        app.active_view = TuiView::Agents;
        app.agents.agents = sample_agents();

        app.handle_key(key(KeyCode::Enter));

        assert_eq!(app.agents.view_mode, AgentsViewMode::Inspect);
        assert_eq!(app.agents.inspect.agent_name.as_deref(), Some("cart-1"));
        assert_eq!(app.agents.inspect.focus, AgentInspectPane::Oplog);
        assert_eq!(
            app.agents.inspect.oplog.args,
            vec!["agent", "oplog", "cart-1"]
        );
        assert_eq!(
            app.agents.inspect.stream.args,
            vec!["agent", "stream", "cart-1"]
        );
    }

    #[test]
    fn inspect_left_right_switches_focus() {
        let mut app = inspect_app();

        app.handle_key(key(KeyCode::Right));
        assert_eq!(app.agents.inspect.focus, AgentInspectPane::Stream);

        app.handle_key(key(KeyCode::Left));
        assert_eq!(app.agents.inspect.focus, AgentInspectPane::Oplog);
    }

    #[test]
    fn inspect_scrolls_focused_pane() {
        let mut app = inspect_app();
        for index in 0..20 {
            app.agents
                .inspect
                .oplog
                .output
                .append(format!("oplog {index}\n").as_bytes());
            app.agents
                .inspect
                .stream
                .output
                .append(format!("stream {index}\n").as_bytes());
        }

        app.handle_key(key(KeyCode::PageUp));
        assert_eq!(app.agents.inspect.oplog.output.scroll_offset, 10);
        assert_eq!(app.agents.inspect.stream.output.scroll_offset, 0);

        app.handle_key(key(KeyCode::Right));
        app.handle_key(key(KeyCode::PageUp));
        assert_eq!(app.agents.inspect.stream.output.scroll_offset, 10);
    }

    #[test]
    fn esc_returns_from_inspect_to_agent_list() {
        let mut app = inspect_app();

        app.handle_key(key(KeyCode::Esc));

        assert_eq!(app.agents.view_mode, AgentsViewMode::List);
    }

    #[test]
    fn renders_agent_inspect_split_view() {
        let mut app = inspect_app();
        app.agents.inspect.oplog.output.append(b"oplog entry\n");
        app.agents.inspect.stream.output.append(b"stream entry\n");

        let frame = render_app_text(&app);

        assert!(frame.contains("inspect"), "{frame}");
        assert!(frame.contains("agent:cart-1"), "{frame}");
        assert!(frame.contains("Oplog"), "{frame}");
        assert!(frame.contains("Stream"), "{frame}");
        assert!(frame.contains("oplog entry"), "{frame}");
        assert!(frame.contains("stream entry"), "{frame}");
    }

    #[test]
    fn inspect_events_route_to_separate_buffers() {
        let mut app = inspect_app();
        let (tx, _rx) = test_event_channel();

        app.handle_event(TuiEvent::AgentOplogOutput(b"oplog event\n".to_vec()), &tx);
        app.handle_event(TuiEvent::AgentStreamOutput(b"stream event\n".to_vec()), &tx);

        let oplog =
            String::from_utf8_lossy(&app.agents.inspect.oplog.output.visible_lines(10).concat())
                .to_string();
        let stream =
            String::from_utf8_lossy(&app.agents.inspect.stream.output.visible_lines(10).concat())
                .to_string();
        assert!(oplog.contains("oplog event"));
        assert!(!oplog.contains("stream event"));
        assert!(stream.contains("stream event"));
        assert!(!stream.contains("oplog event"));
    }

    #[test]
    fn typed_agent_metadata_maps_to_agent_list_item() {
        let response = sample_agents_metadata_response(vec![sample_agent_metadata_view(
            "cart",
            "CartAgent(\"cart-1\")",
            golem_common::model::AgentStatus::Running,
        )]);

        let items = agent_items_from_metadata_response(response);

        assert_eq!(items[0].name, "CartAgent(\"cart-1\")");
        assert_eq!(items[0].component.as_deref(), Some("cart"));
        assert_eq!(items[0].agent_type.as_deref(), Some("CartAgent"));
        assert_eq!(items[0].status.as_deref(), Some("Running"));
        assert_eq!(items[0].raw["agentName"], "CartAgent(\"cart-1\")");
    }

    #[test]
    fn agent_refresh_result_updates_agents() {
        let mut app = test_app();
        let (tx, _rx) = test_event_channel();
        app.agents.refresh_generation = 1;
        app.agents.refresh_context_id = Some(TuiContextId::new(1));
        app.agents.refresh_running = true;

        app.handle_event(
            TuiEvent::AgentRefreshFinished {
                generation: 1,
                result: agent_refresh_success(
                    1,
                    sample_agents_metadata_response(vec![sample_agent_metadata_view(
                        "cart",
                        "cart-1",
                        golem_common::model::AgentStatus::Idle,
                    )]),
                ),
            },
            &tx,
        );

        assert!(!app.agents.refresh_running);
        assert_eq!(app.agents.agents.len(), 1);
        assert_eq!(app.agents.agents[0].name, "cart-1");
    }

    #[test]
    fn stale_agent_refresh_context_is_ignored() {
        let mut app = test_app();
        let (tx, _rx) = test_event_channel();
        app.agents.refresh_generation = 1;
        app.agents.refresh_context_id = Some(TuiContextId::new(1));
        app.agents.refresh_running = true;

        app.handle_event(
            TuiEvent::AgentRefreshFinished {
                generation: 1,
                result: agent_refresh_success(
                    2,
                    sample_agents_metadata_response(vec![sample_agent_metadata_view(
                        "cart",
                        "cart-1",
                        golem_common::model::AgentStatus::Idle,
                    )]),
                ),
            },
            &tx,
        );

        assert!(app.agents.refresh_running);
        assert!(app.agents.agents.is_empty());
    }

    #[test]
    fn agent_refresh_error_sets_error_state_with_logs() {
        let mut app = test_app();
        let (tx, _rx) = test_event_channel();
        app.agents.refresh_generation = 1;
        app.agents.refresh_context_id = Some(TuiContextId::new(1));
        app.agents.refresh_running = true;

        app.handle_event(
            TuiEvent::AgentRefreshFinished {
                generation: 1,
                result: TuiContextTaskResult::new(
                    TuiContextId::new(1),
                    Err("refresh failed".to_string()),
                    vec!["captured log".to_string()],
                ),
            },
            &tx,
        );

        assert!(!app.agents.refresh_running);
        assert_eq!(
            app.agents.last_error.as_deref(),
            Some("refresh failed\ncaptured log")
        );
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
        assert!(frame.contains("ctrl+p / :"), "{frame}");
    }

    #[test]
    fn global_help_uses_registered_actions() {
        let mut app = test_app();
        let mut driver = TuiTestDriver::new(120, 48);

        driver.key(&mut app, key(KeyCode::Char('?')));

        driver.assert_visible(&app, action(TuiActionId::Build).label);
        driver.assert_visible(&app, action(TuiActionId::Deploy).label);
        driver.assert_visible(&app, action(TuiActionId::Clean).label);
        driver.assert_visible(&app, action(TuiActionId::StartOrFocusRepl).label);
        driver.assert_visible(&app, action(TuiActionId::ShowHelp).label);
    }

    #[test]
    fn agent_inspect_help_is_reachable_and_contextual() {
        let mut app = inspect_app();
        let mut driver = TuiTestDriver::new(120, 48);

        driver.key(&mut app, key(KeyCode::Char('?')));

        assert_eq!(app.mode, TuiMode::Help);
        driver.assert_visible(&app, "Agent Inspect");
        driver.assert_visible(&app, "left / right");
        driver.assert_visible(&app, "Return to agent list");
    }

    #[test]
    fn repl_leader_help_is_contextual() {
        let mut app = test_app();
        let mut driver = TuiTestDriver::new(120, 48);

        driver.key(&mut app, key(KeyCode::Char('r')));
        driver.key(
            &mut app,
            modified_key(KeyCode::Char('x'), KeyModifiers::CONTROL),
        );
        driver.key(&mut app, key(KeyCode::Char('?')));

        assert_eq!(app.mode, TuiMode::Help);
        driver.assert_visible(&app, "REPL");
        driver.assert_visible(&app, action(TuiActionId::LeaveRepl).label);
        driver.assert_visible(&app, action(TuiActionId::StopRepl).label);
        driver.assert_visible(&app, action(TuiActionId::RestartRepl).label);
    }

    #[test]
    fn command_interaction_help_is_contextual() {
        let mut app = test_app();
        let mut driver = TuiTestDriver::new(120, 48);

        driver.key(&mut app, key(KeyCode::Char('b')));
        driver.key(&mut app, key(KeyCode::Char('?')));

        assert_eq!(app.mode, TuiMode::Help);
        driver.assert_visible(&app, "Command");
        driver.assert_visible(&app, "Cancel command");
        driver.assert_visible(&app, "mouse wheel");
    }

    #[test]
    fn unavailable_palette_action_renders_reason_and_does_not_execute() {
        let mut app = test_app();

        app.handle_key(modified_key(KeyCode::Char('p'), KeyModifiers::CONTROL));
        for character in "refresh agents".chars() {
            app.handle_key(key(KeyCode::Char(character)));
        }
        let frame = render_app_text(&app);

        assert!(frame.contains("Refresh Agents"), "{frame}");
        assert!(frame.contains("context executor unavailable"), "{frame}");

        app.handle_key(key(KeyCode::Enter));

        assert_eq!(app.mode, TuiMode::Palette);
        assert!(!app.agents.refresh_running);
        assert!(app.agents.last_error.is_none());
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

        app.handle_key(modified_key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        app.handle_key(key(KeyCode::Char('y')));
        app.handle_key(modified_key(KeyCode::Char('x'), KeyModifiers::CONTROL));
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

        app.handle_key(modified_key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        app.handle_key(key(KeyCode::Char('y')));
        app.handle_key(modified_key(KeyCode::Char('x'), KeyModifiers::CONTROL));
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

        app.handle_key(modified_key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        app.handle_key(key(KeyCode::Char('y')));
        let frame = render_app_text(&app);
        assert!(frame.contains("stdin"), "{frame}");
    }

    #[test]
    fn output_input_row_is_hidden_for_yes_command() {
        let mut app = test_app();

        app.handle_key(modified_key(KeyCode::Char('x'), KeyModifiers::CONTROL));
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
        let (tx, _rx) = test_event_channel();

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
    fn full_tick_channel_is_not_treated_as_closed() {
        let (tx, _rx) = mpsc::channel(1);
        tx.try_send(TuiEvent::SpinnerTick(1)).expect("first tick");

        assert!(!event_channel_closed(tx.try_send(TuiEvent::SpinnerTick(2))));
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

        app.handle_key(modified_key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        app.handle_key(key(KeyCode::Char('s')));
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
        app.handle_key(modified_key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        app.handle_key(key(KeyCode::Char('R')));
        assert_eq!(app.server.run.status, ServerStatus::Stopping);
        assert_eq!(
            app.server.run.restart_after_stop,
            Some(ServerStartMode::Current)
        );

        app.server.run.status = ServerStatus::Running;
        app.handle_key(modified_key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        app.handle_key(key(KeyCode::Char('C')));
        assert_eq!(
            app.server.run.restart_after_stop,
            Some(ServerStartMode::Clean)
        );
    }

    #[test]
    fn server_can_be_started_from_global_shortcut_or_enter() {
        let mut app = test_app();

        app.handle_key(key(KeyCode::Char('s')));
        assert_eq!(app.active_view, TuiView::Server);
        assert_eq!(app.server.run.status, ServerStatus::Starting);

        app.server.run.status = ServerStatus::Running;
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.server.run.status, ServerStatus::Stopping);
    }

    #[test]
    fn server_palette_action_switches_to_server_and_toggles() {
        let mut app = test_app();

        app.execute_action(TuiActionKind::ToggleServer, None);

        assert_eq!(app.active_view, TuiView::Server);
        assert_eq!(app.server.run.status, ServerStatus::Starting);
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

    #[test]
    fn repl_key_switches_to_repl_and_records_command() {
        let mut app = test_app();

        app.handle_key(key(KeyCode::Char('r')));

        assert_eq!(app.active_view, TuiView::Repl);
        assert_eq!(app.mode, TuiMode::Repl);
        assert_eq!(app.repl.run.status, ReplStatus::Starting);
        assert_eq!(app.repl.run.args, vec!["repl"]);

        let frame = render_app_text(&app);
        assert!(frame.contains("REPL"), "{frame}");
        assert!(frame.contains("starting"), "{frame}");
        assert!(frame.contains("golem repl"), "{frame}");
    }

    #[test]
    fn repl_output_is_rendered_as_terminal_screen() {
        let mut app = test_app();
        let (tx, _rx) = test_event_channel();

        app.handle_key(key(KeyCode::Char('r')));
        app.handle_event(TuiEvent::ReplOutput(b"hello\x1b[2DXY".to_vec()), &tx);

        let frame = render_app_text(&app);
        assert!(frame.contains("helXY"), "{frame}");
    }

    #[test]
    fn repl_focus_sets_cursor_from_terminal_screen() {
        let mut app = test_app();
        let (tx, _rx) = test_event_channel();

        app.handle_key(key(KeyCode::Char('r')));
        app.handle_event(TuiEvent::ReplOutput(b"abc".to_vec()), &tx);
        let (_, cursor) = render_app_text_and_cursor(&app);

        assert!(cursor.x > 2, "cursor should be inside REPL terminal");
        assert!(cursor.y > 3, "cursor should be inside REPL terminal");
    }

    #[test]
    fn repl_leader_can_leave_or_stop_repl() {
        let mut app = test_app();

        app.handle_key(key(KeyCode::Char('r')));
        app.handle_key(modified_key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        assert_eq!(app.mode, TuiMode::LeaderRepl);

        app.handle_key(key(KeyCode::Char('q')));
        assert_eq!(app.mode, TuiMode::Normal);
        assert_eq!(app.repl.run.status, ReplStatus::Starting);

        app.handle_key(key(KeyCode::Char('r')));
        app.handle_key(modified_key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        app.handle_key(key(KeyCode::Char('k')));
        assert_eq!(app.mode, TuiMode::Normal);
        assert_eq!(app.repl.run.status, ReplStatus::Stopping);
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
            repl: ReplState::default(),
            agents: AgentsState::default(),
            next_command_id: 1,
            context: TuiContextInfo {
                application: "sample-app".to_string(),
                environment: "local".to_string(),
                server: "local".to_string(),
                config_dir: "/tmp/golem-config".to_string(),
            },
            context_executor: None,
        }
    }

    fn test_event_channel() -> (TuiEventSender, mpsc::Receiver<TuiEvent>) {
        mpsc::channel(16)
    }

    fn agent_refresh_success(
        context_id: u64,
        response: AgentsMetadataResponseView,
    ) -> TuiContextTaskResult<AgentsMetadataResponseView> {
        TuiContextTaskResult::new(TuiContextId::new(context_id), Ok(response), Vec::new())
    }

    fn sample_agents_metadata_response(
        agents: Vec<AgentMetadataView>,
    ) -> AgentsMetadataResponseView {
        AgentsMetadataResponseView {
            agents,
            cursors: BTreeMap::new(),
        }
    }

    fn sample_agent_metadata_view(
        component_name: &str,
        agent_name: &str,
        status: golem_common::model::AgentStatus,
    ) -> AgentMetadataView {
        AgentMetadataView {
            component_name: golem_common::model::component::ComponentName(
                component_name.to_string(),
            ),
            agent_name: crate::model::worker::RawAgentId(agent_name.to_string()),
            created_by: golem_common::model::account::AccountId(uuid::Uuid::nil()),
            environment_id: golem_common::model::environment::EnvironmentId(uuid::Uuid::nil()),
            env: BTreeMap::new().into_iter().collect(),
            default_env: BTreeMap::new().into_iter().collect(),
            config: Vec::new(),
            default_config: Vec::new(),
            status,
            component_revision: golem_common::model::component::ComponentRevision::new(1).unwrap(),
            retry_count: 0,
            pending_invocation_count: 0,
            updates: Vec::new(),
            created_at: "2024-01-01T00:00:00Z".parse().unwrap(),
            last_error: None,
            component_size: 0,
            total_linear_memory_size: 0,
            exported_resource_instances: BTreeMap::new().into_iter().collect(),
            source_language: crate::agent_id_display::SourceLanguage::default(),
            secret_config_paths: BTreeSet::new(),
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

    fn inspect_app() -> TuiApp {
        let mut app = test_app();
        app.active_view = TuiView::Agents;
        app.agents.agents = sample_agents();
        app.handle_key(key(KeyCode::Enter));
        app
    }

    struct TuiTestDriver {
        width: u16,
        height: u16,
    }

    impl TuiTestDriver {
        fn new(width: u16, height: u16) -> Self {
            Self { width, height }
        }

        fn key(&mut self, app: &mut TuiApp, key: KeyEvent) {
            app.handle_key(key);
        }

        fn frame(&self, app: &TuiApp) -> String {
            render_app_text_at(app, self.width, self.height)
        }

        fn assert_visible(&self, app: &TuiApp, text: &str) {
            let frame = self.frame(app);
            assert!(frame.contains(text), "expected visible `{text}`\n{frame}");
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

    fn render_app_text_at(app: &TuiApp, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| render(frame, app)).unwrap();

        render_buffer_text(terminal.backend().buffer())
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
