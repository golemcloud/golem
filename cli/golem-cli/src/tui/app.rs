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

use crate::app::context::ApplicationContext;
use crate::auth::AuthPresenter;
use crate::command::GolemCliGlobalFlags;
use crate::command_handler::Handlers;
use crate::config::Config;
use crate::context::Context;
use crate::log::Output;
use crate::model::app::ApplicationSourceMode;
use crate::model::app_raw::{BuiltinServer, Server};
use crate::model::environment::EnvironmentReference;
use crate::model::worker::{
    AgentListMode, AgentListRequest, AgentMetadataView, AgentsMetadataResponseView,
};
use crate::tui::TuiEvent;
use crate::tui::context_executor::{TuiContextExecutor, TuiContextId, TuiContextTaskResult};
use crate::tui::input::encode_key_for_pty;
use crate::tui::layout::{
    self, DragTarget, LayoutInput, LayoutSnapshot, RegionKind, TuiLayoutState,
};
use crate::tui::nested_cli::{
    CommandExit, NestedCliRuntime, NestedCliSpec, NestedCliTarget, spawn_nested_cli,
};
use crate::tui::terminal::TerminalGuard;
use crate::tui::terminal_screen::TerminalScreen;
#[cfg(feature = "tui-preview")]
use crate::tui::visual::TuiVisualVariant;
use crate::tui::visual::active_style;
use crate::tui::visual::{TuiVisualStyle, with_style};
use ansi_to_tui::IntoText;
use crossterm::event::{
    Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use futures_util::StreamExt;
use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;
use golem_client::model::{EnvironmentWithDetails, OAuth2WebflowData};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::io::{Write, stdout};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{
    mpsc::{self, Sender},
    oneshot,
};
use tokio::time::{Duration, sleep};

const TUI_EVENT_CHANNEL_CAPACITY: usize = 1024;
const CONTEXT_PICKER_RIGHT_PADDING: usize = 4;

type TuiEventSender = Sender<TuiEvent>;

pub async fn run(ctx: Arc<Context>, global_flags: GolemCliGlobalFlags) -> anyhow::Result<()> {
    let context_executor = Arc::new(TuiContextExecutor::new(ctx.clone()));
    let mut app = TuiApp::from_launch(TuiLaunchConfig::new(ctx.clone(), global_flags));
    app.context_executor = Some(context_executor);
    let mut terminal = TerminalGuard::enter()?;
    let (event_tx, mut event_rx) = mpsc::channel::<TuiEvent>(TUI_EVENT_CHANNEL_CAPACITY);
    spawn_terminal_event_reader(event_tx.clone());

    terminal.draw(|frame| render(frame, &app))?;

    while !app.should_quit {
        let Some(event) = event_rx.recv().await else {
            break;
        };
        match event {
            TuiEvent::AuthPromptStarted { url, ready } => {
                terminal.suspend()?;
                print_tui_auth_prompt(&url)?;
                app.handle_event(
                    TuiEvent::AuthPromptStarted {
                        url,
                        ready: dropped_auth_prompt_ready(),
                    },
                    &event_tx,
                );
                let _ = ready.send(());
            }
            TuiEvent::AuthPromptFinished => {
                app.handle_event(TuiEvent::AuthPromptFinished, &event_tx);
                if let Err(error) = terminal
                    .resume()
                    .and_then(|_| terminal.draw(|frame| render(frame, &app)))
                {
                    terminal.restore_for_exit();
                    return Err(error);
                }
            }
            TuiEvent::Terminal(_)
                if app
                    .auth_prompt
                    .as_ref()
                    .is_some_and(|prompt| !prompt.url.is_empty()) => {}
            event => {
                app.handle_event(event, &event_tx);
                if app.auth_prompt.is_none() {
                    terminal.draw(|frame| render(frame, &app))?;
                }
            }
        }
    }

    app.cleanup_running_command();
    Ok(())
}

#[derive(Clone)]
pub(crate) struct TuiLaunchConfig {
    initial_context: Arc<Context>,
    base_flags: GolemCliGlobalFlags,
}

impl TuiLaunchConfig {
    fn new(initial_context: Arc<Context>, base_flags: GolemCliGlobalFlags) -> Self {
        Self {
            initial_context,
            base_flags,
        }
    }
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

fn spawn_context_environment_list_spinner(
    generation: u64,
    event_tx: TuiEventSender,
    stop: Arc<AtomicBool>,
) {
    tokio::spawn(async move {
        while !stop.load(Ordering::Relaxed) {
            sleep(Duration::from_millis(120)).await;
            if stop.load(Ordering::Relaxed) {
                return;
            }
            if event_channel_closed(
                event_tx.try_send(TuiEvent::ContextEnvironmentListTick { generation }),
            ) {
                return;
            }
        }
    });
}

fn event_channel_closed(result: Result<(), TrySendError<TuiEvent>>) -> bool {
    matches!(result, Err(TrySendError::Closed(_)))
}

fn tui_auth_prompt_text(url: &str) -> String {
    format!(
        "\nAuthenticate with GitHub\n\nOpen this URL in a browser:\n{url}\n\nWaiting for authentication...\n"
    )
}

fn print_tui_auth_prompt(url: &str) -> anyhow::Result<()> {
    let mut stdout = stdout();
    stdout.write_all(tui_auth_prompt_text(url).as_bytes())?;
    stdout.flush()?;
    Ok(())
}

fn dropped_auth_prompt_ready() -> oneshot::Sender<()> {
    let (ready, _rx) = oneshot::channel();
    ready
}

struct TuiAuthPresenter {
    event_tx: TuiEventSender,
}

#[async_trait::async_trait]
impl AuthPresenter for TuiAuthPresenter {
    async fn oauth2_started(&self, data: &OAuth2WebflowData) {
        let (ready_tx, ready_rx) = oneshot::channel();
        if self
            .event_tx
            .send(TuiEvent::AuthPromptStarted {
                url: data.url.to_string(),
                ready: ready_tx,
            })
            .await
            .is_ok()
        {
            let _ = ready_rx.await;
        }
    }

    async fn oauth2_finished(&self) {
        let _ = self.event_tx.send(TuiEvent::AuthPromptFinished).await;
    }
}

fn tui_auth_presenter(event_tx: &TuiEventSender) -> Arc<dyn AuthPresenter> {
    Arc::new(TuiAuthPresenter {
        event_tx: event_tx.clone(),
    })
}

struct TuiApp {
    should_quit: bool,
    active_workspace: TuiWorkspace,
    dev_focus: DevPanel,
    mode: TuiMode,
    palette: CommandPalette,
    command_options: CommandOptions,
    command_run: Option<CommandRun>,
    server: LocalServerService,
    repl: ReplState,
    agents: AgentsState,
    next_command_id: u64,
    context: TuiContextInfo,
    context_switcher: ContextSwitcherState,
    context_cli_args: Vec<String>,
    selected_environment_reference: Option<EnvironmentReference>,
    context_executor: Option<Arc<TuiContextExecutor>>,
    auth_prompt: Option<AuthPromptState>,
    layout: TuiLayoutState,
    layout_snapshot: RefCell<Option<LayoutSnapshot>>,
}

impl TuiApp {
    fn from_launch(launch: TuiLaunchConfig) -> Self {
        let context = TuiContextInfo::from_context(launch.initial_context.as_ref());
        let context_cli_args = context_args_from_flags(&launch.base_flags);
        let server = LocalServerService::from_launch(&launch);
        let targets = context_targets_from_launch(&launch).unwrap_or_else(|error| {
            vec![TuiContextTarget::error(format!(
                "failed to load context targets: {error:#}"
            ))]
        });
        Self {
            should_quit: false,
            active_workspace: TuiWorkspace::Home,
            dev_focus: DevPanel::Repl,
            mode: TuiMode::Normal,
            palette: CommandPalette::default(),
            command_options: CommandOptions::default(),
            command_run: None,
            server,
            repl: ReplState::default(),
            agents: AgentsState::default(),
            next_command_id: 1,
            context,
            context_switcher: ContextSwitcherState::new(targets),
            context_cli_args,
            selected_environment_reference: None,
            context_executor: None,
            auth_prompt: None,
            layout: TuiLayoutState::default(),
            layout_snapshot: RefCell::new(None),
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
            TuiEvent::Terminal(Event::Mouse(mouse)) => self.handle_mouse(mouse, Some(event_tx)),
            TuiEvent::Terminal(_) => {}
            TuiEvent::CommandOutput(bytes) => self.append_command_output(&bytes),
            TuiEvent::CommandOutputClosed(error) => {
                if let Some(error) = error {
                    self.append_local_command_line(format!("output closed: {error}"));
                }
            }
            TuiEvent::CommandExited(exit) => self.finish_command(exit, event_tx),
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
            TuiEvent::ReplExited(exit) => self.finish_repl(exit, event_tx),
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
            TuiEvent::ContextSwitchFinished { generation, result } => {
                self.finish_context_switch(generation, result, event_tx)
            }
            TuiEvent::ContextEnvironmentListFinished {
                generation,
                server_key,
                result,
            } => self.finish_context_environment_list(generation, server_key, result),
            TuiEvent::ContextEnvironmentListTick { generation } => {
                self.handle_context_environment_list_tick(generation);
            }
            TuiEvent::AuthPromptStarted { url, .. } => {
                self.auth_prompt = Some(AuthPromptState { url });
            }
            TuiEvent::AuthPromptFinished => {
                self.auth_prompt = None;
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
            TuiMode::ContextPicker => self.handle_context_picker_key(key, event_tx),
            TuiMode::ContextSwitchConfirm => self.handle_context_switch_confirm_key(key, event_tx),
            TuiMode::Help => self.handle_help_key(key),
            TuiMode::AgentFilter => self.handle_agent_filter_key(key, event_tx),
            TuiMode::CommandInteraction => self.handle_command_interaction_key(key),
            TuiMode::Repl => self.handle_repl_key(key),
            TuiMode::LeaderRepl => self.handle_leader_key(key, event_tx, TuiMode::Repl),
        }
    }

    fn handle_global_key(&mut self, key: KeyEvent, event_tx: Option<&TuiEventSender>) {
        if self.agents_focused() && self.agents.view_mode == AgentsViewMode::Inspect {
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
                if self.dev_panel_focused(DevPanel::Server) && self.server.run.is_running() {
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
            KeyCode::Char('u') if self.agents_focused() => self.refresh_agents(event_tx),
            KeyCode::Enter if self.agents_focused() => self.open_agent_inspect(event_tx),
            KeyCode::Char('/') if self.agents_focused() => self.mode = TuiMode::AgentFilter,
            KeyCode::Char('s') => self.open_and_toggle_server(event_tx),
            KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.layout.server_drawer_open = !self.layout.server_drawer_open;
            }
            KeyCode::Enter if self.dev_panel_focused(DevPanel::Server) => {
                self.toggle_server(event_tx)
            }
            KeyCode::Enter if self.dev_panel_focused(DevPanel::Repl) => {
                self.start_or_focus_repl(event_tx)
            }
            KeyCode::PageUp if self.dev_panel_focused(DevPanel::Server) => {
                self.scroll_server_up_by(10)
            }
            KeyCode::PageDown if self.dev_panel_focused(DevPanel::Server) => {
                self.scroll_server_down_by(10)
            }
            KeyCode::Home if self.dev_panel_focused(DevPanel::Server) => {
                self.server.run.output.scroll_top()
            }
            KeyCode::End if self.dev_panel_focused(DevPanel::Server) => {
                self.server.run.output.scroll_bottom()
            }
            KeyCode::PageUp => self.scroll_output_up(),
            KeyCode::PageDown => self.scroll_output_down(),
            KeyCode::Home => self.scroll_output_top(),
            KeyCode::End => self.scroll_output_bottom(),
            KeyCode::Up if self.dev_panel_focused(DevPanel::Output) => self.scroll_output_up_by(1),
            KeyCode::Down if self.dev_panel_focused(DevPanel::Output) => {
                self.scroll_output_down_by(1)
            }
            KeyCode::Up if self.dev_panel_focused(DevPanel::Server) => self.scroll_server_up_by(1),
            KeyCode::Down if self.dev_panel_focused(DevPanel::Server) => {
                self.scroll_server_down_by(1)
            }
            KeyCode::Up if self.agents_focused() => self.select_previous_agent(),
            KeyCode::Down if self.agents_focused() => self.select_next_agent(),
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.open_palette();
            }
            KeyCode::Char(':') => self.open_palette(),
            KeyCode::Char('?') => self.mode = TuiMode::Help,
            KeyCode::Tab if self.active_workspace == TuiWorkspace::Dev => self.next_dev_panel(),
            KeyCode::BackTab if self.active_workspace == TuiWorkspace::Dev => {
                self.previous_dev_panel()
            }
            KeyCode::Char(']') => self.next_workspace(),
            KeyCode::Char('[') => self.previous_workspace(),
            KeyCode::Char('1') => self.active_workspace = TuiWorkspace::Home,
            KeyCode::Char('2') => self.open_dev_workspace(DevPanel::Repl),
            KeyCode::Char('3') => self.open_ops_workspace(event_tx),
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

    fn handle_context_picker_key(&mut self, key: KeyEvent, event_tx: Option<&TuiEventSender>) {
        if self.context_switcher.environment_list_running {
            match key.code {
                KeyCode::Esc => self.cancel_context_environment_list(),
                KeyCode::Char('q') => self.should_quit = true,
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.should_quit = true;
                }
                _ => {}
            }
            return;
        }
        match key.code {
            KeyCode::Esc if self.context_switcher.mode == ContextPickerStep::AppEnvironments => {
                self.context_switcher.show_targets();
            }
            KeyCode::Esc => self.mode = self.default_mode(),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.mode = self.default_mode();
            }
            KeyCode::Down | KeyCode::Tab => self.context_switcher.next(),
            KeyCode::Up | KeyCode::BackTab => self.context_switcher.previous(),
            KeyCode::Enter => self.select_context_picker_row(event_tx),
            _ => {}
        }
    }

    fn handle_context_switch_confirm_key(
        &mut self,
        key: KeyEvent,
        event_tx: Option<&TuiEventSender>,
    ) {
        match key.code {
            KeyCode::Enter | KeyCode::Char('y') => {
                self.confirm_context_switch(event_tx);
            }
            KeyCode::Esc | KeyCode::Char('n') => {
                self.context_switcher.pending_action = None;
                self.context_switcher.waiting_for_dev_stop = false;
                self.mode = TuiMode::ContextPicker;
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
            KeyCode::Char('e') => {
                self.open_context_picker();
            }
            KeyCode::Char('y') => {
                self.command_options.yes = !self.command_options.yes;
                self.mode = return_mode;
            }
            KeyCode::Char('r') => {
                self.command_options.reset = !self.command_options.reset;
                self.mode = return_mode;
            }
            KeyCode::Char('s') => {
                if self.server.available {
                    self.server.clean = !self.server.clean;
                }
                self.mode = return_mode;
            }
            KeyCode::Char('l') => {
                self.layout.dev_preset = self.layout.dev_preset.next();
                self.mode = return_mode;
            }
            KeyCode::Char('v') => {
                self.layout.server_drawer_open = !self.layout.server_drawer_open;
                self.mode = return_mode;
            }
            KeyCode::Char('a') if self.agents_focused() => {
                self.toggle_agent_auto_refresh(event_tx);
                self.mode = return_mode;
            }
            KeyCode::Char('d') if self.agents_focused() => {
                self.agents.detail_visible = !self.agents.detail_visible;
                self.mode = return_mode;
            }
            KeyCode::Char('m') if self.agents_focused() => {
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
            KeyCode::Char('R') if self.dev_panel_focused(DevPanel::Server) => {
                self.restart_server(ServerStartMode::Current, event_tx);
                self.mode = return_mode;
            }
            KeyCode::Char('C') if self.dev_panel_focused(DevPanel::Server) => {
                self.restart_server(ServerStartMode::Clean, event_tx);
                self.mode = return_mode;
            }
            _ => self.mode = return_mode,
        }
    }

    fn handle_mouse(&mut self, mouse: MouseEvent, event_tx: Option<&TuiEventSender>) {
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.scroll_region_under_pointer(mouse.column, mouse.row, 3, true)
            }
            MouseEventKind::ScrollDown => {
                self.scroll_region_under_pointer(mouse.column, mouse.row, 3, false)
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.layout.dragging = self.drag_target_at(mouse.column, mouse.row);
                self.click_region(mouse.column, mouse.row, event_tx);
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                self.drag_layout(mouse.column, mouse.row);
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.drag_layout(mouse.column, mouse.row);
                self.layout.dragging = None;
            }
            _ => {}
        }
    }

    fn scroll_region_under_pointer(&mut self, x: u16, y: u16, amount: usize, up: bool) {
        let region = self
            .layout_snapshot
            .borrow()
            .as_ref()
            .and_then(|snapshot| snapshot.hit_test(x, y));
        if region.is_none() {
            self.scroll_focused_region(amount, up);
            return;
        }
        match region {
            Some(RegionKind::DevPanelBody(DevPanel::Output)) => {
                if up {
                    self.scroll_output_up_by(amount);
                } else {
                    self.scroll_output_down_by(amount);
                }
            }
            Some(RegionKind::DevPanelBody(DevPanel::Server)) | Some(RegionKind::ServerDrawer) => {
                if up {
                    self.scroll_server_up_by(amount);
                } else {
                    self.scroll_server_down_by(amount);
                }
            }
            Some(RegionKind::OpsInspectPane(pane)) => {
                self.agents.inspect.focus = pane;
                if up {
                    self.scroll_agent_inspect_up_by(amount);
                } else {
                    self.scroll_agent_inspect_down_by(amount);
                }
            }
            _ => {}
        }
    }

    fn scroll_focused_region(&mut self, amount: usize, up: bool) {
        if self.dev_panel_focused(DevPanel::Output) {
            if up {
                self.scroll_output_up_by(amount);
            } else {
                self.scroll_output_down_by(amount);
            }
        } else if self.dev_panel_focused(DevPanel::Server) {
            if up {
                self.scroll_server_up_by(amount);
            } else {
                self.scroll_server_down_by(amount);
            }
        } else if self.ops_agents_focused() && self.agents.view_mode == AgentsViewMode::Inspect {
            if up {
                self.scroll_agent_inspect_up_by(amount);
            } else {
                self.scroll_agent_inspect_down_by(amount);
            }
        }
    }

    fn click_region(&mut self, x: u16, y: u16, event_tx: Option<&TuiEventSender>) {
        let hit = self
            .layout_snapshot
            .borrow()
            .as_ref()
            .and_then(|snapshot| snapshot.hit_test(x, y));
        match hit {
            Some(RegionKind::HeaderTab(TuiWorkspace::Ops)) => self.open_ops_workspace(event_tx),
            Some(RegionKind::HeaderTab(workspace)) => self.active_workspace = workspace,
            Some(RegionKind::DevPanelTitle(panel)) | Some(RegionKind::DevPanelBody(panel)) => {
                self.active_workspace = TuiWorkspace::Dev;
                self.dev_focus = panel;
            }
            Some(RegionKind::ServerDrawer) => {
                self.layout.server_drawer_open = true;
            }
            Some(RegionKind::OpsList) => self.select_agent_at_row(y),
            Some(RegionKind::OpsInspectPane(pane)) => self.agents.inspect.focus = pane,
            Some(RegionKind::ContextPickerRow(index)) => {
                self.context_switcher.selected = index;
                self.select_context_picker_row(event_tx);
            }
            Some(RegionKind::ContextConfirm) => self.confirm_context_switch(event_tx),
            Some(RegionKind::ContextCancel) => {
                self.context_switcher.pending_action = None;
                self.context_switcher.waiting_for_dev_stop = false;
                self.mode = TuiMode::ContextPicker;
            }
            _ => {}
        }
    }

    fn drag_target_at(&self, x: u16, y: u16) -> Option<DragTarget> {
        self.layout_snapshot
            .borrow()
            .as_ref()
            .and_then(|snapshot| match snapshot.hit_test(x, y) {
                Some(RegionKind::DevPrimarySplit) => Some(DragTarget::DevPrimary),
                Some(RegionKind::DevSecondarySplit) => Some(DragTarget::DevSecondary),
                Some(RegionKind::ServerDrawerSplit) => Some(DragTarget::ServerDrawer),
                _ => None,
            })
    }

    fn drag_layout(&mut self, x: u16, y: u16) {
        let Some(target) = self.layout.dragging else {
            return;
        };
        let Some(snapshot) = self.layout_snapshot.borrow().clone() else {
            return;
        };
        match target {
            DragTarget::DevPrimary => {
                let ratio = layout::ratio_from_pointer(
                    snapshot.workspace_body,
                    self.layout.dev_preset,
                    x,
                    y,
                );
                self.layout.dev_primary_ratio = layout::clamp_dev_primary_ratio(
                    snapshot.workspace_body,
                    self.layout.dev_preset,
                    ratio,
                );
            }
            DragTarget::DevSecondary => {
                let ratio = layout::secondary_ratio_from_pointer(
                    snapshot.workspace_body,
                    self.layout.dev_preset,
                    x,
                    y,
                );
                self.layout.dev_secondary_ratio = layout::clamp_dev_secondary_ratio(
                    snapshot.workspace_body,
                    self.layout.dev_preset,
                    ratio,
                );
            }
            DragTarget::ServerDrawer => {
                let ratio = layout::drawer_ratio_from_pointer(snapshot.body, x);
                self.layout.server_drawer_ratio = layout::clamp_drawer_ratio(snapshot.body, ratio);
            }
        }
    }

    fn select_agent_at_row(&mut self, y: u16) {
        let Some(list_area) = self
            .layout_snapshot
            .borrow()
            .as_ref()
            .and_then(|snapshot| snapshot.region(RegionKind::OpsList))
        else {
            return;
        };
        if y < list_area.y {
            return;
        }
        let visible_index = y.saturating_sub(list_area.y) as usize;
        let filtered = self.filtered_agents();
        if filtered.is_empty() {
            return;
        }
        let selected = self.agents.selected.min(filtered.len().saturating_sub(1));
        let start = selected.saturating_sub((list_area.height as usize).saturating_sub(1));
        self.agents.selected = (start + visible_index).min(filtered.len().saturating_sub(1));
    }

    fn execute_action(&mut self, action: TuiActionKind, event_tx: Option<&TuiEventSender>) {
        match action {
            TuiActionKind::SelectView(view) => {
                self.close_palette();
                match view {
                    TuiWorkspace::Ops => self.open_ops_workspace(event_tx),
                    workspace => self.active_workspace = workspace,
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
                if self.server.available {
                    self.server.clean = !self.server.clean;
                }
                self.close_palette();
            }
            TuiActionKind::CycleDevLayoutPreset => {
                self.layout.dev_preset = self.layout.dev_preset.next();
                self.close_palette();
            }
            TuiActionKind::ToggleServerDrawer => {
                self.layout.server_drawer_open = !self.layout.server_drawer_open;
                self.close_palette();
            }
            TuiActionKind::RefreshAgents => {
                self.close_palette();
                self.refresh_agents(event_tx);
            }
            TuiActionKind::OpenContextPicker => {
                self.palette.reset();
                self.open_context_picker();
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
            TuiActionId::Build
            | TuiActionId::Deploy
            | TuiActionId::Clean
            | TuiActionId::StartOrFocusRepl
            | TuiActionId::RestartRepl
                if !self.context.dev_eligible =>
            {
                TuiActionAvailability::Unavailable("selected context is ops-only")
            }
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
            TuiActionId::OpenContextPicker if self.context_switch_busy() => {
                TuiActionAvailability::Unavailable("context switch already running")
            }
            TuiActionId::ToggleServer
            | TuiActionId::RestartServer
            | TuiActionId::CleanRestartServer
            | TuiActionId::ToggleServerClean
                if !self.server.available =>
            {
                TuiActionAvailability::Unavailable("local server is unavailable")
            }
            _ => TuiActionAvailability::Available,
        }
    }

    fn open_palette(&mut self) {
        self.palette.reset();
        self.mode = TuiMode::Palette;
    }

    fn open_context_picker(&mut self) {
        self.context_switcher.last_error = None;
        self.context_switcher.clamp_selection();
        self.mode = TuiMode::ContextPicker;
    }

    fn close_palette(&mut self) {
        self.palette.reset();
        self.mode = self.default_mode();
    }

    fn default_mode(&self) -> TuiMode {
        if self.command_is_running() {
            TuiMode::CommandInteraction
        } else if self.dev_panel_focused(DevPanel::Repl) && self.repl.is_running() {
            TuiMode::Repl
        } else {
            TuiMode::Normal
        }
    }

    fn next_workspace(&mut self) {
        self.active_workspace =
            TuiWorkspace::from_index((self.active_workspace.index() + 1) % TuiWorkspace::ALL.len());
    }

    fn previous_workspace(&mut self) {
        self.active_workspace = TuiWorkspace::from_index(
            (self.active_workspace.index() + TuiWorkspace::ALL.len() - 1) % TuiWorkspace::ALL.len(),
        );
    }

    fn next_dev_panel(&mut self) {
        self.dev_focus = self.dev_focus.next();
    }

    fn previous_dev_panel(&mut self) {
        self.dev_focus = self.dev_focus.previous();
    }

    fn open_dev_workspace(&mut self, panel: DevPanel) {
        self.active_workspace = TuiWorkspace::Dev;
        self.dev_focus = panel;
    }

    fn open_ops_workspace(&mut self, event_tx: Option<&TuiEventSender>) {
        self.active_workspace = TuiWorkspace::Ops;
        if self.agents.agents.is_empty() && !self.agents.refresh_running {
            self.refresh_agents(event_tx);
        }
    }

    fn dev_panel_focused(&self, panel: DevPanel) -> bool {
        self.active_workspace == TuiWorkspace::Dev && self.dev_focus == panel
    }

    fn ops_agents_focused(&self) -> bool {
        self.active_workspace == TuiWorkspace::Ops
    }

    fn agents_focused(&self) -> bool {
        self.ops_agents_focused() || self.dev_panel_focused(DevPanel::Agents)
    }

    fn context_switch_dev_blockers(&self) -> Vec<&'static str> {
        let mut blockers = Vec::new();
        if self.command_is_running() {
            blockers.push("command");
        }
        if self.repl.is_running() {
            blockers.push("repl");
        }
        blockers
    }

    fn context_switch_dev_blocker_message(&self) -> Option<String> {
        let blockers = self.context_switch_dev_blockers();
        if blockers.is_empty() {
            None
        } else {
            Some(format!(
                "Switching context will stop running dev jobs: {}",
                blockers.join(", ")
            ))
        }
    }

    fn context_switch_busy(&self) -> bool {
        self.context_switcher.switch_running || self.context_switcher.environment_list_running
    }

    fn context_switch_pending(&self) -> bool {
        self.context_switcher.pending_action.is_some()
            || self.context_switcher.waiting_for_dev_stop
            || self.context_switch_busy()
    }

    fn select_context_picker_row(&mut self, event_tx: Option<&TuiEventSender>) {
        match self.context_switcher.mode {
            ContextPickerStep::Targets => self.select_context_target(event_tx),
            ContextPickerStep::AppEnvironments => self.select_context_app_environment(event_tx),
        }
    }

    fn select_context_target(&mut self, event_tx: Option<&TuiEventSender>) {
        if self.context_switch_busy() {
            return;
        }

        let Some(target) = self.context_switcher.selected_target().cloned() else {
            return;
        };
        if let Some(error) = &target.error {
            self.context_switcher.last_error = Some(error.clone());
            return;
        }
        if target.is_current {
            self.context_switcher.last_error = None;
            return;
        }
        match &target.kind {
            TuiContextTargetKind::ManifestAppContext { .. } => {
                self.request_context_switch_action(
                    PendingContextSwitchAction::Switch {
                        global_flags: target.global_flags.clone(),
                        environment_reference: None,
                    },
                    event_tx,
                );
            }
            TuiContextTargetKind::ServerTarget { .. } => {
                self.request_context_switch_action(
                    PendingContextSwitchAction::LoadEnvironments { target },
                    event_tx,
                );
            }
            TuiContextTargetKind::Error => {}
        }
    }

    fn select_context_app_environment(&mut self, event_tx: Option<&TuiEventSender>) {
        if self.context_switch_busy() {
            return;
        }

        let Some(target) = self.context_switcher.selected_app_environment().cloned() else {
            return;
        };
        if let Some(error) = &target.error {
            self.context_switcher.last_error = Some(error.clone());
            return;
        }

        self.request_context_switch_action(
            PendingContextSwitchAction::Switch {
                global_flags: target.server_flags.clone(),
                environment_reference: target.environment_reference.clone(),
            },
            event_tx,
        );
    }

    fn request_context_switch_action(
        &mut self,
        action: PendingContextSwitchAction,
        event_tx: Option<&TuiEventSender>,
    ) {
        self.close_ops_for_context_switch();
        self.context_switcher.last_error = None;
        if self.context_switch_dev_blockers().is_empty() {
            self.start_context_switch_action(action, event_tx);
        } else {
            self.context_switcher.pending_action = Some(action);
            self.context_switcher.waiting_for_dev_stop = false;
            self.mode = TuiMode::ContextSwitchConfirm;
        }
    }

    fn confirm_context_switch(&mut self, event_tx: Option<&TuiEventSender>) {
        if self.context_switcher.pending_action.is_none() {
            self.mode = TuiMode::ContextPicker;
            return;
        }
        self.close_ops_for_context_switch();
        self.request_dev_stop_for_context_switch();
        self.context_switcher.waiting_for_dev_stop = true;
        self.resume_pending_context_switch_if_ready(event_tx);
    }

    fn start_context_switch_action(
        &mut self,
        action: PendingContextSwitchAction,
        event_tx: Option<&TuiEventSender>,
    ) {
        match action {
            PendingContextSwitchAction::Switch {
                global_flags,
                environment_reference,
            } => self.start_context_switch(global_flags, environment_reference, event_tx),
            PendingContextSwitchAction::LoadEnvironments { target } => {
                self.start_context_environment_list(target, event_tx)
            }
        }
    }

    fn resume_pending_context_switch_if_ready(&mut self, event_tx: Option<&TuiEventSender>) {
        if !self.context_switcher.waiting_for_dev_stop
            || !self.context_switch_dev_blockers().is_empty()
        {
            return;
        }
        self.context_switcher.waiting_for_dev_stop = false;
        if let Some(action) = self.context_switcher.pending_action.take() {
            self.mode = TuiMode::ContextPicker;
            self.start_context_switch_action(action, event_tx);
        } else {
            self.mode = TuiMode::ContextPicker;
        }
    }

    fn close_ops_for_context_switch(&mut self) {
        self.close_agent_inspect_jobs();
        self.agents.refresh_running = false;
        self.agents.refresh_context_id = None;
        self.agents.refresh_generation += 1;
    }

    fn request_dev_stop_for_context_switch(&mut self) {
        if self.command_is_running() {
            self.cancel_or_force_kill_command();
        }
        if self.repl.is_running() {
            self.stop_repl();
        }
    }

    fn cancel_context_environment_list(&mut self) {
        self.context_switcher.environment_list_running = false;
        self.context_switcher.stop_environment_list_spinner();
        self.context_switcher.generation += 1;
        self.context_switcher.pending_key = None;
        self.context_switcher.last_error = None;
        self.context_switcher.show_targets();
    }

    fn handle_context_environment_list_tick(&mut self, generation: u64) {
        if generation == self.context_switcher.generation
            && self.context_switcher.environment_list_running
        {
            self.context_switcher.environment_list_spinner_frame = self
                .context_switcher
                .environment_list_spinner_frame
                .wrapping_add(1);
        }
    }

    fn start_context_switch(
        &mut self,
        global_flags: GolemCliGlobalFlags,
        environment_reference: Option<EnvironmentReference>,
        event_tx: Option<&TuiEventSender>,
    ) {
        let Some(event_tx) = event_tx.cloned() else {
            return;
        };
        let Some(context_executor) = self.context_executor.clone() else {
            self.context_switcher.last_error =
                Some("TUI context executor is not available".to_string());
            return;
        };

        self.context_switcher.generation += 1;
        self.context_switcher.switch_running = true;
        self.context_switcher.pending_key = environment_reference
            .as_ref()
            .map(ToString::to_string)
            .or_else(|| Some("context".to_string()));
        self.context_switcher.last_error = None;
        let generation = self.context_switcher.generation;
        let auth_presenter = Some(tui_auth_presenter(&event_tx));
        context_executor.spawn(
            event_tx,
            auth_presenter,
            move |_launch_context| async move {
                Context::new(global_flags.clone(), Some(Output::Captured))
                    .await
                    .map(|context| (Arc::new(context), environment_reference))
            },
            move |result| TuiEvent::ContextSwitchFinished { generation, result },
        );
    }

    fn start_context_environment_list(
        &mut self,
        target: TuiContextTarget,
        event_tx: Option<&TuiEventSender>,
    ) {
        let Some(event_tx) = event_tx.cloned() else {
            return;
        };
        let Some(context_executor) = self.context_executor.clone() else {
            self.context_switcher.last_error =
                Some("TUI context executor is not available".to_string());
            return;
        };

        self.context_switcher.generation += 1;
        self.context_switcher.environment_list_running = true;
        self.context_switcher.environment_list_spinner_frame = 0;
        self.context_switcher.pending_key = Some(target.key.clone());
        self.context_switcher.last_error = None;
        let generation = self.context_switcher.generation;
        let server_key = target.key.clone();
        let stop = Arc::new(AtomicBool::new(false));
        spawn_context_environment_list_spinner(generation, event_tx.clone(), stop.clone());
        self.context_switcher.environment_list_spinner_stop = Some(stop);
        let auth_presenter = Some(tui_auth_presenter(&event_tx));
        context_executor.spawn(
            event_tx,
            auth_presenter,
            move |_launch_context| async move {
                let context = Arc::new(
                    Context::new(target.global_flags.clone(), Some(Output::Captured)).await?,
                );
                let environments = context
                    .environment_handler()
                    .list_visible_environment_details()
                    .await?;
                Ok(environments)
            },
            move |result| TuiEvent::ContextEnvironmentListFinished {
                generation,
                server_key,
                result,
            },
        );
    }

    fn finish_context_switch(
        &mut self,
        generation: u64,
        result: TuiContextTaskResult<(Arc<Context>, Option<EnvironmentReference>)>,
        event_tx: &TuiEventSender,
    ) {
        if generation != self.context_switcher.generation {
            return;
        }
        self.context_switcher.switch_running = false;
        let (_, result, logs) = result.into_parts();
        match result {
            Ok((context, environment_reference)) => {
                let Some(context_executor) = self.context_executor.as_ref() else {
                    self.context_switcher.last_error =
                        Some("TUI context executor is not available".to_string());
                    return;
                };
                context_executor.select_context(context.clone());
                let dev_eligible = self.selected_switch_dev_eligible(&environment_reference);
                self.context = TuiContextInfo::from_selected(
                    context.as_ref(),
                    environment_reference.as_ref(),
                    dev_eligible,
                );
                self.context_cli_args = self.selected_switch_context_args();
                self.selected_environment_reference = environment_reference;
                self.reset_agents_for_context_switch();
                self.context_switcher.last_error = None;
                self.mode = self.default_mode();
                if self.agents_focused() {
                    self.refresh_agents(Some(event_tx));
                }
            }
            Err(error) => {
                self.context_switcher.last_error = Some(agent_refresh_error(error, logs));
            }
        }
    }

    fn finish_context_environment_list(
        &mut self,
        generation: u64,
        server_key: String,
        result: TuiContextTaskResult<Vec<EnvironmentWithDetails>>,
    ) {
        if generation != self.context_switcher.generation {
            return;
        }
        self.context_switcher.environment_list_running = false;
        self.context_switcher.stop_environment_list_spinner();
        let (_, result, logs) = result.into_parts();
        match result {
            Ok(environments) => {
                let Some(server_target) = self
                    .context_switcher
                    .targets
                    .iter()
                    .find(|target| target.key == server_key)
                    .cloned()
                else {
                    self.context_switcher.last_error =
                        Some("selected server target no longer exists".to_string());
                    return;
                };
                let app_environments = app_environment_targets_from_details(
                    &server_target,
                    environments,
                    &self.context,
                );
                self.context_switcher
                    .show_app_environments(server_key, app_environments);
                self.context_switcher.last_error = None;
            }
            Err(error) => {
                self.context_switcher.last_error = Some(agent_refresh_error(error, logs));
            }
        }
    }

    fn selected_switch_dev_eligible(
        &self,
        environment_reference: &Option<EnvironmentReference>,
    ) -> bool {
        match self.context_switcher.mode {
            ContextPickerStep::Targets => {
                self.context_switcher
                    .selected_target()
                    .is_some_and(|target| {
                        matches!(target.kind, TuiContextTargetKind::ManifestAppContext { .. })
                    })
            }
            ContextPickerStep::AppEnvironments => {
                self.context_switcher
                    .selected_app_environment()
                    .is_some_and(|target| target.dev_eligible)
                    || environment_reference.is_none()
            }
        }
    }

    fn selected_switch_context_args(&self) -> Vec<String> {
        match self.context_switcher.mode {
            ContextPickerStep::Targets => self
                .context_switcher
                .selected_target()
                .map(|target| context_args_from_flags(&target.global_flags))
                .unwrap_or_default(),
            ContextPickerStep::AppEnvironments => self
                .context_switcher
                .selected_app_environment()
                .map(|target| context_args_from_flags(&target.server_flags))
                .unwrap_or_default(),
        }
    }

    fn reset_agents_for_context_switch(&mut self) {
        self.close_agent_inspect_jobs();
        self.agents.view_mode = AgentsViewMode::List;
        self.agents.selected = 0;
        self.agents.last_error = None;
        self.agents.agents.clear();
        self.agents.refresh_running = false;
        self.agents.refresh_context_id = None;
        self.agents.refresh_generation += 1;
    }

    fn local_server_unavailable(&self) -> bool {
        !self.server.available
    }

    fn dev_context_unavailable(&self) -> bool {
        !self.context.dev_eligible
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
        if self.context_switch_pending() {
            return;
        }
        if self.agents.refresh_running {
            return;
        }

        let Some(event_tx) = event_tx.cloned() else {
            return;
        };
        let Some(context_executor) = self.context_executor.clone() else {
            self.agents.last_error = Some("TUI context executor is not available".to_string());
            return;
        };

        self.agents.refresh_generation += 1;
        let generation = self.agents.refresh_generation;
        self.agents.refresh_running = true;
        self.agents.last_error = None;
        self.agents.refresh_context_id = Some(context_executor.current_context_id());
        self.agents.refresh_context_label = Some(self.context.short_label());
        let mode = self.agents.mode;
        let environment_reference = self.selected_environment_reference.clone();
        let auth_presenter = Some(tui_auth_presenter(&event_tx));
        context_executor.spawn(
            event_tx,
            auth_presenter,
            move |launch_context| async move {
                let request = AgentListRequest {
                    mode: mode.agent_list_mode(),
                    stable_sort: true,
                    environment_reference,
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
            oplog: InspectJob::start(
                vec!["agent".to_string(), "oplog".to_string(), agent_name.clone()],
                self.context.short_label(),
            ),
            stream: InspectJob::start(
                vec!["agent".to_string(), "stream".to_string(), agent_name],
                self.context.short_label(),
            ),
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
        if self.dev_context_unavailable() {
            self.open_dev_workspace(DevPanel::Output);
            self.command_run = Some(CommandRun::new(
                self.next_command_id,
                kind,
                self.command_args(kind),
                self.command_options,
                self.context.short_label(),
            ));
            self.next_command_id += 1;
            self.append_local_command_line("selected context is ops-only");
            self.set_command_status(CommandStatus::Failed);
            return;
        }
        if self.command_is_running() {
            self.append_local_command_line("command already running");
            self.open_dev_workspace(DevPanel::Output);
            return;
        }

        let args = self.command_args(kind);
        let command_id = self.next_command_id;
        self.next_command_id += 1;
        self.open_dev_workspace(DevPanel::Output);
        self.mode = TuiMode::CommandInteraction;
        self.command_run = Some(CommandRun::new(
            command_id,
            kind,
            args.clone(),
            self.command_options,
            self.context.short_label(),
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
        self.nested_cli_spec(args)
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

    fn finish_command(&mut self, exit: CommandExit, event_tx: &TuiEventSender) {
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
        self.resume_pending_context_switch_if_ready(Some(event_tx));
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
        if self.local_server_unavailable() {
            self.server
                .run
                .output
                .append_local_line("local server is unavailable");
            self.open_dev_workspace(DevPanel::Server);
            return;
        }
        if self.server.run.is_running() {
            self.stop_server();
        } else {
            self.start_server(ServerStartMode::Current, event_tx);
        }
    }

    fn open_and_toggle_server(&mut self, event_tx: Option<&TuiEventSender>) {
        self.open_dev_workspace(DevPanel::Server);
        self.toggle_server(event_tx);
    }

    fn start_server(&mut self, mode: ServerStartMode, event_tx: Option<&TuiEventSender>) {
        if self.local_server_unavailable() {
            self.server
                .run
                .output
                .append_local_line("local server is unavailable");
            self.open_dev_workspace(DevPanel::Server);
            return;
        }
        if self.server.run.is_running() {
            self.server
                .run
                .output
                .append_local_line("server already running");
            self.open_dev_workspace(DevPanel::Server);
            return;
        }

        let clean = match mode {
            ServerStartMode::Current => self.server.clean,
            ServerStartMode::Clean => true,
        };
        let args = server_args(clean);
        self.open_dev_workspace(DevPanel::Server);
        self.server.run = ServerRun::new(
            self.server.next_id,
            clean,
            args.clone(),
            self.server.launch_label.clone(),
        );
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
        self.nested_cli_spec_with_context_args(args, &self.server.cli_args)
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
        if self.local_server_unavailable() {
            self.server
                .run
                .output
                .append_local_line("local server is unavailable");
            self.open_dev_workspace(DevPanel::Server);
            return;
        }
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
        self.resume_pending_context_switch_if_ready(Some(event_tx));
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
        self.open_dev_workspace(DevPanel::Repl);
        if self.dev_context_unavailable() {
            self.repl.run.last_error = Some("selected context is ops-only".to_string());
            self.mode = TuiMode::Normal;
            return;
        }
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
        self.repl.run = ReplRun::new(
            args.clone(),
            screen_rows,
            screen_cols,
            self.context.short_label(),
        );
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
        self.nested_cli_spec(args)
    }

    fn nested_cli_spec(&self, args: Vec<String>) -> anyhow::Result<NestedCliSpec> {
        self.nested_cli_spec_with_context_args(args, &self.context_cli_args)
    }

    fn nested_cli_spec_with_context_args(
        &self,
        args: Vec<String>,
        context_args: &[String],
    ) -> anyhow::Result<NestedCliSpec> {
        let mut env = HashMap::new();
        env.insert("CLICOLOR_FORCE".to_string(), "1".to_string());
        env.insert("FORCE_COLOR".to_string(), "1".to_string());
        if std::env::var("TERM").is_err() || std::env::var("TERM").is_ok_and(|term| term == "dumb")
        {
            env.insert("TERM".to_string(), "xterm-256color".to_string());
        }

        let mut full_args = context_args.to_vec();
        full_args.extend(args);

        Ok(NestedCliSpec {
            program: PathBuf::from(crate::binary_path_to_string()?),
            args: full_args,
            cwd: crate::fs::current_dir_lexical()?,
            env,
        })
    }

    fn focus_repl(&mut self) {
        self.open_dev_workspace(DevPanel::Repl);
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

    fn finish_repl(&mut self, exit: CommandExit, event_tx: &TuiEventSender) {
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
        self.resume_pending_context_switch_if_ready(Some(event_tx));
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
pub(super) enum TuiMode {
    Normal,
    LeaderNormal,
    Palette,
    ContextPicker,
    ContextSwitchConfirm,
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
    launch_context: String,
    output: OutputBuffer,
    runtime: Option<NestedCliRuntime>,
    spinner_frame: usize,
    spinner_stop: Option<Arc<AtomicBool>>,
    exit_code: Option<i32>,
}

impl CommandRun {
    fn new(
        id: u64,
        kind: CommandKind,
        args: Vec<String>,
        options: CommandOptions,
        launch_context: String,
    ) -> Self {
        Self {
            id,
            kind,
            status: CommandStatus::Running,
            options,
            args,
            launch_context,
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

struct LocalServerService {
    available: bool,
    cli_args: Vec<String>,
    launch_label: String,
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
    refresh_context_label: Option<String>,
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
            refresh_context_label: None,
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
pub(super) enum AgentsViewMode {
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
pub(super) enum AgentInspectPane {
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
    launch_context: String,
    status: InspectJobStatus,
    output: OutputBuffer,
    runtime: Option<NestedCliRuntime>,
    exit_code: Option<i32>,
}

impl Default for InspectJob {
    fn default() -> Self {
        Self {
            args: Vec::new(),
            launch_context: String::new(),
            status: InspectJobStatus::Idle,
            output: OutputBuffer::default(),
            runtime: None,
            exit_code: None,
        }
    }
}

impl InspectJob {
    fn start(args: Vec<String>, launch_context: String) -> Self {
        Self {
            args,
            launch_context,
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

impl LocalServerService {
    fn from_launch(launch: &TuiLaunchConfig) -> Self {
        Self {
            cli_args: local_server_args_from_launch_flags(&launch.base_flags),
            launch_label: "local server".to_string(),
            ..Self::default()
        }
    }
}

impl Default for LocalServerService {
    fn default() -> Self {
        Self {
            available: true,
            cli_args: Vec::new(),
            launch_label: "local server".to_string(),
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
    launch_context: String,
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
            launch_context: String::new(),
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
    fn new(id: u64, clean: bool, args: Vec<String>, launch_context: String) -> Self {
        Self {
            id,
            status: ServerStatus::Starting,
            clean,
            args,
            launch_context,
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
    launch_context: String,
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
            launch_context: String::new(),
            screen: TerminalScreen::new(20, 80),
            runtime: None,
            exit_code: None,
            last_error: None,
        }
    }
}

impl ReplRun {
    fn new(args: Vec<String>, rows: u16, cols: u16, launch_context: String) -> Self {
        Self {
            status: ReplStatus::Starting,
            args,
            launch_context,
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
pub(super) enum TuiWorkspace {
    Home,
    Dev,
    Ops,
}

impl TuiWorkspace {
    pub(super) const ALL: [Self; 3] = [Self::Home, Self::Dev, Self::Ops];

    pub(super) fn title(self) -> &'static str {
        match self {
            Self::Home => "Home",
            Self::Dev => "Dev",
            Self::Ops => "Ops",
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|view| *view == self).unwrap_or(0)
    }

    fn from_index(index: usize) -> Self {
        Self::ALL[index]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DevPanel {
    Repl,
    Output,
    Server,
    Agents,
}

impl DevPanel {
    const ALL: [Self; 4] = [Self::Repl, Self::Output, Self::Server, Self::Agents];

    fn title(self) -> &'static str {
        match self {
            Self::Repl => "REPL",
            Self::Output => "Output",
            Self::Server => "Server",
            Self::Agents => "Agents",
        }
    }

    fn next(self) -> Self {
        Self::ALL[(self.index() + 1) % Self::ALL.len()]
    }

    fn previous(self) -> Self {
        Self::ALL[(self.index() + Self::ALL.len() - 1) % Self::ALL.len()]
    }

    fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|panel| *panel == self)
            .unwrap_or(0)
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
struct ContextSwitcherState {
    targets: Vec<TuiContextTarget>,
    app_environments: Vec<TuiAppEnvironmentTarget>,
    mode: ContextPickerStep,
    selected: usize,
    generation: u64,
    switch_running: bool,
    environment_list_running: bool,
    environment_list_spinner_frame: usize,
    environment_list_spinner_stop: Option<Arc<AtomicBool>>,
    pending_key: Option<String>,
    selected_server_key: Option<String>,
    last_error: Option<String>,
    pending_action: Option<PendingContextSwitchAction>,
    waiting_for_dev_stop: bool,
}

impl ContextSwitcherState {
    fn new(mut targets: Vec<TuiContextTarget>) -> Self {
        targets.sort_by_key(|target| context_target_group(target).order());
        let selected = targets
            .iter()
            .position(|target| target.is_current)
            .unwrap_or(0);
        Self {
            targets,
            app_environments: Vec::new(),
            mode: ContextPickerStep::Targets,
            selected,
            generation: 0,
            switch_running: false,
            environment_list_running: false,
            environment_list_spinner_frame: 0,
            environment_list_spinner_stop: None,
            pending_key: None,
            selected_server_key: None,
            last_error: None,
            pending_action: None,
            waiting_for_dev_stop: false,
        }
    }

    fn selected_target(&self) -> Option<&TuiContextTarget> {
        self.targets.get(self.selected)
    }

    fn selected_app_environment(&self) -> Option<&TuiAppEnvironmentTarget> {
        self.app_environments.get(self.selected)
    }

    fn row_count(&self) -> usize {
        match self.mode {
            ContextPickerStep::Targets => self.targets.len(),
            ContextPickerStep::AppEnvironments => self.app_environments.len(),
        }
    }

    fn clamp_selection(&mut self) {
        self.selected = self.selected.min(self.row_count().saturating_sub(1));
    }

    fn next(&mut self) {
        let count = self.row_count();
        if count > 0 {
            self.selected = (self.selected + 1) % count;
        }
    }

    fn previous(&mut self) {
        let count = self.row_count();
        if count > 0 {
            self.selected = (self.selected + count - 1) % count;
        }
    }

    fn show_targets(&mut self) {
        self.mode = ContextPickerStep::Targets;
        self.selected = self
            .targets
            .iter()
            .position(|target| target.is_current)
            .unwrap_or(0);
        self.app_environments.clear();
        self.selected_server_key = None;
    }

    fn stop_environment_list_spinner(&mut self) {
        if let Some(stop) = self.environment_list_spinner_stop.take() {
            stop.store(true, Ordering::Relaxed);
        }
    }

    fn show_app_environments(
        &mut self,
        server_key: String,
        app_environments: Vec<TuiAppEnvironmentTarget>,
    ) {
        self.mode = ContextPickerStep::AppEnvironments;
        self.selected_server_key = Some(server_key);
        self.app_environments = app_environments;
        self.selected = 0;
    }
}

#[derive(Debug, Clone)]
enum PendingContextSwitchAction {
    Switch {
        global_flags: GolemCliGlobalFlags,
        environment_reference: Option<EnvironmentReference>,
    },
    LoadEnvironments {
        target: TuiContextTarget,
    },
}

#[derive(Debug, Clone)]
struct AuthPromptState {
    url: String,
}

#[derive(Debug, Clone)]
struct TuiContextTarget {
    key: String,
    label: String,
    detail: String,
    kind: TuiContextTargetKind,
    global_flags: GolemCliGlobalFlags,
    error: Option<String>,
    is_current: bool,
}

impl TuiContextTarget {
    fn error(error: String) -> Self {
        Self {
            key: "error".to_string(),
            label: "Context discovery error".to_string(),
            detail: String::new(),
            kind: TuiContextTargetKind::Error,
            global_flags: GolemCliGlobalFlags::default(),
            error: Some(error),
            is_current: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ContextPickerStep {
    Targets,
    AppEnvironments,
}

#[derive(Debug, Clone)]
enum TuiContextTargetKind {
    ManifestAppContext { app: String, environment: String },
    ServerTarget { source: TuiServerTargetSource },
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TuiServerTargetSource {
    ManifestEnvironment,
    Builtin,
    Profile,
    LaunchSelector,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TuiContextTargetGroup {
    ManifestEnvironments,
    Servers,
    Other,
}

impl TuiContextTargetGroup {
    fn label(self) -> &'static str {
        match self {
            Self::ManifestEnvironments => "Manifest Environments",
            Self::Servers => "Servers",
            Self::Other => "Other",
        }
    }

    fn order(self) -> usize {
        match self {
            Self::ManifestEnvironments => 0,
            Self::Servers => 1,
            Self::Other => 2,
        }
    }
}

fn context_target_group(target: &TuiContextTarget) -> TuiContextTargetGroup {
    match target.kind {
        TuiContextTargetKind::ManifestAppContext { .. } => {
            TuiContextTargetGroup::ManifestEnvironments
        }
        TuiContextTargetKind::ServerTarget { .. } => TuiContextTargetGroup::Servers,
        TuiContextTargetKind::Error => TuiContextTargetGroup::Other,
    }
}

#[derive(Debug, Clone)]
struct TuiAppEnvironmentTarget {
    label: String,
    detail: String,
    server_label: String,
    server_flags: GolemCliGlobalFlags,
    environment_reference: Option<EnvironmentReference>,
    app: Option<String>,
    environment: Option<String>,
    dev_eligible: bool,
    error: Option<String>,
}

#[derive(Debug, Clone)]
struct TuiContextInfo {
    application: String,
    environment: String,
    server: String,
    config_dir: String,
    uses_local_server: bool,
    dev_eligible: bool,
}

impl TuiContextInfo {
    fn from_context(ctx: &Context) -> Self {
        Self::from_selected(ctx, None, true)
    }

    fn from_selected(
        ctx: &Context,
        environment_reference: Option<&EnvironmentReference>,
        dev_eligible: bool,
    ) -> Self {
        let manifest_environment = ctx.selected_manifest_environment();
        let (application, environment) = match environment_reference {
            Some(EnvironmentReference::ApplicationEnvironment {
                application_name,
                environment_name,
            })
            | Some(EnvironmentReference::AccountApplicationEnvironment {
                application_name,
                environment_name,
                ..
            }) => (application_name.0.clone(), environment_name.0.clone()),
            Some(EnvironmentReference::Environment { environment_name }) => (
                manifest_environment
                    .map(|environment| environment.application_name.0.clone())
                    .unwrap_or_else(|| "manifest application".to_string()),
                environment_name.0.clone(),
            ),
            None => (
                manifest_environment
                    .map(|environment| environment.application_name.0.clone())
                    .unwrap_or_else(|| "no application manifest".to_string()),
                manifest_environment
                    .map(|environment| environment.environment_name.0.clone())
                    .unwrap_or_else(|| "profile/default".to_string()),
            ),
        };
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
            uses_local_server: ctx.uses_local_server(),
            dev_eligible,
        }
    }

    fn short_label(&self) -> String {
        if self.application == "no application manifest" {
            format!("profile:{}", self.environment)
        } else {
            format!("{}:{}", self.application, self.environment)
        }
    }
}

fn context_target_marker(target: &TuiContextTarget) -> Option<String> {
    if target.is_current {
        return Some("current".to_string());
    }
    match &target.kind {
        TuiContextTargetKind::ManifestAppContext { app, environment } => {
            let scope = format!("{app}/{environment}");
            if target.label.contains(&scope) || target.detail.contains(&scope) {
                None
            } else {
                Some(scope)
            }
        }
        TuiContextTargetKind::ServerTarget { source } => match source {
            TuiServerTargetSource::ManifestEnvironment => Some("manifest server".to_string()),
            TuiServerTargetSource::Builtin => Some("built-in".to_string()),
            TuiServerTargetSource::Profile => Some("profile".to_string()),
            TuiServerTargetSource::LaunchSelector => Some("launch".to_string()),
        },
        TuiContextTargetKind::Error => Some("error".to_string()),
    }
}

fn app_environment_marker(target: &TuiAppEnvironmentTarget) -> Option<String> {
    if target.error.is_some() {
        Some("error".to_string())
    } else if !target.dev_eligible {
        Some("ops-only".to_string())
    } else if target.app.is_none() || target.environment.is_none() {
        Some("server only".to_string())
    } else {
        None
    }
}

fn format_server(server: &Server) -> String {
    match server {
        Server::Builtin(BuiltinServer::Local) => "local".to_string(),
        Server::Builtin(BuiltinServer::Cloud) => "cloud".to_string(),
        Server::Custom(custom) => custom.url.to_string(),
    }
}

fn context_targets_from_launch(launch: &TuiLaunchConfig) -> anyhow::Result<Vec<TuiContextTarget>> {
    let mut targets = Vec::new();
    let mut seen = HashSet::new();
    let current_context = TuiContextInfo::from_context(launch.initial_context.as_ref());

    let app_source_mode = app_source_mode_from_global_flags(&launch.base_flags);
    let preload =
        ApplicationContext::preload_application(app_source_mode, launch.base_flags.dev_mode)?;
    if let Some(application) = preload.application_preload {
        let application_name = application.application_name.value.0.clone();
        for (environment_name, environment) in application.environments {
            let mut flags = context_flags_base(&launch.base_flags);
            flags.environment = Some(EnvironmentReference::Environment {
                environment_name: environment_name.clone(),
            });
            push_context_target(
                &mut targets,
                &mut seen,
                mark_current_context_target(
                    TuiContextTarget {
                        key: format!("manifest:{}", environment_name.0),
                        label: format!("{}/{}", application_name, environment_name.0),
                        detail: format!(
                            "server: {}",
                            environment
                                .server
                                .as_ref()
                                .map(format_server)
                                .unwrap_or_else(|| "local".to_string())
                        ),
                        kind: TuiContextTargetKind::ManifestAppContext {
                            app: application_name.clone(),
                            environment: environment_name.0.clone(),
                        },
                        global_flags: flags,
                        error: None,
                        is_current: false,
                    },
                    &current_context,
                ),
            );

            if matches!(environment.server, Some(Server::Custom(_))) {
                let mut server_flags = context_flags_base(&launch.base_flags);
                server_flags.environment = Some(EnvironmentReference::Environment {
                    environment_name: environment_name.clone(),
                });
                let server_detail = environment
                    .server
                    .as_ref()
                    .map(format_server)
                    .unwrap_or_else(|| "local".to_string());
                push_context_target(
                    &mut targets,
                    &mut seen,
                    mark_current_context_target(
                        TuiContextTarget {
                            key: format!(
                                "server:manifest:{}:{}",
                                environment_name.0, server_detail
                            ),
                            label: format!("Manifest {} server", environment_name.0),
                            detail: server_detail,
                            kind: TuiContextTargetKind::ServerTarget {
                                source: TuiServerTargetSource::ManifestEnvironment,
                            },
                            global_flags: server_flags,
                            error: None,
                            is_current: false,
                        },
                        &current_context,
                    ),
                );
            }
        }
    }

    let mut local_flags = context_flags_base(&launch.base_flags);
    local_flags.local = true;
    push_context_target(
        &mut targets,
        &mut seen,
        mark_current_context_target(
            TuiContextTarget {
                key: "server:builtin:local".to_string(),
                label: "Built-in local".to_string(),
                detail: "server".to_string(),
                kind: TuiContextTargetKind::ServerTarget {
                    source: TuiServerTargetSource::Builtin,
                },
                global_flags: local_flags,
                error: None,
                is_current: false,
            },
            &current_context,
        ),
    );

    let mut cloud_flags = context_flags_base(&launch.base_flags);
    cloud_flags.cloud = true;
    push_context_target(
        &mut targets,
        &mut seen,
        mark_current_context_target(
            TuiContextTarget {
                key: "server:builtin:cloud".to_string(),
                label: "Built-in cloud".to_string(),
                detail: "server".to_string(),
                kind: TuiContextTargetKind::ServerTarget {
                    source: TuiServerTargetSource::Builtin,
                },
                global_flags: cloud_flags,
                error: None,
                is_current: false,
            },
            &current_context,
        ),
    );

    let config = Config::from_dir(&launch.base_flags.config_dir())?;
    let mut profile_names = config.profiles.keys().cloned().collect::<Vec<_>>();
    profile_names.sort();
    for profile_name in profile_names {
        if profile_name.is_builtin() {
            continue;
        }
        let mut flags = context_flags_base(&launch.base_flags);
        flags.profile = Some(profile_name.clone());
        push_context_target(
            &mut targets,
            &mut seen,
            mark_current_context_target(
                TuiContextTarget {
                    key: format!("server:profile:{}", profile_name.0),
                    label: format!("Profile {}", profile_name.0),
                    detail: "configured".to_string(),
                    kind: TuiContextTargetKind::ServerTarget {
                        source: TuiServerTargetSource::Profile,
                    },
                    global_flags: flags,
                    error: None,
                    is_current: false,
                },
                &current_context,
            ),
        );
    }

    if launch.base_flags.environment.is_some()
        || launch.base_flags.local
        || launch.base_flags.cloud
        || launch.base_flags.profile.is_some()
    {
        push_context_target(
            &mut targets,
            &mut seen,
            mark_current_context_target(
                TuiContextTarget {
                    key: "server:launch".to_string(),
                    label: "Launch selector server".to_string(),
                    detail: "resolved launch flags and environment overrides".to_string(),
                    kind: TuiContextTargetKind::ServerTarget {
                        source: TuiServerTargetSource::LaunchSelector,
                    },
                    global_flags: launch.base_flags.clone(),
                    error: None,
                    is_current: false,
                },
                &current_context,
            ),
        );
    }

    Ok(targets)
}

fn mark_current_context_target(
    mut target: TuiContextTarget,
    current_context: &TuiContextInfo,
) -> TuiContextTarget {
    target.is_current = target_matches_current_context(&target, current_context);
    target
}

fn target_matches_current_context(
    target: &TuiContextTarget,
    current_context: &TuiContextInfo,
) -> bool {
    match &target.kind {
        TuiContextTargetKind::ManifestAppContext { app, environment } => {
            current_context.application == *app && current_context.environment == *environment
        }
        TuiContextTargetKind::ServerTarget { source } => {
            current_context.application == "no application manifest"
                && match source {
                    TuiServerTargetSource::Builtin => {
                        target.key.ends_with(":local") && current_context.uses_local_server
                            || target.key.ends_with(":cloud")
                                && current_context.server.contains("golem.cloud")
                    }
                    TuiServerTargetSource::Profile => {
                        target.key.ends_with(":local") && current_context.uses_local_server
                            || target.key.ends_with(":cloud")
                                && current_context.server.contains("golem.cloud")
                    }
                    TuiServerTargetSource::LaunchSelector => true,
                    TuiServerTargetSource::ManifestEnvironment => false,
                }
        }
        TuiContextTargetKind::Error => false,
    }
}

fn push_context_target(
    targets: &mut Vec<TuiContextTarget>,
    seen: &mut HashSet<String>,
    target: TuiContextTarget,
) {
    if seen.insert(target.key.clone()) {
        targets.push(target);
    }
}

fn app_environment_targets_from_details(
    server_target: &TuiContextTarget,
    environments: Vec<EnvironmentWithDetails>,
    current_context: &TuiContextInfo,
) -> Vec<TuiAppEnvironmentTarget> {
    let mut targets = Vec::new();
    let mut seen = HashSet::new();
    for environment in environments {
        let application_name = environment.application.name;
        let environment_name = environment.environment.name;
        let key = format!(
            "{}:app-env:{}:{}",
            server_target.key, application_name.0, environment_name.0
        );
        if !seen.insert(key.clone()) {
            continue;
        }
        let dev_eligible = current_context.dev_eligible
            && current_context.application == application_name.0
            && current_context.environment == environment_name.0;
        let deployment = environment
            .environment
            .current_deployment
            .as_ref()
            .map(|deployment| {
                format!(
                    "deployment r{} v{}",
                    deployment.deployment_revision.get(),
                    deployment.deployment_version.0
                )
            })
            .unwrap_or_else(|| "no current deployment".to_string());
        targets.push(TuiAppEnvironmentTarget {
            label: format!("{}/{}", application_name.0, environment_name.0),
            detail: format!("{} via {}", deployment, server_target.label),
            server_label: server_target.label.clone(),
            server_flags: server_target.global_flags.clone(),
            environment_reference: Some(EnvironmentReference::ApplicationEnvironment {
                application_name: application_name.clone(),
                environment_name: environment_name.clone(),
            }),
            app: Some(application_name.0),
            environment: Some(environment_name.0),
            dev_eligible,
            error: None,
        });
    }
    if targets.is_empty() {
        targets.push(TuiAppEnvironmentTarget {
            label: "No visible app environments".to_string(),
            detail: "This server returned no environments for the active credentials".to_string(),
            server_label: server_target.label.clone(),
            server_flags: server_target.global_flags.clone(),
            environment_reference: None,
            app: None,
            environment: None,
            dev_eligible: false,
            error: Some("no visible app environments".to_string()),
        });
    }
    targets
}

fn context_flags_base(base_flags: &GolemCliGlobalFlags) -> GolemCliGlobalFlags {
    let mut flags = base_flags.clone();
    flags.environment = None;
    flags.local = false;
    flags.cloud = false;
    flags.profile = None;
    flags
}

fn local_server_args_from_launch_flags(base_flags: &GolemCliGlobalFlags) -> Vec<String> {
    context_args_from_flags(&context_flags_base(base_flags))
}

fn app_source_mode_from_global_flags(global_flags: &GolemCliGlobalFlags) -> ApplicationSourceMode {
    if global_flags.disable_app_manifest_discovery {
        ApplicationSourceMode::None
    } else {
        global_flags
            .app_manifest_path
            .clone()
            .map(ApplicationSourceMode::ByRootManifest)
            .unwrap_or(ApplicationSourceMode::Automatic)
    }
}

fn context_args_from_flags(flags: &GolemCliGlobalFlags) -> Vec<String> {
    let mut args = vec![
        "--config-dir".to_string(),
        flags.config_dir().display().to_string(),
    ];

    if let Some(environment) = &flags.environment {
        args.push("--environment".to_string());
        args.push(environment.to_string());
    }
    if flags.local {
        args.push("--local".to_string());
    }
    if flags.cloud {
        args.push("--cloud".to_string());
    }
    if let Some(profile) = &flags.profile {
        args.push("--profile".to_string());
        args.push(profile.to_string());
    }
    if let Some(app_manifest_path) = &flags.app_manifest_path {
        args.push("--app-manifest-path".to_string());
        args.push(app_manifest_path.display().to_string());
    }
    if flags.disable_app_manifest_discovery {
        args.push("--disable-app-manifest-discovery".to_string());
    }
    if flags.dev_mode {
        args.push("--dev-mode".to_string());
    }
    args
}

fn theme() -> crate::tui::visual::TuiVisualStyle {
    active_style()
}

fn render(frame: &mut Frame<'_>, app: &TuiApp) {
    render_styled(frame, app, &TuiVisualStyle::production());
}

fn render_styled(frame: &mut Frame<'_>, app: &TuiApp, visual: &TuiVisualStyle) {
    with_style(visual, || render_tree(frame, app));
}

fn render_tree(frame: &mut Frame<'_>, app: &TuiApp) {
    let snapshot = layout::compute(LayoutInput {
        area: frame.area(),
        active_workspace: app.active_workspace,
        focused_dev_panel: app.dev_focus,
        mode: app.mode,
        context_picker_rows: app.context_switcher.row_count(),
        context_picker_step: app.context_switcher.mode.clone(),
        agents_view_mode: app.agents.view_mode,
        agent_details_visible: app.agents.detail_visible,
        layout: app.layout.clone(),
    });
    app.layout_snapshot.replace(Some(snapshot.clone()));

    render_header(frame, snapshot.header, app);

    render_tabs(frame, snapshot.tabs, app);
    render_separator(frame, snapshot.separator);

    match app.active_workspace {
        TuiWorkspace::Home => render_home_workspace(frame, snapshot.workspace_body, app),
        TuiWorkspace::Dev => render_dev_workspace(frame, snapshot.workspace_body, app),
        TuiWorkspace::Ops => render_agents_view(frame, snapshot.workspace_body, app),
    }
    if let Some(drawer_area) = snapshot.server_drawer {
        render_split_handle(
            frame,
            snapshot
                .region(RegionKind::ServerDrawerSplit)
                .unwrap_or(drawer_area),
        );
        render_server_view(frame, drawer_area, app);
    }

    let footer = Paragraph::new(footer_line(app))
        .style(footer_style())
        .alignment(Alignment::Center);
    frame.render_widget(footer, snapshot.footer);
    render_left_rail(frame, snapshot.footer, footer_rail_style());

    match app.mode {
        TuiMode::Normal => {}
        TuiMode::LeaderNormal => render_leader_hint(frame, app, TuiMode::Normal),
        TuiMode::ContextPicker => render_context_picker(frame, app),
        TuiMode::ContextSwitchConfirm => render_context_switch_confirm(frame, app),
        TuiMode::AgentFilter => {}
        TuiMode::CommandInteraction => {}
        TuiMode::Repl => {}
        TuiMode::LeaderRepl => render_leader_hint(frame, app, TuiMode::Repl),
        TuiMode::Palette => render_palette(frame, app),
        TuiMode::Help => render_help(frame, app),
    }
}

#[cfg(feature = "tui-preview")]
pub(super) fn render_preview_buffer(
    story: &str,
    variant: TuiVisualVariant,
    width: u16,
    height: u16,
) -> anyhow::Result<ratatui::buffer::Buffer> {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let app = preview_story(story)?;
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend)?;
    let visual = TuiVisualStyle::for_variant(variant);
    terminal.draw(|frame| render_styled(frame, &app, &visual))?;
    Ok(terminal.backend().buffer().clone())
}

#[cfg(all(feature = "tui-preview", test))]
pub(super) fn render_production_preview_buffer(
    story: &str,
    width: u16,
    height: u16,
) -> anyhow::Result<ratatui::buffer::Buffer> {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let app = preview_story(story)?;
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend)?;
    terminal.draw(|frame| render(frame, &app))?;
    Ok(terminal.backend().buffer().clone())
}

fn render_home_workspace(frame: &mut Frame<'_>, area: Rect, app: &TuiApp) {
    render_surface(frame, area);
    let home = Paragraph::new(home_lines(app, area.width as usize))
        .style(surface_style())
        .wrap(Wrap { trim: false });
    frame.render_widget(home, area);
    render_left_rail(frame, area, surface_rail_style());
}

fn render_dev_workspace(frame: &mut Frame<'_>, area: Rect, app: &TuiApp) {
    render_surface(frame, area);
    let input = LayoutInput {
        area,
        active_workspace: app.active_workspace,
        focused_dev_panel: app.dev_focus,
        mode: app.mode,
        context_picker_rows: app.context_switcher.row_count(),
        context_picker_step: app.context_switcher.mode.clone(),
        agents_view_mode: app.agents.view_mode,
        agent_details_visible: app.agents.detail_visible,
        layout: app.layout.clone(),
    };

    for (panel, panel_area) in layout::dev_panel_areas(area, &input) {
        if panel_area.width == 0 || panel_area.height == 0 {
            continue;
        }
        render_panel_title(frame, panel_area, panel, app.dev_focus);
        match panel {
            DevPanel::Repl => render_repl_view(frame, inset_top(panel_area), app),
            DevPanel::Output => render_output_view(frame, inset_top(panel_area), app),
            DevPanel::Server => render_server_view(frame, inset_top(panel_area), app),
            DevPanel::Agents => render_agents_view(frame, inset_top(panel_area), app),
        }
    }
    if let Some((primary, secondary)) = layout::dev_split_regions(area, &input) {
        render_split_handle(frame, primary);
        if let Some(secondary) = secondary {
            render_split_handle(frame, secondary);
        }
    }
}

fn render_split_handle(frame: &mut Frame<'_>, area: Rect) {
    frame.render_widget(Paragraph::new("").style(separator_style()), area);
}

fn render_panel_title(frame: &mut Frame<'_>, area: Rect, panel: DevPanel, focused: DevPanel) {
    let style = if panel == focused {
        command_status_bg_style()
    } else {
        tabs_style()
    };
    let title_style = if panel == focused {
        Style::default()
            .fg(theme().accent_hover)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme().text_muted)
    };
    let line = Line::from(vec![
        Span::styled(
            "┃ ",
            if panel == focused {
                command_rail_style()
            } else {
                tabs_rail_style()
            },
        ),
        Span::styled(panel.title(), title_style),
        Span::raw("  "),
        Span::styled("tab focus", Style::default().fg(theme().text_muted)),
    ]);
    frame.render_widget(Paragraph::new(line).style(style), title_area(area));
    render_left_rail(
        frame,
        title_area(area),
        if panel == focused {
            command_rail_style()
        } else {
            tabs_rail_style()
        },
    );
}

fn title_area(area: Rect) -> Rect {
    Rect {
        height: area.height.min(1),
        ..area
    }
}

fn inset_top(area: Rect) -> Rect {
    Rect {
        y: area.y.saturating_add(1),
        height: area.height.saturating_sub(1),
        ..area
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
        Paragraph::new(server_status_line(&app.server, app.server.available))
            .style(command_status_bg_style()),
        summary_area,
    );
    render_left_rail(frame, summary_area, command_rail_style());

    let output_lines = if app.server.run.output.total_lines() == 0 {
        if app.server.available {
            vec![prefixed_line(
                "Server logs will appear here. Press s to start.",
            )]
        } else {
            vec![prefixed_line("Local server is unavailable.")]
        }
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
        Span::raw(" | "),
        fixed_span(
            format!("ctx:{}", job.launch_context),
            24,
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
                agents
                    .refresh_context_label
                    .as_deref()
                    .unwrap_or("refreshing")
            } else {
                "enter inspect"
            },
            24,
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
            TuiActionId::OpenContextPicker,
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
    push_raw_help(&mut lines, "] / [", "Next / previous workspace");
    push_raw_help(&mut lines, "1 / 2 / 3", "Jump to Home / Dev / Ops");
    push_raw_help(&mut lines, "tab", "Next Dev panel");

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
            TuiActionId::CycleDevLayoutPreset,
            TuiActionId::ToggleServerDrawer,
            TuiActionId::OpenContextPicker,
            TuiActionId::RestartServer,
            TuiActionId::CleanRestartServer,
        ],
    );

    if app.agents_focused() {
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
    } else if app.dev_panel_focused(DevPanel::Output) {
        push_section_break(&mut lines, "Output");
        push_raw_help(&mut lines, "up / down", "Scroll output");
        push_raw_help(&mut lines, "pageup / pagedown", "Scroll output");
        push_raw_help(&mut lines, "home / end", "Top / latest output");
        push_raw_help(&mut lines, "mouse wheel", "Scroll output");
    }

    if app.dev_panel_focused(DevPanel::Repl) || app.repl.is_running() {
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

    for (index, view) in TuiWorkspace::ALL.iter().copied().enumerate() {
        if spans.len() > 1 {
            spans.push(Span::raw("  "));
        }

        let tab_style = if view == app.active_workspace {
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

fn tab_status(view: TuiWorkspace, app: &TuiApp) -> Option<Span<'static>> {
    let running = match view {
        TuiWorkspace::Dev => {
            app.command_is_running() || app.server.run.is_running() || app.repl.is_running()
        }
        TuiWorkspace::Ops => app.agents.refresh_running,
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
        push_leader_item(
            &mut spans,
            TuiActionId::CycleDevLayoutPreset,
            format!("layout:{}", app.layout.dev_preset.label()),
        );
        push_leader_item(
            &mut spans,
            TuiActionId::ToggleServerDrawer,
            format!("drawer:{}", flag_state(app.layout.server_drawer_open)),
        );
        push_leader_item(&mut spans, TuiActionId::OpenContextPicker, "context");
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

fn home_lines(app: &TuiApp, width: usize) -> Vec<Line<'static>> {
    vec![
        dashboard_line(
            vec![Span::styled(
                "Home",
                Style::default().add_modifier(Modifier::BOLD),
            )],
            width,
        ),
        Line::default(),
        dashboard_line(vec![Span::raw("Selected context and current work.")], width),
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
            vec![Span::raw(format!(
                "Command     : {}",
                app.command_run
                    .as_ref()
                    .map(command_status_summary)
                    .unwrap_or_else(|| "idle".to_string())
            ))],
            width,
        ),
        dashboard_line(
            vec![Span::raw(format!(
                "Local server: {}",
                server_status_display(&app.server.run)
            ))],
            width,
        ),
        dashboard_line(
            vec![Span::raw(format!(
                "REPL        : {}",
                app.repl.run.status.title()
            ))],
            width,
        ),
        dashboard_line(
            vec![Span::raw(format!(
                "Agents      : {} loaded{}",
                app.agents.agents.len(),
                if app.agents.refresh_running {
                    " (refreshing)"
                } else {
                    ""
                }
            ))],
            width,
        ),
        Line::default(),
        dashboard_line(
            vec![Span::raw("2 Dev workbench   3 Ops agents explorer")],
            width,
        ),
    ]
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
        fixed_span(
            format!("ctx:{}", run.launch_context),
            24,
            Style::default().fg(theme().text_muted),
        ),
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

fn server_status_line(server: &LocalServerService, local_server_available: bool) -> Line<'static> {
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
            format!("ctx:{}", run.launch_context),
            24,
            Style::default().fg(theme().text_muted),
        ),
        Span::raw(" | "),
        fixed_span(
            server_hint(run, local_server_available),
            21,
            if local_server_available {
                Style::default().fg(theme().text_muted)
            } else {
                Style::default().fg(theme().error)
            },
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
            format!("ctx:{}", run.launch_context),
            24,
            Style::default().fg(theme().text_muted),
        ),
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

fn server_hint(run: &ServerRun, local_server_available: bool) -> &'static str {
    if !local_server_available {
        return "local server unavailable";
    }
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

fn command_status_summary(run: &CommandRun) -> String {
    format!("{} {}", run.kind.title(), command_status_display(run))
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

fn render_context_picker(frame: &mut Frame<'_>, app: &TuiApp) {
    let visible_count = app.context_switcher.row_count().min(10);
    let selected = app
        .context_switcher
        .selected
        .min(app.context_switcher.row_count().saturating_sub(1));
    let rows = context_picker_rows(&app.context_switcher, visible_count);
    let area = centered_rect_fixed(
        context_picker_width(&rows, frame.area().width),
        context_picker_height(rows.len(), frame.area().height),
        frame.area(),
    );
    let (title, subtitle, footer) = match app.context_switcher.mode {
        ContextPickerStep::Targets => (
            "Switch Context",
            format!("Current: {}", app.context.short_label()),
            "enter select  up/down move  esc close",
        ),
        ContextPickerStep::AppEnvironments => (
            "Select App Environment",
            app.context_switcher
                .selected_server_key
                .as_ref()
                .and_then(|server_key| {
                    app.context_switcher
                        .targets
                        .iter()
                        .find(|target| &target.key == server_key)
                })
                .map(|target| format!("Server: {} - {}", target.label, target.detail))
                .unwrap_or_else(|| "Server app environments".to_string()),
            "enter switch  up/down move  esc servers",
        ),
    };
    let mut lines = vec![
        palette_line(vec![Span::styled(
            title,
            Style::default().add_modifier(Modifier::BOLD),
        )]),
        palette_line(vec![Span::raw(subtitle)]),
        palette_line(vec![]),
    ];

    if app.context_switcher.switch_running {
        lines.push(palette_line(vec![Span::styled(
            "switching...",
            Style::default().fg(theme().accent),
        )]));
        lines.push(palette_line(vec![]));
    }
    if let Some(error) = &app.context_switcher.last_error {
        lines.push(palette_line(vec![Span::styled(
            error.clone(),
            Style::default().fg(theme().error),
        )]));
        lines.push(palette_line(vec![]));
    }

    let label_width = context_picker_label_width(&rows);
    let content_width = area.width.saturating_sub(2) as usize;
    lines.extend(
        rows.iter()
            .map(|row| context_picker_render_line(row, selected, label_width, content_width)),
    );

    lines.push(palette_line(vec![]));
    lines.push(palette_line(vec![Span::styled(
        footer,
        Style::default().fg(theme().text_muted),
    )]));

    frame.render_widget(Clear, area);
    let content_area = Rect {
        x: area.x.saturating_add(2),
        y: area.y,
        width: area.width.saturating_sub(2),
        height: area.height,
    };
    frame.render_widget(Paragraph::new(lines), content_area);
    render_left_rail(frame, area, Style::default().fg(theme().accent));
    if app.context_switcher.environment_list_running {
        render_context_environment_loading(frame, app);
    }
}

fn render_context_switch_confirm(frame: &mut Frame<'_>, app: &TuiApp) {
    let area = centered_rect_fixed(72, 11, frame.area());
    let message = app
        .context_switch_dev_blocker_message()
        .unwrap_or_else(|| "Switching context will stop running dev jobs.".to_string());
    let status = if app.context_switcher.waiting_for_dev_stop {
        "Waiting for dev jobs to stop..."
    } else {
        "Press enter to stop them gracefully, or esc to keep the current context."
    };
    let lines = vec![
        palette_line(vec![Span::styled(
            "Confirm Context Switch",
            Style::default().add_modifier(Modifier::BOLD),
        )]),
        palette_line(vec![]),
        palette_line(vec![Span::styled(
            message,
            Style::default().fg(theme().marker),
        )]),
        palette_line(vec![]),
        palette_line(vec![Span::raw(status)]),
        palette_line(vec![]),
        palette_line(vec![Span::styled(
            "enter/y confirm  esc/n cancel",
            Style::default().fg(theme().text_muted),
        )]),
    ];
    frame.render_widget(Clear, area);
    let content_area = Rect {
        x: area.x.saturating_add(2),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(4),
        height: area.height.saturating_sub(2),
    };
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        content_area,
    );
    render_left_rail(frame, area, Style::default().fg(theme().marker));
}

fn render_context_environment_loading(frame: &mut Frame<'_>, app: &TuiApp) {
    let area = centered_rect_fixed(52, 7, frame.area());
    let spinner = spinner_symbol(app.context_switcher.environment_list_spinner_frame);
    let lines = vec![
        palette_line(vec![Span::styled(
            "Loading",
            Style::default().add_modifier(Modifier::BOLD),
        )]),
        palette_line(vec![]),
        palette_line(vec![Span::styled(
            format!("{spinner} Loading app environments"),
            Style::default().fg(theme().accent),
        )]),
        palette_line(vec![]),
        palette_line(vec![Span::styled(
            "esc cancel  q quit",
            Style::default().fg(theme().text_muted),
        )]),
    ];
    frame.render_widget(Clear, area);
    let content_area = Rect {
        x: area.x.saturating_add(2),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(4),
        height: area.height.saturating_sub(2),
    };
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        content_area,
    );
    render_left_rail(frame, area, Style::default().fg(theme().accent));
}

#[derive(Debug, Clone)]
enum ContextPickerRenderRow {
    Section(String),
    Item {
        selectable_index: usize,
        label: String,
        marker: Option<String>,
        detail: String,
        unavailable: bool,
    },
}

impl ContextPickerRenderRow {
    fn text_width(&self) -> usize {
        match self {
            Self::Section(label) => label.chars().count() + 2,
            Self::Item {
                label,
                marker,
                detail,
                ..
            } => {
                let marker_width = marker
                    .as_ref()
                    .map(|marker| marker.chars().count() + 1)
                    .unwrap_or_default();
                4 + label.chars().count()
                    + 1
                    + detail.chars().count()
                    + marker_width
                    + CONTEXT_PICKER_RIGHT_PADDING
            }
        }
    }
}

fn context_picker_rows(
    context_switcher: &ContextSwitcherState,
    visible_count: usize,
) -> Vec<ContextPickerRenderRow> {
    match context_switcher.mode {
        ContextPickerStep::Targets => {
            context_switcher
                .targets
                .iter()
                .take(visible_count)
                .enumerate()
                .fold(
                    (Vec::new(), None),
                    |(mut rows, previous_group), (index, target)| {
                        let group = context_target_group(target);
                        if previous_group != Some(group) {
                            rows.push(ContextPickerRenderRow::Section(group.label().to_string()));
                        }
                        rows.push(ContextPickerRenderRow::Item {
                            selectable_index: index,
                            label: target.label.clone(),
                            marker: context_target_marker(target),
                            detail: target
                                .error
                                .as_ref()
                                .cloned()
                                .unwrap_or_else(|| target.detail.clone()),
                            unavailable: target.error.is_some(),
                        });
                        (rows, Some(group))
                    },
                )
                .0
        }
        ContextPickerStep::AppEnvironments => {
            context_switcher
                .app_environments
                .iter()
                .take(visible_count)
                .enumerate()
                .fold(
                    (Vec::new(), None::<bool>),
                    |(mut rows, previous_unavailable), (index, target)| {
                        let unavailable = target.error.is_some();
                        if previous_unavailable != Some(unavailable) {
                            rows.push(ContextPickerRenderRow::Section(
                                if unavailable {
                                    "Unavailable"
                                } else {
                                    "App Environments"
                                }
                                .to_string(),
                            ));
                        }
                        let detail = target.error.as_ref().cloned().unwrap_or_else(|| {
                            format!("{} - {}", target.detail, target.server_label)
                        });
                        rows.push(ContextPickerRenderRow::Item {
                            selectable_index: index,
                            label: target.label.clone(),
                            marker: app_environment_marker(target),
                            detail,
                            unavailable,
                        });
                        (rows, Some(unavailable))
                    },
                )
                .0
        }
    }
}

fn context_picker_render_line(
    row: &ContextPickerRenderRow,
    selected: usize,
    label_width: usize,
    content_width: usize,
) -> Line<'static> {
    match row {
        ContextPickerRenderRow::Section(label) => palette_line(vec![Span::styled(
            format!("  {label}"),
            Style::default()
                .fg(theme().text_secondary)
                .add_modifier(Modifier::BOLD),
        )]),
        ContextPickerRenderRow::Item {
            selectable_index,
            label,
            marker,
            detail,
            unavailable,
        } => {
            let is_selected = *selectable_index == selected;
            let style = match (is_selected, unavailable) {
                (true, true) => Style::default()
                    .fg(theme().text_muted)
                    .add_modifier(Modifier::REVERSED),
                (true, false) => Style::default().add_modifier(Modifier::REVERSED),
                (false, true) => Style::default().fg(theme().text_muted),
                (false, false) => Style::default(),
            };
            let prefix = if is_selected { ">   " } else { "    " };
            let marker_width = marker
                .as_ref()
                .map(|marker| marker.chars().count() + 1)
                .unwrap_or_default();
            let fixed_width = prefix.chars().count() + label_width + 1;
            let detail_width = content_width
                .saturating_sub(fixed_width + marker_width + CONTEXT_PICKER_RIGHT_PADDING);
            let detail_text = if marker.is_some() {
                pad_or_ellipsis(detail, detail_width)
            } else {
                ellipsis_text(detail, detail_width)
            };
            let mut spans = vec![
                Span::styled(prefix.to_string(), style),
                Span::styled(pad_or_ellipsis(label, label_width), style),
                Span::styled(" ".to_string(), style),
                Span::styled(detail_text, style),
            ];
            if let Some(marker) = marker {
                spans.push(Span::styled(" ".to_string(), style));
                spans.push(Span::styled(
                    marker.clone(),
                    style.fg(theme().text_secondary),
                ));
            }
            Line::from(spans)
        }
    }
}

fn context_picker_label_width(rows: &[ContextPickerRenderRow]) -> usize {
    rows.iter()
        .filter_map(|row| match row {
            ContextPickerRenderRow::Item { label, .. } => Some(label.chars().count()),
            ContextPickerRenderRow::Section(_) => None,
        })
        .max()
        .unwrap_or(18)
        .clamp(18, 36)
}

fn context_picker_width(rows: &[ContextPickerRenderRow], terminal_width: u16) -> u16 {
    let preferred_width = rows
        .iter()
        .map(ContextPickerRenderRow::text_width)
        .max()
        .unwrap_or("Switch Context".len())
        .max("Select App Environment".len())
        + 4;
    let max_width = if terminal_width <= 60 {
        terminal_width.saturating_sub(2).max(20) as usize
    } else {
        (terminal_width.saturating_sub(4) as usize).min(120).max(56)
    };
    let min_width = 56.min(max_width);
    preferred_width.clamp(min_width, max_width) as u16
}

fn context_picker_height(row_count: usize, terminal_height: u16) -> u16 {
    let content_height = 6 + row_count.max(1);
    let max_height = terminal_height.saturating_sub(4).max(8) as usize;
    (content_height + 2).clamp(8, max_height) as u16
}

fn ellipsis_text(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let len = text.chars().count();
    if len <= width {
        return text.to_string();
    }
    if width <= 3 {
        return text.chars().take(width).collect();
    }
    format!("{}...", text.chars().take(width - 3).collect::<String>())
}

fn pad_or_ellipsis(text: &str, width: usize) -> String {
    let text = ellipsis_text(text, width);
    let len = text.chars().count();
    if len < width {
        format!("{text}{}", " ".repeat(width - len))
    } else {
        text
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
    CycleDevLayoutPreset,
    ToggleServerDrawer,
    RefreshAgents,
    ToggleAgentAutoRefresh,
    CycleAgentMode,
    ToggleAgentDetails,
    SelectHome,
    SelectDev,
    SelectOps,
    StartOrFocusRepl,
    FocusRepl,
    LeaveRepl,
    StopRepl,
    RestartRepl,
    OpenPalette,
    OpenContextPicker,
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
    SelectView(TuiWorkspace),
    Build,
    Deploy,
    Clean,
    ToggleYes,
    ToggleReset,
    ToggleServer,
    RestartServer,
    CleanRestartServer,
    ToggleServerClean,
    CycleDevLayoutPreset,
    ToggleServerDrawer,
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
    OpenContextPicker,
    ShowHelp,
    Quit,
}

const ACTIONS: [TuiAction; 27] = [
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
        description: "Focus Dev Server and toggle it",
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
        id: TuiActionId::CycleDevLayoutPreset,
        label: "Cycle Dev Layout",
        description: "Cycle Dev panel layout preset",
        shortcut: Some("ctrl+x l"),
        category: TuiActionCategory::Settings,
        scope: TuiActionScope::Leader,
        execution_kind: TuiActionExecutionKind::Internal,
        palette_visible: true,
        kind: TuiActionKind::CycleDevLayoutPreset,
    },
    TuiAction {
        id: TuiActionId::ToggleServerDrawer,
        label: "Toggle Server Drawer",
        description: "Open or close the global local-server drawer",
        shortcut: Some("ctrl+x v"),
        category: TuiActionCategory::Dev,
        scope: TuiActionScope::Leader,
        execution_kind: TuiActionExecutionKind::Internal,
        palette_visible: true,
        kind: TuiActionKind::ToggleServerDrawer,
    },
    TuiAction {
        id: TuiActionId::RefreshAgents,
        label: "Refresh Agents",
        description: "Refresh the Ops agent list",
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
        id: TuiActionId::SelectHome,
        label: "Go to Home",
        description: "Switch to Home workspace",
        shortcut: Some("1"),
        category: TuiActionCategory::Navigation,
        scope: TuiActionScope::Global,
        execution_kind: TuiActionExecutionKind::ViewNavigation,
        palette_visible: true,
        kind: TuiActionKind::SelectView(TuiWorkspace::Home),
    },
    TuiAction {
        id: TuiActionId::SelectDev,
        label: "Go to Dev",
        description: "Switch to Dev workspace",
        shortcut: Some("2"),
        category: TuiActionCategory::Navigation,
        scope: TuiActionScope::Global,
        execution_kind: TuiActionExecutionKind::ViewNavigation,
        palette_visible: true,
        kind: TuiActionKind::SelectView(TuiWorkspace::Dev),
    },
    TuiAction {
        id: TuiActionId::SelectOps,
        label: "Go to Ops",
        description: "Switch to Ops workspace",
        shortcut: Some("3"),
        category: TuiActionCategory::Navigation,
        scope: TuiActionScope::Global,
        execution_kind: TuiActionExecutionKind::ViewNavigation,
        palette_visible: true,
        kind: TuiActionKind::SelectView(TuiWorkspace::Ops),
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
        id: TuiActionId::OpenContextPicker,
        label: "Switch Context",
        description: "Switch selected TUI context",
        shortcut: Some("ctrl+x e"),
        category: TuiActionCategory::Settings,
        scope: TuiActionScope::Leader,
        execution_kind: TuiActionExecutionKind::Internal,
        palette_visible: true,
        kind: TuiActionKind::OpenContextPicker,
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

#[cfg(feature = "tui-preview")]
fn preview_story(name: &str) -> anyhow::Result<TuiApp> {
    use crate::tui::layout::DevLayoutPreset;

    let mut app = TuiApp {
        should_quit: false,
        active_workspace: TuiWorkspace::Home,
        dev_focus: DevPanel::Repl,
        mode: TuiMode::Normal,
        palette: CommandPalette::default(),
        command_options: CommandOptions::default(),
        command_run: None,
        server: LocalServerService::default(),
        repl: ReplState::default(),
        agents: AgentsState::default(),
        next_command_id: 1,
        context: TuiContextInfo {
            application: "preview-app".into(),
            environment: "local".into(),
            server: "local".into(),
            config_dir: "/preview/config".into(),
            uses_local_server: true,
            dev_eligible: true,
        },
        context_switcher: ContextSwitcherState::new(vec![TuiContextTarget {
            key: "manifest:local".into(),
            label: "preview-app/local".into(),
            detail: "server: local".into(),
            kind: TuiContextTargetKind::ManifestAppContext {
                app: "preview-app".into(),
                environment: "local".into(),
            },
            global_flags: GolemCliGlobalFlags::default(),
            error: None,
            is_current: true,
        }]),
        context_cli_args: Vec::new(),
        selected_environment_reference: None,
        context_executor: None,
        auth_prompt: None,
        layout: TuiLayoutState::default(),
        layout_snapshot: RefCell::new(None),
    };

    let add_agents = |app: &mut TuiApp| {
        app.agents.agents = vec![
            AgentListItem {
                name: "cart-1".into(),
                component: Some("cart".into()),
                agent_type: Some("CartAgent".into()),
                status: Some("Running".into()),
                raw: serde_json::json!({"region":"eu-central","revision":7}),
            },
            AgentListItem {
                name: "orders-42".into(),
                component: Some("orders".into()),
                agent_type: Some("OrderAgent".into()),
                status: Some("Idle".into()),
                raw: serde_json::json!({"region":"us-east","revision":3}),
            },
        ];
    };
    let add_command = |app: &mut TuiApp, status| {
        let mut run = CommandRun::new(
            1,
            CommandKind::Build,
            vec!["build".into(), "-P".into(), "release".into()],
            CommandOptions::default(),
            "preview-app:local".into(),
        );
        run.status = status;
        run.output.append_local_line("Building component checkout");
        run.output.append_local_line("Compiled component in 2.4s");
        if status == CommandStatus::Failed {
            run.output
                .append_local_line("error: component validation failed");
        }
        app.command_run = Some(run);
    };

    match name {
        "home-idle" => {}
        "home-active" => add_command(&mut app, CommandStatus::Running),
        "dev-running" => {
            app.active_workspace = TuiWorkspace::Dev;
            add_command(&mut app, CommandStatus::Running);
        }
        "dev-completed" => {
            app.active_workspace = TuiWorkspace::Dev;
            add_command(&mut app, CommandStatus::Succeeded);
        }
        "dev-failed" => {
            app.active_workspace = TuiWorkspace::Dev;
            add_command(&mut app, CommandStatus::Failed);
        }
        "dev-server-drawer" => {
            app.active_workspace = TuiWorkspace::Dev;
            app.layout.server_drawer_open = true;
            app.server.run.status = ServerStatus::Running;
            app.server
                .run
                .output
                .append_local_line("Listening on 127.0.0.1:9881");
        }
        "dev-layout-left" => {
            app.active_workspace = TuiWorkspace::Dev;
            app.layout.dev_preset = DevLayoutPreset::Left;
        }
        "dev-layout-top" => {
            app.active_workspace = TuiWorkspace::Dev;
            app.layout.dev_preset = DevLayoutPreset::Top;
        }
        "dev-layout-bottom" => {
            app.active_workspace = TuiWorkspace::Dev;
            app.layout.dev_preset = DevLayoutPreset::Bottom;
        }
        "ops-list" => {
            app.active_workspace = TuiWorkspace::Ops;
            app.agents.detail_visible = false;
            add_agents(&mut app);
        }
        "ops-details" => {
            app.active_workspace = TuiWorkspace::Ops;
            add_agents(&mut app);
        }
        "ops-loading" => {
            app.active_workspace = TuiWorkspace::Ops;
            app.agents.refresh_running = true;
        }
        "ops-error" => {
            app.active_workspace = TuiWorkspace::Ops;
            app.agents.last_error = Some("preview connection refused".into());
        }
        "agent-inspect" => {
            app.active_workspace = TuiWorkspace::Ops;
            add_agents(&mut app);
            app.agents.view_mode = AgentsViewMode::Inspect;
            app.agents.inspect.agent_name = Some("cart-1".into());
        }
        "palette" => app.open_palette(),
        "help" => app.mode = TuiMode::Help,
        "context-picker" => app.mode = TuiMode::ContextPicker,
        "confirmation" => app.mode = TuiMode::ContextSwitchConfirm,
        "loading" => {
            app.mode = TuiMode::ContextPicker;
            app.context_switcher.environment_list_running = true;
        }
        _ => anyhow::bail!("unknown TUI preview story: {name}"),
    }
    Ok(app)
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
    use crate::command::GolemCliCommand;
    use crate::config::{AuthenticationConfig, Profile, ProfileName};
    use crate::log::{LogContext, Output, logln};
    use clap::Parser;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use std::collections::{BTreeMap, BTreeSet, HashSet};
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;
    use test_r::test;
    use url::Url;

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

        assert!(frame.contains("Home"), "{frame}");
        assert!(frame.contains("Dev"), "{frame}");
        assert!(frame.contains("Ops"), "{frame}");
        assert!(frame.contains("[1] Home"), "{frame}");
        assert!(frame.contains("[3] Ops"), "{frame}");
        assert!(!frame.contains("[4]"), "{frame}");
        assert!(!frame.contains("[5]"), "{frame}");
    }

    #[test]
    fn tabs_show_running_indicator_for_dev_workspace() {
        let mut app = test_app();
        app.command_run = Some(CommandRun::new(
            1,
            CommandKind::Build,
            vec!["build".to_string()],
            CommandOptions::default(),
            "sample-app:local".to_string(),
        ));
        app.server.run.status = ServerStatus::Running;
        app.repl.run.status = ReplStatus::Running;

        let frame = render_app_text(&app);
        let indicator_count = frame.chars().filter(|character| *character == '●').count();

        assert!(indicator_count >= 1, "{frame}");
    }

    #[test]
    fn tabs_show_idle_indicators_for_dev_and_ops() {
        let app = test_app();
        let frame = render_app_text(&app);
        let indicator_count = frame.chars().filter(|character| *character == '○').count();

        assert!(indicator_count >= 2, "{frame}");
    }

    #[test]
    fn dashboard_omits_decorative_logo_background() {
        let app = test_app();
        let frame = render_app_text_at(&app, 160, 32);

        assert!(!frame.contains("⠻⣿⣿"), "{frame}");
        assert!(!frame.contains("⠺⢿⣷"), "{frame}");
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
        assert_eq!(app.active_workspace, TuiWorkspace::Dev);
        assert!(frame.contains("Dev"), "{frame}");

        app.handle_key(key(KeyCode::Char('[')));
        let frame = render_app_text(&app);
        assert_eq!(app.active_workspace, TuiWorkspace::Home);
        assert!(frame.contains("Home"), "{frame}");
    }

    #[test]
    fn jumps_to_tab_with_number() {
        let mut app = test_app();

        app.handle_key(key(KeyCode::Char('3')));
        let frame = render_app_text(&app);

        assert_eq!(app.active_workspace, TuiWorkspace::Ops);
        assert!(frame.contains("agents"), "{frame}");
    }

    #[test]
    fn tab_cycles_dev_panel_focus() {
        let mut app = test_app();
        app.handle_key(key(KeyCode::Char('2')));

        assert_eq!(app.active_workspace, TuiWorkspace::Dev);
        assert_eq!(app.dev_focus, DevPanel::Repl);

        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.dev_focus, DevPanel::Output);

        app.handle_key(key(KeyCode::BackTab));
        assert_eq!(app.dev_focus, DevPanel::Repl);
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
    fn leader_opens_context_picker() {
        let mut app = test_app();

        app.handle_key(modified_key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        app.handle_key(key(KeyCode::Char('e')));

        assert_eq!(app.mode, TuiMode::ContextPicker);
        let frame = render_app_text(&app);
        assert!(frame.contains("Switch Context"), "{frame}");
        assert!(frame.contains("sample-app/local"), "{frame}");
        assert!(frame.contains("current"), "{frame}");
    }

    #[test]
    fn context_picker_groups_and_indents_targets() {
        let mut app = test_app();
        app.context_switcher = ContextSwitcherState::new(vec![
            test_server_context_target("server:profile:prod", "Profile prod", "configured profile"),
            test_manifest_context_target("prod"),
            test_current_context_target(),
        ]);

        app.open_context_picker();
        let frame = render_app_text_at(&app, 140, 32);

        assert!(frame.contains("Manifest Environments"), "{frame}");
        assert!(frame.contains("Servers"), "{frame}");
        assert!(!frame.contains("┃   Current"), "{frame}");
        assert!(frame.contains(">   sample-app/local"), "{frame}");
        assert!(frame.contains("current"), "{frame}");
        assert!(!frame.contains("[manifest"), "{frame}");
        assert!(frame.contains("    sample-app/prod"), "{frame}");
        assert!(frame.contains("    Profile prod"), "{frame}");
        assert!(frame.contains("profile"), "{frame}");
    }

    #[test]
    fn selecting_current_context_is_noop() {
        let mut app = test_app();
        app.context_switcher.last_error = Some("transient".to_string());

        app.open_context_picker();
        app.handle_key(key(KeyCode::Enter));

        assert_eq!(app.mode, TuiMode::ContextPicker);
        assert!(app.context_switcher.last_error.is_none());
        assert!(!app.context_switcher.switch_running);
        assert!(!app.context_switcher.environment_list_running);
        assert!(app.context_switcher.pending_action.is_none());
    }

    #[test]
    fn context_picker_navigation_ignores_group_headers() {
        let mut app = test_app();
        app.context_switcher = ContextSwitcherState::new(vec![
            test_current_context_target(),
            test_manifest_context_target("prod"),
            test_server_context_target("server:builtin:cloud", "Built-in cloud server", "cloud"),
        ]);

        app.open_context_picker();
        app.handle_key(key(KeyCode::Down));

        assert_eq!(app.context_switcher.selected, 1);
        let frame = render_app_text_at(&app, 140, 32);
        assert!(frame.contains(">   sample-app/prod"), "{frame}");
    }

    #[test]
    async fn tui_context_info_does_not_log_selected_context() {
        let fixture = test_launch_with_manifest().await;
        let log_context = LogContext::captured();

        log_context
            .scope(async {
                let _ = TuiContextInfo::from_context(fixture.launch.initial_context.as_ref());
            })
            .await;

        assert!(log_context.take_buffered_lines().is_empty());
    }

    #[test]
    async fn tui_context_creation_preserves_captured_log_output() {
        let fixture = test_launch_with_manifest().await;
        let global_flags = fixture.launch.base_flags.clone();
        let log_context = LogContext::captured();

        let context = log_context
            .scope(async {
                Context::new(global_flags, Some(Output::Captured))
                    .await
                    .expect("context")
            })
            .await;
        log_context
            .scope(async {
                let _ = context.manifest_environment();
                logln("captured marker");
            })
            .await;

        let logs = log_context.take_buffered_lines().join("\n");
        assert!(logs.contains("captured marker"), "{logs}");
    }

    #[test]
    async fn context_targets_dedup_builtin_servers_and_keep_custom_sources() {
        let fixture = test_launch_with_manifest().await;

        let targets = context_targets_from_launch(&fixture.launch).expect("context targets");
        let keys = targets
            .iter()
            .map(|target| target.key.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            keys.iter()
                .filter(|key| **key == "server:builtin:local")
                .count(),
            1
        );
        assert_eq!(
            keys.iter()
                .filter(|key| **key == "server:builtin:cloud")
                .count(),
            1
        );
        assert!(!keys.contains(&"server:profile:local"));
        assert!(!keys.contains(&"server:profile:cloud"));
        assert!(keys.contains(&"server:profile:prod"));
        assert!(
            !keys
                .iter()
                .any(|key| key.starts_with("server:manifest:local:"))
        );
        assert!(
            !keys
                .iter()
                .any(|key| key.starts_with("server:manifest:cloud:"))
        );
        assert!(
            !keys
                .iter()
                .any(|key| key.starts_with("server:manifest:implicit:"))
        );
        assert!(
            keys.iter()
                .any(|key| key.starts_with("server:manifest:custom:http://localhost:9881"))
        );
        assert!(keys.contains(&"manifest:local"));
        assert!(keys.contains(&"manifest:cloud"));
        assert!(keys.contains(&"manifest:implicit"));
        assert!(keys.contains(&"manifest:custom"));
    }

    #[test]
    fn context_environment_loading_renders_blocking_modal() {
        let mut app = test_app();
        app.open_context_picker();
        app.context_switcher.environment_list_running = true;
        app.context_switcher.environment_list_spinner_frame = 2;

        let frame = render_app_text_at(&app, 100, 30);

        assert!(frame.contains("Loading app environments"), "{frame}");
        assert!(frame.contains("esc cancel  q quit"), "{frame}");
        assert!(frame.contains("| Loading app environments"), "{frame}");
    }

    #[test]
    fn context_environment_loading_ignores_selection_input() {
        let mut app = test_app();
        app.context_switcher = ContextSwitcherState::new(vec![
            test_current_context_target(),
            test_manifest_context_target("prod"),
        ]);
        app.open_context_picker();
        app.context_switcher.environment_list_running = true;

        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Enter));

        assert_eq!(app.context_switcher.selected, 0);
        assert!(app.context_switcher.environment_list_running);
        assert!(app.context_switcher.pending_action.is_none());
        assert!(!app.context_switcher.switch_running);
    }

    #[test]
    fn context_environment_loading_cancel_ignores_stale_result() {
        let mut app = test_app();
        app.open_context_picker();
        app.context_switcher.environment_list_running = true;
        app.context_switcher.generation = 7;
        app.context_switcher.pending_key = Some("server:builtin:cloud".to_string());

        app.handle_key(key(KeyCode::Esc));

        assert!(!app.context_switcher.environment_list_running);
        assert_eq!(app.context_switcher.generation, 8);
        assert!(app.context_switcher.pending_key.is_none());
        assert_eq!(app.context_switcher.mode, ContextPickerStep::Targets);

        app.finish_context_environment_list(
            7,
            "server:builtin:cloud".to_string(),
            TuiContextTaskResult::new(TuiContextId::new(1), Ok(Vec::new()), Vec::new()),
        );

        assert!(app.context_switcher.app_environments.is_empty());
        assert_eq!(app.context_switcher.mode, ContextPickerStep::Targets);
    }

    #[test]
    fn context_environment_loading_q_quits() {
        let mut app = test_app();
        app.open_context_picker();
        app.context_switcher.environment_list_running = true;

        app.handle_key(key(KeyCode::Char('q')));

        assert!(app.should_quit);
    }

    #[test]
    fn context_picker_width_expands_but_is_bounded() {
        let mut state = ContextSwitcherState::new(vec![
            test_current_context_target(),
            test_server_context_target(
                "server:profile:prod",
                "Profile production with long descriptive name",
                "https://very-long-production-worker-service.internal.example.com:9443",
            ),
        ]);
        let rows = context_picker_rows(&state, state.row_count());

        assert_eq!(context_picker_width(&rows, 200), 120);
        assert_eq!(context_picker_width(&rows, 50), 48);

        state.show_app_environments(
            "server:profile:prod".to_string(),
            vec![TuiAppEnvironmentTarget {
                label: "large-application/prod".to_string(),
                detail: "deployment r42 v2026.07.01-production-release".to_string(),
                server_label: "Profile production with long descriptive name".to_string(),
                server_flags: GolemCliGlobalFlags::default(),
                environment_reference: None,
                app: Some("large-application".to_string()),
                environment: Some("prod".to_string()),
                dev_eligible: false,
                error: None,
            }],
        );
        let rows = context_picker_rows(&state, state.row_count());
        assert_eq!(context_picker_width(&rows, 200), 120);
    }

    #[test]
    fn context_picker_truncates_long_details_on_narrow_terminals() {
        let mut app = test_app();
        app.context_switcher = ContextSwitcherState::new(vec![
            test_current_context_target(),
            test_server_context_target(
                "server:profile:prod",
                "Profile production",
                "https://very-long-production-worker-service.internal.example.com:9443",
            ),
        ]);

        app.open_context_picker();
        let frame = render_app_text_at(&app, 80, 24);

        assert!(frame.contains("https://very-long"), "{frame}");
        assert!(frame.contains("..."), "{frame}");
        assert!(!frame.contains("internal.example.com:9443"), "{frame}");
    }

    #[test]
    fn context_picker_selection_confirms_while_command_runs() {
        let mut app = test_app();
        app.command_run = Some(CommandRun::new(
            1,
            CommandKind::Build,
            vec!["build".to_string()],
            CommandOptions::default(),
            "sample-app:local".to_string(),
        ));
        let (tx, _rx) = test_event_channel();

        app.open_context_picker();
        app.context_switcher
            .targets
            .push(test_manifest_context_target("prod"));
        app.context_switcher.show_targets();
        app.handle_key(key(KeyCode::Down));
        app.handle_key_with_events(key(KeyCode::Enter), Some(&tx));

        assert_eq!(app.mode, TuiMode::ContextSwitchConfirm);
        assert!(app.context_switcher.pending_action.is_some());
        assert!(app.context_switcher.last_error.is_none());
        assert!(!app.context_switcher.switch_running);

        let frame = render_app_text(&app);
        assert_eq!(
            frame
                .matches("Switching context will stop running dev jobs")
                .count(),
            1
        );
    }

    #[test]
    fn context_switch_confirmation_requests_graceful_command_stop() {
        let mut app = test_app();
        app.command_run = Some(CommandRun::new(
            1,
            CommandKind::Build,
            vec!["build".to_string()],
            CommandOptions::default(),
            "sample-app:local".to_string(),
        ));
        let (tx, _rx) = test_event_channel();

        app.open_context_picker();
        app.context_switcher
            .targets
            .push(test_manifest_context_target("prod"));
        app.context_switcher.show_targets();
        app.handle_key(key(KeyCode::Down));
        app.handle_key_with_events(key(KeyCode::Enter), Some(&tx));
        app.handle_key_with_events(key(KeyCode::Enter), Some(&tx));

        assert_eq!(
            app.command_run.as_ref().map(|run| run.status),
            Some(CommandStatus::Cancelling)
        );
        assert!(app.context_switcher.waiting_for_dev_stop);
    }

    #[test]
    fn context_switch_confirmation_leaves_global_server_running() {
        let mut app = test_app();
        app.command_run = Some(CommandRun::new(
            1,
            CommandKind::Build,
            vec!["build".to_string()],
            CommandOptions::default(),
            "sample-app:local".to_string(),
        ));
        app.server.run.status = ServerStatus::Running;
        let (tx, _rx) = test_event_channel();

        app.open_context_picker();
        app.context_switcher
            .targets
            .push(test_manifest_context_target("prod"));
        app.context_switcher.show_targets();
        app.handle_key(key(KeyCode::Down));
        app.handle_key_with_events(key(KeyCode::Enter), Some(&tx));
        app.handle_key_with_events(key(KeyCode::Enter), Some(&tx));

        assert_eq!(
            app.command_run.as_ref().map(|run| run.status),
            Some(CommandStatus::Cancelling)
        );
        assert_eq!(app.server.run.status, ServerStatus::Running);
    }

    #[test]
    fn context_switch_auto_closes_ops_refresh_without_confirmation() {
        let mut app = test_app();
        app.context_switcher = ContextSwitcherState::new(vec![
            test_current_context_target(),
            test_manifest_context_target("prod"),
        ]);
        app.agents.refresh_running = true;
        app.agents.refresh_generation = 7;

        app.open_context_picker();
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Enter));

        assert!(!app.agents.refresh_running);
        assert_eq!(app.agents.refresh_generation, 8);
        assert_ne!(app.mode, TuiMode::ContextSwitchConfirm);
        assert!(app.context_switcher.pending_action.is_none());
    }

    #[test]
    fn auth_prompt_events_track_suspended_auth_state() {
        let mut app = test_app();
        let (tx, _rx) = test_event_channel();
        let url = "https://auth.example.test/login".to_string();

        app.handle_event(
            TuiEvent::AuthPromptStarted {
                url: url.clone(),
                ready: dropped_auth_prompt_ready(),
            },
            &tx,
        );

        assert_eq!(
            app.auth_prompt.as_ref().map(|prompt| prompt.url.as_str()),
            Some(url.as_str())
        );
        let text = tui_auth_prompt_text(&url);
        assert!(text.contains("Authenticate with GitHub"), "{text}");
        assert!(text.contains(&url), "{text}");

        app.handle_event(TuiEvent::AuthPromptFinished, &tx);

        assert!(app.auth_prompt.is_none());
    }

    #[test]
    fn non_local_context_keeps_global_server_actions_available() {
        let mut app = test_app();
        app.context.uses_local_server = false;
        app.context.dev_eligible = false;

        assert_eq!(
            app.action_availability(action(TuiActionId::ToggleServer)),
            TuiActionAvailability::Available
        );

        app.handle_key(key(KeyCode::Char('s')));

        assert_eq!(app.server.run.status, ServerStatus::Starting);
        assert_eq!(app.active_workspace, TuiWorkspace::Dev);
        assert_eq!(app.dev_focus, DevPanel::Server);
    }

    #[test]
    fn context_picker_switch_does_not_stop_global_server() {
        let mut app = test_app();
        app.context_switcher = ContextSwitcherState::new(vec![
            test_current_context_target(),
            test_manifest_context_target("prod"),
        ]);
        app.server.run.status = ServerStatus::Running;

        app.open_context_picker();
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Enter));

        assert_ne!(app.mode, TuiMode::ContextSwitchConfirm);
        assert_eq!(app.server.run.status, ServerStatus::Running);
        assert!(app.context_switcher.pending_action.is_none());
    }

    #[test]
    fn app_environment_details_map_to_picker_targets() {
        let app = test_app();
        let server_target = TuiContextTarget {
            key: "server:cloud".to_string(),
            label: "Built-in cloud server".to_string(),
            detail: "cloud".to_string(),
            kind: TuiContextTargetKind::ServerTarget {
                source: TuiServerTargetSource::Builtin,
            },
            global_flags: GolemCliGlobalFlags::default(),
            error: None,
            is_current: false,
        };

        let targets = app_environment_targets_from_details(
            &server_target,
            vec![sample_environment_with_details("sample-app", "local")],
            &app.context,
        );

        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].label, "sample-app/local");
        assert!(targets[0].dev_eligible);
        assert!(matches!(
            targets[0].environment_reference,
            Some(EnvironmentReference::ApplicationEnvironment { .. })
        ));
    }

    #[test]
    fn ops_only_context_disables_dev_actions() {
        let mut app = test_app();
        app.context.dev_eligible = false;

        assert_eq!(
            app.action_availability(action(TuiActionId::Build)),
            TuiActionAvailability::Unavailable("selected context is ops-only")
        );

        app.handle_key(key(KeyCode::Char('b')));

        assert_eq!(app.active_workspace, TuiWorkspace::Dev);
        assert_eq!(app.dev_focus, DevPanel::Output);
        assert_eq!(
            app.command_run.as_ref().map(|run| run.status),
            Some(CommandStatus::Failed)
        );
    }

    #[test]
    fn context_picker_escape_returns_from_app_environments_to_targets() {
        let mut app = test_app();
        app.open_context_picker();
        app.context_switcher.show_app_environments(
            "current".to_string(),
            vec![TuiAppEnvironmentTarget {
                label: "sample-app/local".to_string(),
                detail: "deployment r1 v1".to_string(),
                server_label: "Current launch context".to_string(),
                server_flags: GolemCliGlobalFlags::default(),
                environment_reference: None,
                app: Some("sample-app".to_string()),
                environment: Some("local".to_string()),
                dev_eligible: true,
                error: None,
            }],
        );

        app.handle_key(key(KeyCode::Esc));

        assert_eq!(app.mode, TuiMode::ContextPicker);
        assert_eq!(app.context_switcher.mode, ContextPickerStep::Targets);
    }

    #[test]
    fn selected_context_args_prefix_nested_cli_specs() {
        let mut app = test_app();
        app.context_cli_args = vec![
            "--config-dir".to_string(),
            "/tmp/golem-config".to_string(),
            "--local".to_string(),
        ];

        let spec = app
            .command_spec(vec!["agent".to_string(), "list".to_string()])
            .expect("spec");

        assert_eq!(
            spec.args,
            vec![
                "--config-dir",
                "/tmp/golem-config",
                "--local",
                "agent",
                "list"
            ]
        );
    }

    #[test]
    fn local_server_spec_uses_launch_scoped_args_not_selected_context_args() {
        let mut app = test_app();
        app.context_cli_args = vec![
            "--config-dir".to_string(),
            "/tmp/selected-config".to_string(),
            "--cloud".to_string(),
        ];
        app.server.cli_args = vec![
            "--config-dir".to_string(),
            "/tmp/launch-config".to_string(),
            "--app-manifest-path".to_string(),
            "/tmp/app/golem.yaml".to_string(),
        ];

        let spec = app
            .server_spec(vec!["server".to_string(), "run".to_string()])
            .expect("spec");

        assert_eq!(
            spec.args,
            vec![
                "--config-dir",
                "/tmp/launch-config",
                "--app-manifest-path",
                "/tmp/app/golem.yaml",
                "server",
                "run"
            ]
        );
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
        app.active_workspace = TuiWorkspace::Ops;
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
        app.active_workspace = TuiWorkspace::Ops;
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
        app.active_workspace = TuiWorkspace::Ops;
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
        assert!(frame.contains("Refresh Agents"), "{frame}");
    }

    #[test]
    fn executes_palette_action() {
        let mut app = test_app();

        app.handle_key(modified_key(KeyCode::Char('p'), KeyModifiers::CONTROL));
        for character in "go ops".chars() {
            app.handle_key(key(KeyCode::Char(character)));
        }
        app.handle_key(key(KeyCode::Enter));

        let frame = render_app_text(&app);
        assert_eq!(app.active_workspace, TuiWorkspace::Ops);
        assert!(frame.contains("agents"), "{frame}");
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
        let frame = render_app_text_at(&app, 140, 30);
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
        assert!(frame.contains("Home"), "{frame}");
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
        assert_eq!(app.active_workspace, TuiWorkspace::Dev);
        assert_eq!(app.dev_focus, DevPanel::Output);
        assert_eq!(app.mode, TuiMode::CommandInteraction);
        assert_eq!(run.kind, CommandKind::Build);
        assert_eq!(run.args, vec!["build"]);
    }

    #[test]
    fn clean_key_switches_to_output_and_records_command() {
        let mut app = test_app();

        app.handle_key(key(KeyCode::Char('c')));

        let run = app.command_run.as_ref().expect("missing command run");
        assert_eq!(app.active_workspace, TuiWorkspace::Dev);
        assert_eq!(app.dev_focus, DevPanel::Output);
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
        assert_eq!(app.active_workspace, TuiWorkspace::Dev);
        assert_eq!(app.dev_focus, DevPanel::Output);
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
        app.active_workspace = TuiWorkspace::Dev;

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
        app.active_workspace = TuiWorkspace::Dev;
        app.dev_focus = DevPanel::Output;
        app.command_run = Some(CommandRun::new(
            1,
            CommandKind::Build,
            vec!["build".to_string()],
            CommandOptions::default(),
            "sample-app:local".to_string(),
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

        app.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::ScrollUp,
                column: 10,
                row: 10,
                modifiers: KeyModifiers::empty(),
            },
            None,
        );
        assert_eq!(
            app.command_run.as_ref().map(|run| run.output.scroll_offset),
            Some(4)
        );

        app.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::ScrollDown,
                column: 10,
                row: 10,
                modifiers: KeyModifiers::empty(),
            },
            None,
        );
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

        app.open_dev_workspace(DevPanel::Server);
        let frame = render_app_text(&app);

        assert_eq!(app.active_workspace, TuiWorkspace::Dev);
        assert_eq!(app.dev_focus, DevPanel::Server);
        assert!(frame.contains("server"), "{frame}");
        assert!(frame.contains("stopped"), "{frame}");
        assert!(frame.contains("clean:off"), "{frame}");
    }

    #[test]
    fn server_clean_toggle_affects_next_start() {
        let mut app = test_app();
        app.active_workspace = TuiWorkspace::Dev;

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
        app.active_workspace = TuiWorkspace::Dev;

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
        assert_eq!(app.active_workspace, TuiWorkspace::Dev);
        assert_eq!(app.dev_focus, DevPanel::Server);
        assert_eq!(app.server.run.status, ServerStatus::Starting);

        app.server.run.status = ServerStatus::Running;
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.server.run.status, ServerStatus::Stopping);
    }

    #[test]
    fn server_palette_action_switches_to_server_and_toggles() {
        let mut app = test_app();

        app.execute_action(TuiActionKind::ToggleServer, None);

        assert_eq!(app.active_workspace, TuiWorkspace::Dev);
        assert_eq!(app.dev_focus, DevPanel::Server);
        assert_eq!(app.server.run.status, ServerStatus::Starting);
    }

    #[test]
    fn server_logs_are_separate_from_command_output() {
        let mut app = test_app();

        app.handle_key(key(KeyCode::Char('b')));
        app.append_command_output(b"command log\n");
        app.server.run.output.append(b"server log\n");

        app.active_workspace = TuiWorkspace::Dev;
        app.dev_focus = DevPanel::Output;
        let output_frame = render_app_text_at(&app, 80, 24);
        assert!(output_frame.contains("command log"), "{output_frame}");
        assert!(!output_frame.contains("server log"), "{output_frame}");

        app.active_workspace = TuiWorkspace::Dev;
        app.dev_focus = DevPanel::Server;
        let server_frame = render_app_text_at(&app, 80, 24);
        assert!(server_frame.contains("server log"), "{server_frame}");
        assert!(!server_frame.contains("command log"), "{server_frame}");
    }

    #[test]
    fn server_mouse_scrolls_logs() {
        let mut app = test_app();
        app.active_workspace = TuiWorkspace::Dev;
        app.dev_focus = DevPanel::Server;
        for index in 0..20 {
            app.server
                .run
                .output
                .append(format!("server line {index}\n").as_bytes());
        }

        app.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::ScrollUp,
                column: 10,
                row: 10,
                modifiers: KeyModifiers::empty(),
            },
            None,
        );

        assert_eq!(app.server.run.output.scroll_offset, 3);
    }

    #[test]
    fn mouse_wheel_scrolls_panel_under_pointer_independent_of_focus() {
        let mut app = test_app();
        app.active_workspace = TuiWorkspace::Dev;
        app.dev_focus = DevPanel::Server;
        app.command_run = Some(CommandRun::new(
            1,
            CommandKind::Build,
            vec!["build".to_string()],
            CommandOptions::default(),
            "sample-app:local".to_string(),
        ));
        app.set_command_status(CommandStatus::Succeeded);
        for index in 0..20 {
            app.append_command_output(format!("output line {index}\n").as_bytes());
            app.server
                .run
                .output
                .append(format!("server line {index}\n").as_bytes());
        }
        render_app_text_at(&app, 120, 32);
        let output = snapshot_region(&app, RegionKind::DevPanelBody(DevPanel::Output));

        app.handle_mouse(
            mouse(MouseEventKind::ScrollUp, output.x + 2, output.y + 1),
            None,
        );

        assert_eq!(
            app.command_run.as_ref().map(|run| run.output.scroll_offset),
            Some(3)
        );
        assert_eq!(app.server.run.output.scroll_offset, 0);
    }

    #[test]
    fn mouse_wheel_scrolls_server_drawer_from_any_workspace() {
        let mut app = test_app();
        app.active_workspace = TuiWorkspace::Home;
        app.layout.server_drawer_open = true;
        for index in 0..20 {
            app.server
                .run
                .output
                .append(format!("server line {index}\n").as_bytes());
        }
        render_app_text_at(&app, 120, 32);
        let drawer = snapshot_region(&app, RegionKind::ServerDrawer);

        app.handle_mouse(
            mouse(MouseEventKind::ScrollUp, drawer.x + 2, drawer.y + 2),
            None,
        );

        assert_eq!(app.server.run.output.scroll_offset, 3);
    }

    #[test]
    fn mouse_click_workspace_tab_switches_workspace() {
        let mut app = test_app();
        render_app_text_at(&app, 120, 32);
        let dev_tab = snapshot_region(&app, RegionKind::HeaderTab(TuiWorkspace::Dev));

        app.handle_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                dev_tab.x + 1,
                dev_tab.y,
            ),
            None,
        );

        assert_eq!(app.active_workspace, TuiWorkspace::Dev);
    }

    #[test]
    fn mouse_click_dev_panel_focuses_panel() {
        let mut app = test_app();
        app.active_workspace = TuiWorkspace::Dev;
        render_app_text_at(&app, 120, 32);
        let output = snapshot_region(&app, RegionKind::DevPanelBody(DevPanel::Output));

        app.handle_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                output.x + 2,
                output.y + 1,
            ),
            None,
        );

        assert_eq!(app.dev_focus, DevPanel::Output);
    }

    #[test]
    fn mouse_click_agent_row_selects_agent() {
        let mut app = test_app();
        app.active_workspace = TuiWorkspace::Ops;
        app.agents.agents = sample_agents();
        render_app_text_at(&app, 120, 32);
        let list = snapshot_region(&app, RegionKind::OpsList);

        app.handle_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                list.x + 4,
                list.y + 1,
            ),
            None,
        );

        assert_eq!(app.agents.selected, 1);
    }

    #[test]
    fn mouse_click_agent_inspect_pane_changes_focus() {
        let mut app = inspect_app();
        render_app_text_at(&app, 120, 32);
        let stream = snapshot_region(&app, RegionKind::OpsInspectPane(AgentInspectPane::Stream));

        app.handle_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                stream.x + 2,
                stream.y + 1,
            ),
            None,
        );

        assert_eq!(app.agents.inspect.focus, AgentInspectPane::Stream);
    }

    #[test]
    fn mouse_click_context_confirm_and_cancel_regions_match_keyboard_actions() {
        let mut confirm_app = test_app();
        confirm_app.mode = TuiMode::ContextSwitchConfirm;
        confirm_app.command_run = Some(CommandRun::new(
            1,
            CommandKind::Build,
            vec!["build".to_string()],
            CommandOptions::default(),
            "sample-app:local".to_string(),
        ));
        confirm_app.context_switcher.pending_action = Some(PendingContextSwitchAction::Switch {
            global_flags: GolemCliGlobalFlags::default(),
            environment_reference: None,
        });
        render_app_text_at(&confirm_app, 120, 32);
        let confirm = snapshot_region(&confirm_app, RegionKind::ContextConfirm);

        confirm_app.handle_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                confirm.x + 1,
                confirm.y,
            ),
            None,
        );

        assert!(confirm_app.context_switcher.waiting_for_dev_stop);

        let mut cancel_app = test_app();
        cancel_app.mode = TuiMode::ContextSwitchConfirm;
        cancel_app.context_switcher.pending_action = Some(PendingContextSwitchAction::Switch {
            global_flags: GolemCliGlobalFlags::default(),
            environment_reference: None,
        });
        render_app_text_at(&cancel_app, 120, 32);
        let cancel = snapshot_region(&cancel_app, RegionKind::ContextCancel);

        cancel_app.handle_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                cancel.x + 1,
                cancel.y,
            ),
            None,
        );

        assert_eq!(cancel_app.mode, TuiMode::ContextPicker);
        assert!(cancel_app.context_switcher.pending_action.is_none());
    }

    #[test]
    fn mouse_drag_dev_split_changes_session_ratio() {
        let mut app = test_app();
        app.active_workspace = TuiWorkspace::Dev;
        render_app_text_at(&app, 120, 32);
        let before = app.layout.dev_primary_ratio;
        let split = snapshot_region(&app, RegionKind::DevPrimarySplit);

        app.handle_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                split.x,
                split.y + 1,
            ),
            None,
        );
        app.handle_mouse(
            mouse(
                MouseEventKind::Drag(MouseButton::Left),
                split.x + 12,
                split.y + 1,
            ),
            None,
        );
        app.handle_mouse(
            mouse(
                MouseEventKind::Up(MouseButton::Left),
                split.x + 12,
                split.y + 1,
            ),
            None,
        );

        assert_ne!(app.layout.dev_primary_ratio, before);
        assert!(app.layout.dragging.is_none());
    }

    #[test]
    fn mouse_drag_drawer_split_changes_session_width() {
        let mut app = test_app();
        app.layout.server_drawer_open = true;
        render_app_text_at(&app, 120, 32);
        let before = app.layout.server_drawer_ratio;
        let split = snapshot_region(&app, RegionKind::ServerDrawerSplit);

        app.handle_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                split.x,
                split.y + 1,
            ),
            None,
        );
        app.handle_mouse(
            mouse(
                MouseEventKind::Drag(MouseButton::Left),
                split.x - 10,
                split.y + 1,
            ),
            None,
        );
        app.handle_mouse(
            mouse(
                MouseEventKind::Up(MouseButton::Left),
                split.x - 10,
                split.y + 1,
            ),
            None,
        );

        assert_ne!(app.layout.server_drawer_ratio, before);
        assert!(app.layout.dragging.is_none());
    }

    #[test]
    fn repl_key_switches_to_repl_and_records_command() {
        let mut app = test_app();

        app.handle_key(key(KeyCode::Char('r')));

        assert_eq!(app.active_workspace, TuiWorkspace::Dev);
        assert_eq!(app.dev_focus, DevPanel::Repl);
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
            active_workspace: TuiWorkspace::Home,
            dev_focus: DevPanel::Repl,
            mode: TuiMode::Normal,
            palette: CommandPalette::default(),
            command_options: CommandOptions::default(),
            command_run: None,
            server: LocalServerService::default(),
            repl: ReplState::default(),
            agents: AgentsState::default(),
            next_command_id: 1,
            context: TuiContextInfo {
                application: "sample-app".to_string(),
                environment: "local".to_string(),
                server: "local".to_string(),
                config_dir: "/tmp/golem-config".to_string(),
                uses_local_server: true,
                dev_eligible: true,
            },
            context_switcher: ContextSwitcherState::new(vec![TuiContextTarget {
                key: "manifest:local".to_string(),
                label: "sample-app/local".to_string(),
                detail: "server: local".to_string(),
                kind: TuiContextTargetKind::ManifestAppContext {
                    app: "sample-app".to_string(),
                    environment: "local".to_string(),
                },
                global_flags: GolemCliGlobalFlags::default(),
                error: None,
                is_current: true,
            }]),
            context_cli_args: Vec::new(),
            selected_environment_reference: None,
            context_executor: None,
            auth_prompt: None,
            layout: TuiLayoutState::default(),
            layout_snapshot: RefCell::new(None),
        }
    }

    async fn test_launch_with_manifest() -> TestLaunchFixture {
        let original_dir =
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")));
        let _ = std::env::set_current_dir(env!("CARGO_MANIFEST_DIR"));
        let app_dir = TempDir::new().expect("app dir");
        let config_dir = TempDir::new().expect("config dir");
        let manifest_path = app_dir.path().join("golem.yaml");
        fs::write(
            &manifest_path,
            r#"
manifestVersion: 1.6.0
app: picker-app

environments:
  local:
    server: local
  cloud:
    server: cloud
  implicit: {}
  custom:
    server:
      url: http://localhost:9881
      workerUrl: http://localhost:9881
      allowInsecure: true
      auth:
        staticToken: token
"#,
        )
        .expect("write manifest");
        Config::set_profile(
            ProfileName("prod".to_string()),
            Profile {
                custom_url: Some(Url::parse("http://localhost:9882").expect("profile url")),
                custom_worker_url: None,
                allow_insecure: true,
                config: Default::default(),
                auth: AuthenticationConfig::static_builtin_local(),
            },
            config_dir.path(),
        )
        .expect("write profile");

        let command = GolemCliCommand::parse_from([
            "golem-cli",
            "--config-dir",
            config_dir.path().to_str().expect("config path"),
            "--app-manifest-path",
            manifest_path.to_str().expect("manifest path"),
            "--environment",
            "local",
            "tui",
        ]);
        let context = Arc::new(
            Context::new(command.global_flags.clone(), Some(Output::None))
                .await
                .expect("context"),
        );
        TestLaunchFixture {
            launch: TuiLaunchConfig::new(context, command.global_flags),
            app_dir,
            config_dir,
            original_dir,
        }
    }

    struct TestLaunchFixture {
        launch: TuiLaunchConfig,
        #[allow(dead_code)]
        app_dir: TempDir,
        #[allow(dead_code)]
        config_dir: TempDir,
        original_dir: PathBuf,
    }

    impl Drop for TestLaunchFixture {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.original_dir);
        }
    }

    fn test_current_context_target() -> TuiContextTarget {
        TuiContextTarget {
            key: "manifest:local".to_string(),
            label: "sample-app/local".to_string(),
            detail: "server: local".to_string(),
            kind: TuiContextTargetKind::ManifestAppContext {
                app: "sample-app".to_string(),
                environment: "local".to_string(),
            },
            global_flags: GolemCliGlobalFlags::default(),
            error: None,
            is_current: true,
        }
    }

    fn test_manifest_context_target(environment: &str) -> TuiContextTarget {
        TuiContextTarget {
            key: format!("manifest:{environment}"),
            label: format!("sample-app/{environment}"),
            detail: "server: cloud".to_string(),
            kind: TuiContextTargetKind::ManifestAppContext {
                app: "sample-app".to_string(),
                environment: environment.to_string(),
            },
            global_flags: GolemCliGlobalFlags::default(),
            error: None,
            is_current: false,
        }
    }

    fn test_server_context_target(key: &str, label: &str, detail: &str) -> TuiContextTarget {
        TuiContextTarget {
            key: key.to_string(),
            label: label.to_string(),
            detail: detail.to_string(),
            kind: TuiContextTargetKind::ServerTarget {
                source: TuiServerTargetSource::Profile,
            },
            global_flags: GolemCliGlobalFlags::default(),
            error: None,
            is_current: false,
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

    fn sample_environment_with_details(
        application_name: &str,
        environment_name: &str,
    ) -> EnvironmentWithDetails {
        EnvironmentWithDetails {
            environment: golem_common::model::environment::EnvironmentSummary {
                id: golem_common::model::environment::EnvironmentId(uuid::Uuid::nil()),
                revision: golem_common::model::environment::EnvironmentRevision::new(1).unwrap(),
                name: golem_common::model::environment::EnvironmentName(
                    environment_name.to_string(),
                ),
                diff_model_version: 0,
                compatibility_check: true,
                version_check: true,
                security_overrides: false,
                current_deployment: None,
            },
            application: golem_common::model::application::ApplicationSummary {
                id: golem_common::model::application::ApplicationId(uuid::Uuid::nil()),
                name: golem_common::model::application::ApplicationName(
                    application_name.to_string(),
                ),
            },
            account: golem_common::model::account::AccountSummary {
                id: golem_common::model::account::AccountId(uuid::Uuid::nil()),
                name: "test account".to_string(),
                email: golem_common::model::account::AccountEmail::new(
                    "test@example.com".to_string(),
                ),
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

    fn inspect_app() -> TuiApp {
        let mut app = test_app();
        app.active_workspace = TuiWorkspace::Ops;
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

    fn snapshot_region(app: &TuiApp, kind: RegionKind) -> Rect {
        app.layout_snapshot
            .borrow()
            .as_ref()
            .and_then(|snapshot| snapshot.region(kind))
            .unwrap_or_else(|| panic!("missing region {kind:?}"))
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::empty(),
        }
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
