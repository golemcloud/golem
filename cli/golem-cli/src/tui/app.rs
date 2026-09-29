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

use crate::agent_id_display::{
    AgentIdHighlightKind, format_agent_id_for_terminal, highlight_agent_id,
};
use crate::app::context::ApplicationContext;
use crate::auth::AuthPresenter;
use crate::command::GolemCliGlobalFlags;
use crate::command_handler::Handlers;
use crate::config::Config;
use crate::context::Context;
use crate::log::Output;
use crate::model::agent::{
    AgentListMode, AgentListPageCursor, AgentListRequest, AgentMetadataView,
    AgentsMetadataResponseView,
};
use crate::model::app::ApplicationSourceMode;
use crate::model::app_raw::{BuiltinServer, Server};
use crate::model::environment::{EnvironmentReference, EnvironmentResolveMode};
use crate::model::masking::Masked;
use crate::tui::TuiEvent;
use crate::tui::context_executor::{TuiContextExecutor, TuiContextId, TuiContextTaskResult};
use crate::tui::input::encode_key_for_pty;
use crate::tui::layout::{
    self, DragTarget, LayoutInput, LayoutSnapshot, RegionKind, TuiLayoutState,
    adaptive_data_popup_rect,
};
use crate::tui::nested_cli::{
    CommandExit, NestedCliRuntime, NestedCliSpec, NestedCliTarget, spawn_nested_cli,
};
use crate::tui::terminal::TerminalGuard;
use crate::tui::terminal_screen::TerminalScreen;
#[cfg(feature = "tui-preview")]
use crate::tui::visual::TuiVisualVariant;
use crate::tui::visual::{
    ChromeTextStyle, FooterLayoutStyle, HeaderMetadataStyle, HeaderSeparatorStyle, PaneEdgeStyle,
    PaneTitleStyle, active_style,
};
use crate::tui::visual::{TuiVisualStyle, with_style};
use crate::tui::widgets::{
    CellPolicy, CellTone, CollectionQueryBar, ColumnChooserState, CommandRow, ContentTableRow,
    ContextHeader, ContextPair, CursorCollectionState, DecisionTableRow, FieldRow, HelpRow,
    JsonDocument, KeyHint, Notice, NoticeKind, OutputLine, OverlayFrame, PaneChrome, PaneEnding,
    PaneFocus, PaneHeader, PaneHeaderStatus, PaneLayout, PaneRegion, PaneResizeDividers, PaneRole,
    PaneScrollbarSlot, PaneSpec, PaneStatusTone, PaneTable, PaneTableCell, PaneTableCellSpan,
    PaneTableColumn, PaneTableState, ScrollableDocument, SearchInput, SectionHeading,
    SelectableRow, ShortcutRow, StatusKind, StatusMarker, TableDecoration, WorkspaceItem,
    WorkspaceSelector, fit, key_span, pane_table_virtual_width, render_scrollbar,
    responsive_table_columns, shortcut_line,
};
use crossterm::event::{
    Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use futures_util::StreamExt;
use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;
use golem_client::model::{EnvironmentWithDetails, OAuth2WebflowData, ScanCursor};
use golem_common::model::AgentStatus;
use golem_common::model::agent::DeployedRegisteredAgentType;
use golem_common::model::oplog::OplogErrorKind;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, ScrollbarState, Wrap};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::io::{Write, stdout};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Instant, SystemTime};
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{
    mpsc::{self, Sender},
    oneshot,
};
use tokio::time::{Duration, sleep};

const TUI_EVENT_CHANNEL_CAPACITY: usize = 1024;
const CONTEXT_PICKER_RIGHT_PADDING: usize = 4;
const AGENT_PAGE_SIZE: u64 = 200;
const COMPLETED_JOB_LIMIT: usize = 20;
const FORCE_STOP_DELAY: Duration = Duration::from_secs(5);

type TuiEventSender = Sender<TuiEvent>;

pub async fn run(ctx: Arc<Context>, global_flags: GolemCliGlobalFlags) -> anyhow::Result<()> {
    let context_executor = Arc::new(TuiContextExecutor::new(ctx.clone()));
    let mut app = TuiApp::from_launch(TuiLaunchConfig::new(ctx.clone(), global_flags));
    app.context_executor = Some(context_executor);
    let mut terminal = TerminalGuard::enter()?;
    let (event_tx, mut event_rx) = mpsc::channel::<TuiEvent>(TUI_EVENT_CHANNEL_CAPACITY);
    spawn_terminal_event_reader(event_tx.clone());
    app.refresh_agents(Some(&event_tx));

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

    app.save_preferences();
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

fn spawn_agent_refresh_spinner(generation: u64, event_tx: TuiEventSender, stop: Arc<AtomicBool>) {
    tokio::spawn(async move {
        while !stop.load(Ordering::Relaxed) {
            sleep(Duration::from_millis(120)).await;
            if stop.load(Ordering::Relaxed) {
                return;
            }
            if event_channel_closed(
                event_tx.try_send(TuiEvent::AgentRefreshSpinnerTick { generation }),
            ) {
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
    ops_subject: OpsSubject,
    ops_view: OpsView,
    dev_primary: DevView,
    dev_focus: DevView,
    mode: TuiMode,
    leader: LeaderState,
    help_scroll: usize,
    help_content_height: Cell<usize>,
    help_viewport_height: Cell<usize>,
    palette: CommandPalette,
    command_options: CommandOptions,
    command_form: Option<CommandReviewForm>,
    command_form_memory: HashMap<String, CommandFormValues>,
    session_form: Option<SessionReviewForm>,
    server_form_memory: HashMap<String, ServerFormValues>,
    repl_form_memory: HashMap<String, ReplFormValues>,
    command_run: Option<CommandRun>,
    completed_jobs: VecDeque<CommandRun>,
    selected_job: usize,
    server: LocalServerService,
    pending_server_restart: Option<ServerFormValues>,
    repl: ReplState,
    pending_repl_restart: Option<ReplFormValues>,
    agents: AgentsState,
    ops_environment: OpsEnvironmentState,
    next_command_id: u64,
    context: TuiContextInfo,
    context_switcher: ContextSwitcherState,
    context_cli_args: Vec<String>,
    selected_environment_reference: Option<EnvironmentReference>,
    context_executor: Option<Arc<TuiContextExecutor>>,
    auth_prompt: Option<AuthPromptState>,
    layout: TuiLayoutState,
    layout_snapshot: RefCell<Option<LayoutSnapshot>>,
    preferences_path: PathBuf,
    preferences_error: Option<String>,
}

impl TuiApp {
    fn from_launch(launch: TuiLaunchConfig) -> Self {
        let context = TuiContextInfo::from_context(launch.initial_context.as_ref());
        let active_workspace = if context.dev_eligible && context.uses_local_server {
            TuiWorkspace::Dev
        } else {
            TuiWorkspace::Ops
        };
        let context_cli_args = context_args_from_flags(&launch.base_flags);
        let server = LocalServerService::from_launch(&launch);
        let preferences_path = tui_preferences_path(&context.config_dir);
        let (preferences, preferences_error) = load_tui_preferences(&preferences_path);
        let mut layout = TuiLayoutState::default();
        let mut agents = AgentsState::default();
        if let Some(preferences) = preferences {
            preferences.apply(&mut layout, &mut agents.table);
        }
        let targets = context_targets_from_launch(&launch).unwrap_or_else(|error| {
            vec![TuiContextTarget::error(format!(
                "failed to load context targets: {error:#}"
            ))]
        });
        Self {
            should_quit: false,
            active_workspace,
            ops_subject: OpsSubject::Root(OpsRootSubject::Agents),
            ops_view: OpsView::Overview,
            dev_primary: DevView::BuildDeploy,
            dev_focus: DevView::BuildDeploy,
            mode: TuiMode::Normal,
            leader: LeaderState::default(),
            help_scroll: 0,
            help_content_height: Cell::new(0),
            help_viewport_height: Cell::new(0),
            palette: CommandPalette::default(),
            command_options: CommandOptions::default(),
            command_form: None,
            command_form_memory: HashMap::new(),
            session_form: None,
            server_form_memory: HashMap::new(),
            repl_form_memory: HashMap::new(),
            command_run: None,
            completed_jobs: VecDeque::new(),
            selected_job: 0,
            server,
            pending_server_restart: None,
            repl: ReplState::default(),
            pending_repl_restart: None,
            agents,
            ops_environment: OpsEnvironmentState::default(),
            next_command_id: 1,
            context,
            context_switcher: ContextSwitcherState::new(targets),
            context_cli_args,
            selected_environment_reference: None,
            context_executor: None,
            auth_prompt: None,
            layout,
            layout_snapshot: RefCell::new(None),
            preferences_path,
            preferences_error,
        }
    }

    fn handle_event(&mut self, event: TuiEvent, event_tx: &TuiEventSender) {
        match event {
            TuiEvent::Terminal(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                self.handle_key_with_events(key, Some(event_tx));
            }
            TuiEvent::Terminal(Event::Resize(cols, rows)) => {
                self.resize_nested_sessions(cols, rows)
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
            TuiEvent::ServerOutput(bytes) => self.server.run.terminal.feed(&bytes),
            TuiEvent::ServerOutputClosed(error) => {
                if let Some(error) = error {
                    self.server
                        .run
                        .terminal
                        .feed_local_line(format!("server output closed: {error}"));
                }
            }
            TuiEvent::ServerExited(exit) => self.finish_server(exit, event_tx),
            TuiEvent::ServerSpinnerTick(server_id) => self.handle_server_spinner_tick(server_id),
            TuiEvent::ReplOutput(bytes) => self.append_repl_output(&bytes),
            TuiEvent::ReplOutputClosed(error) => self.handle_repl_output_closed(error),
            TuiEvent::ReplExited(exit) => self.finish_repl(exit, event_tx),
            TuiEvent::AgentRefreshTick => self.refresh_agents(Some(event_tx)),
            TuiEvent::AgentRefreshSpinnerTick { generation } => {
                if self.agents.refresh_running && generation == self.agents.refresh_generation {
                    self.agents.refresh_spinner_frame =
                        self.agents.refresh_spinner_frame.wrapping_add(1);
                }
            }
            TuiEvent::AgentRefreshFinished {
                generation,
                append,
                result,
            } => self.finish_agent_refresh(generation, append, result),
            TuiEvent::OpsEnvironmentRefreshFinished { generation, result } => {
                self.finish_ops_environment_refresh(generation, result)
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
            TuiMode::Leader => self.handle_leader_key(key, event_tx),
            TuiMode::Layout => self.handle_layout_key(key),
            TuiMode::Jobs => self.handle_jobs_key(key),
            TuiMode::QuitBlocked => self.handle_quit_blocked_key(key),
            TuiMode::Palette => self.handle_palette_key(key, event_tx),
            TuiMode::CommandForm => self.handle_command_form_key(key, event_tx),
            TuiMode::SessionForm => self.handle_session_form_key(key, event_tx),
            TuiMode::ContextPicker => self.handle_context_picker_key(key, event_tx),
            TuiMode::ContextSwitchConfirm => self.handle_context_switch_confirm_key(key, event_tx),
            TuiMode::Help => self.handle_help_key(key),
            TuiMode::AgentFilter => self.handle_agent_filter_key(key, event_tx),
            TuiMode::AgentDatasetFilter => self.handle_agent_dataset_filter_key(key, event_tx),
            TuiMode::AgentColumns => self.handle_agent_columns_key(key),
            TuiMode::CommandInteraction => self.handle_command_interaction_key(key),
            TuiMode::Repl => self.handle_repl_key(key),
            TuiMode::TerminalScrollback => self.handle_terminal_scrollback_key(key),
        }
    }

    fn handle_global_key(&mut self, key: KeyEvent, event_tx: Option<&TuiEventSender>) {
        if self.handle_modified_shortcut(key, event_tx) {
            return;
        }
        match key.code {
            KeyCode::Esc if self.agent_details_focused() => {
                self.agents.focus = AgentOverviewFocus::List
            }
            KeyCode::Esc if matches!(self.ops_subject, OpsSubject::Agent(_)) => {
                self.ops_subject = OpsSubject::Root(OpsRootSubject::Agents)
            }
            KeyCode::Enter if self.agent_list_focused() => self.open_selected_agent(),
            KeyCode::Up if self.ops_document_focused() => self.scroll_agent_details_up(1),
            KeyCode::Down if self.ops_document_focused() => self.scroll_agent_details_down(1),
            KeyCode::PageUp if self.ops_document_focused() => self.scroll_agent_details_up(10),
            KeyCode::PageDown if self.ops_document_focused() => self.scroll_agent_details_down(10),
            KeyCode::Home if self.ops_document_focused() => self.agents.details_scroll = 0,
            KeyCode::End if self.ops_document_focused() => self.scroll_agent_details_bottom(),
            KeyCode::Up if self.agent_list_focused() => self.select_previous_agent(),
            KeyCode::Down if self.agent_list_focused() => self.select_next_agent(),
            KeyCode::PageUp if self.agent_list_focused() => self.select_previous_agent_page(),
            KeyCode::PageDown if self.agent_list_focused() => self.select_next_agent_page(),
            KeyCode::Home if self.agent_list_focused() => self.select_first_agent(),
            KeyCode::End if self.agent_list_focused() => self.select_last_agent(),
            KeyCode::Up if self.agent_details_focused() => self.scroll_agent_details_up(1),
            KeyCode::Down if self.agent_details_focused() => self.scroll_agent_details_down(1),
            KeyCode::PageUp if self.agent_details_focused() => self.scroll_agent_details_up(10),
            KeyCode::PageDown if self.agent_details_focused() => self.scroll_agent_details_down(10),
            KeyCode::Home if self.agent_details_focused() => self.agents.details_scroll = 0,
            KeyCode::End if self.agent_details_focused() => self.scroll_agent_details_bottom(),
            KeyCode::Left if self.agent_list_focused() => self.agents.table.pan_left(),
            KeyCode::Right if self.agent_list_focused() => self.agents.table.pan_right_to_width(
                self.agents.table_virtual_width.get(),
                self.agents.table_viewport_width.get(),
            ),
            KeyCode::Tab => self.focus_next_pane(),
            KeyCode::BackTab => self.focus_previous_pane(),
            KeyCode::Backspace if self.agent_list_focused() && !self.agents.query.is_empty() => {
                self.mode = TuiMode::AgentFilter;
                self.agents.query.pop();
                self.agents.reset_selection();
            }
            KeyCode::Char(character)
                if self.agent_list_focused()
                    && (key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT)
                    && (!character.is_whitespace() || !self.agents.query.is_empty()) =>
            {
                self.mode = TuiMode::AgentFilter;
                self.agents.query.push(character);
                self.agents.reset_selection();
            }
            _ => {}
        }
    }

    fn handle_modified_shortcut(
        &mut self,
        key: KeyEvent,
        event_tx: Option<&TuiEventSender>,
    ) -> bool {
        match key.code {
            KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.request_quit();
            }
            KeyCode::Char('x') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.open_leader();
            }
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.open_palette(PaletteScope::All);
            }
            KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.open_help();
            }
            KeyCode::Char('r')
                if self.active_workspace == TuiWorkspace::Ops
                    && key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                match self.ops_subject.root() {
                    OpsRootSubject::Agents => self.refresh_agents(event_tx),
                    OpsRootSubject::Deployment | OpsRootSubject::Environment => {
                        self.refresh_ops_environment(event_tx)
                    }
                    OpsRootSubject::Server => {}
                }
            }
            KeyCode::Char(' ')
                if self.agent_list_focused() && key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.toggle_selected_agent();
            }
            KeyCode::Char('f')
                if self.agent_list_focused() && key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.open_agent_dataset_filter();
            }
            KeyCode::Char('a')
                if self.agent_list_focused() && key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.include_filtered_agents();
            }
            KeyCode::Char('n')
                if self.agent_list_focused() && key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.exclude_filtered_agents();
            }
            KeyCode::Char('l')
                if self.agent_list_focused() && key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.load_more_agents(event_tx);
            }
            _ => return false,
        }
        true
    }

    fn handle_palette_key(&mut self, key: KeyEvent, event_tx: Option<&TuiEventSender>) {
        if key.code == KeyCode::Char('p') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.palette.scope = PaletteScope::All;
            self.palette.selected = 0;
            return;
        }
        if key.code == KeyCode::Char('x') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.leader.return_mode = LeaderReturnMode::Palette;
            self.leader.group = None;
            self.mode = TuiMode::Leader;
            return;
        }
        if key.code == KeyCode::Char('h') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.open_help();
            return;
        }
        if key.code == KeyCode::Char('q') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.request_quit();
            return;
        }
        match key.code {
            KeyCode::Esc => self.close_palette(),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.close_palette();
            }
            KeyCode::Backspace => self.palette.backspace(),
            KeyCode::Down | KeyCode::Tab => {
                let action_count = self.filtered_actions().len();
                self.palette.next(action_count);
            }
            KeyCode::Up | KeyCode::BackTab => {
                let action_count = self.filtered_actions().len();
                self.palette.previous(action_count);
            }
            KeyCode::Enter => {
                if let Some(action) = self.filtered_actions().get(self.palette.selected).copied() {
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
                KeyCode::Char('q') | KeyCode::Char('c')
                    if key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    self.request_quit();
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
            KeyCode::Enter => {
                self.confirm_context_switch(event_tx);
            }
            KeyCode::Esc => {
                self.context_switcher.pending_action = None;
                self.context_switcher.waiting_for_dev_stop = false;
                self.mode = TuiMode::ContextPicker;
            }
            _ => {}
        }
    }

    fn handle_help_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.mode = TuiMode::Normal,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.mode = TuiMode::Normal;
            }
            KeyCode::Up => self.help_scroll = self.help_scroll.saturating_sub(1),
            KeyCode::Down => self.scroll_help_down(1),
            KeyCode::PageUp => self.help_scroll = self.help_scroll.saturating_sub(10),
            KeyCode::PageDown => self.scroll_help_down(10),
            KeyCode::Home => self.help_scroll = 0,
            KeyCode::End => {
                self.help_scroll = self
                    .help_content_height
                    .get()
                    .saturating_sub(self.help_viewport_height.get())
            }
            _ => {}
        }
    }

    fn open_help(&mut self) {
        self.help_scroll = 0;
        self.mode = TuiMode::Help;
    }

    fn scroll_help_down(&mut self, amount: usize) {
        let max = self
            .help_content_height
            .get()
            .saturating_sub(self.help_viewport_height.get());
        self.help_scroll = self.help_scroll.saturating_add(amount).min(max);
    }

    fn handle_agent_filter_key(&mut self, key: KeyEvent, event_tx: Option<&TuiEventSender>) {
        if self.handle_modified_shortcut(key, event_tx) {
            return;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Enter => self.mode = TuiMode::Normal,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.mode = TuiMode::Normal;
            }
            KeyCode::Backspace => {
                self.agents.query.pop();
                self.agents.reset_selection();
            }
            KeyCode::Up => self.select_previous_agent(),
            KeyCode::Down => self.select_next_agent(),
            KeyCode::Tab | KeyCode::BackTab => self.cycle_ops_pane_focus(),
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.agents.query.clear();
                self.agents.reset_selection();
            }
            KeyCode::Char(character)
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                self.agents.query.push(character);
                self.agents.reset_selection();
            }
            _ => {}
        }
    }

    fn handle_agent_dataset_filter_key(
        &mut self,
        key: KeyEvent,
        event_tx: Option<&TuiEventSender>,
    ) {
        if self.handle_modified_shortcut(key, event_tx) {
            return;
        }
        let Some(filter) = self.agents.dataset_filter.as_mut() else {
            self.mode = TuiMode::Normal;
            return;
        };
        match key.code {
            KeyCode::Esc => {
                self.agents.dataset_filter = None;
                self.mode = TuiMode::Normal;
            }
            KeyCode::Backspace => {
                filter.query.pop();
                filter.reset_filtered_selection();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                filter.query.clear();
                filter.reset_filtered_selection();
            }
            KeyCode::Up => filter.selected = filter.selected.saturating_sub(1),
            KeyCode::Down => {
                let choice_count = filter.filtered_indices().len();
                filter.selected = filter
                    .selected
                    .saturating_add(1)
                    .min(choice_count.saturating_sub(1));
            }
            KeyCode::Enter => {
                let selected = filter.selected_choice().cloned();
                self.agents.dataset_filter = None;
                self.mode = TuiMode::Normal;
                if let Some(selected) = selected
                    && selected != self.agents.dataset_scope
                {
                    self.agents.dataset_scope = selected;
                    self.reset_agent_dataset();
                    self.refresh_agents(event_tx);
                }
            }
            KeyCode::Char(character)
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                filter.query.push(character);
                filter.reset_filtered_selection();
            }
            _ => {}
        }
    }

    fn handle_leader_key(&mut self, key: KeyEvent, event_tx: Option<&TuiEventSender>) {
        if key.code == KeyCode::Esc {
            self.close_leader();
            return;
        }
        if key.code == KeyCode::Char('x') && key.modifiers.contains(KeyModifiers::CONTROL) {
            if self.leader.return_mode == LeaderReturnMode::Repl {
                if let Some(runtime) = self.repl.run.runtime.as_mut() {
                    let _ = runtime.write_all(&[24]);
                }
            }
            self.close_leader();
            return;
        }

        match self.leader.group {
            None => match key.code {
                KeyCode::Char('w') => self.leader.group = Some(LeaderGroup::Workspaces),
                KeyCode::Char('v') => self.leader.group = Some(LeaderGroup::Views),
                KeyCode::Char('l') => self.mode = TuiMode::Layout,
                KeyCode::Char('a') if self.leader.return_mode == LeaderReturnMode::Palette => {
                    self.palette.scope = PaletteScope::Context;
                    self.palette.selected = 0;
                    self.leader.group = None;
                    self.mode = TuiMode::Palette;
                }
                KeyCode::Char('a') => self.open_palette(PaletteScope::Context),
                KeyCode::Char('j') => self.open_jobs_palette(),
                KeyCode::Char('e') => self.open_context_picker(),
                _ => self.close_leader(),
            },
            Some(LeaderGroup::Workspaces) => match key.code {
                KeyCode::Char('n') => {
                    self.next_workspace();
                    self.close_leader();
                }
                KeyCode::Char('p') => {
                    self.previous_workspace();
                    self.close_leader();
                }
                KeyCode::Char('d') => {
                    self.active_workspace = TuiWorkspace::Dev;
                    self.close_leader();
                }
                KeyCode::Char('o') => {
                    self.open_ops_workspace(event_tx);
                    self.close_leader();
                }
                _ => self.close_leader(),
            },
            Some(LeaderGroup::Views) => {
                match (self.active_workspace, key.code) {
                    (TuiWorkspace::Dev, KeyCode::Char('b')) => {
                        self.open_dev_workspace(DevView::BuildDeploy)
                    }
                    (TuiWorkspace::Dev, KeyCode::Char('s')) => {
                        self.open_dev_workspace(DevView::Server)
                    }
                    (TuiWorkspace::Dev, KeyCode::Char('r')) => {
                        self.open_dev_workspace(DevView::Repl)
                    }
                    (TuiWorkspace::Ops, KeyCode::Char('o')) => self.ops_view = OpsView::Overview,
                    (TuiWorkspace::Ops, KeyCode::Char('a')) => self.ops_view = OpsView::Activity,
                    (TuiWorkspace::Ops, KeyCode::Char('m')) => self.ops_view = OpsView::Metrics,
                    _ => {}
                }
                self.close_leader();
            }
        }
    }

    fn handle_agent_columns_key(&mut self, key: KeyEvent) {
        let Some(chooser) = self.agents.column_chooser.as_mut() else {
            self.mode = TuiMode::Normal;
            return;
        };
        match key.code {
            KeyCode::Esc => {
                self.agents.column_chooser = None;
                self.mode = TuiMode::Normal;
            }
            KeyCode::Up => chooser.selected = chooser.selected.saturating_sub(1),
            KeyCode::Down => {
                chooser.selected = chooser
                    .selected
                    .saturating_add(1)
                    .min(AGENT_COLUMNS.len() - 1)
            }
            KeyCode::Char(' ')
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                chooser.toggle(&AGENT_COLUMNS, chooser.selected)
            }
            KeyCode::Enter => {
                if let Some(chooser) = self.agents.column_chooser.take() {
                    chooser.apply(&mut self.agents.table);
                    self.save_preferences();
                }
                self.mode = TuiMode::Normal;
            }
            _ => {}
        }
    }

    fn handle_mouse(&mut self, mouse: MouseEvent, event_tx: Option<&TuiEventSender>) {
        if self.mode == TuiMode::Help {
            match mouse.kind {
                MouseEventKind::ScrollUp => self.help_scroll = self.help_scroll.saturating_sub(3),
                MouseEventKind::ScrollDown => self.scroll_help_down(3),
                _ => {}
            }
            return;
        }
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
                self.save_preferences();
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
            Some(RegionKind::DevViewBody(DevView::BuildDeploy)) => {
                if up {
                    self.scroll_output_up_by(amount);
                    self.mode = TuiMode::TerminalScrollback;
                } else {
                    self.scroll_output_down_by(amount);
                }
            }
            Some(RegionKind::DevViewBody(DevView::Server)) => {
                if up {
                    self.scroll_server_up_by(amount);
                    self.mode = TuiMode::TerminalScrollback;
                } else {
                    self.scroll_server_down_by(amount);
                }
            }
            Some(RegionKind::DevViewBody(DevView::Repl)) => {
                if up {
                    self.repl.run.screen.scroll_up(amount);
                    self.mode = TuiMode::TerminalScrollback;
                } else {
                    self.repl.run.screen.scroll_down(amount);
                }
            }
            Some(RegionKind::OpsDetails) if self.ops_agents_focused() => {
                self.agents.focus = AgentOverviewFocus::Details;
                if up {
                    self.scroll_agent_details_up(amount);
                } else {
                    self.scroll_agent_details_down(amount);
                }
            }
            Some(RegionKind::OpsList) if self.ops_agents_focused() => {
                self.agents.focus = AgentOverviewFocus::List;
                self.scroll_agent_list(amount, up);
            }
            Some(RegionKind::OpsList) if self.ops_document_focused() => {
                if up {
                    self.scroll_agent_details_up(amount);
                } else {
                    self.scroll_agent_details_down(amount);
                }
            }
            _ => {}
        }
    }

    fn scroll_focused_region(&mut self, amount: usize, up: bool) {
        if self.dev_panel_focused(DevView::BuildDeploy) {
            if up {
                self.scroll_output_up_by(amount);
            } else {
                self.scroll_output_down_by(amount);
            }
        } else if self.dev_panel_focused(DevView::Server) {
            if up {
                self.scroll_server_up_by(amount);
            } else {
                self.scroll_server_down_by(amount);
            }
        } else if self.dev_panel_focused(DevView::Repl) {
            if up {
                self.repl.run.screen.scroll_up(amount);
            } else {
                self.repl.run.screen.scroll_down(amount);
            }
        } else if self.agent_details_focused() {
            if up {
                self.scroll_agent_details_up(amount);
            } else {
                self.scroll_agent_details_down(amount);
            }
        } else if self.ops_document_focused() {
            if up {
                self.scroll_agent_details_up(amount);
            } else {
                self.scroll_agent_details_down(amount);
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
            Some(RegionKind::Workspace(TuiWorkspace::Ops)) => self.open_ops_workspace(event_tx),
            Some(RegionKind::Workspace(workspace)) => self.active_workspace = workspace,
            Some(RegionKind::DevViewTitle(view)) | Some(RegionKind::DevViewBody(view)) => {
                self.active_workspace = TuiWorkspace::Dev;
                self.dev_focus = view;
            }
            Some(RegionKind::OpsList) if self.ops_agents_focused() => {
                self.agents.focus = AgentOverviewFocus::List;
                self.select_agent_at_row(y);
            }
            Some(RegionKind::OpsDetails) if self.ops_agents_focused() => {
                self.agents.focus = AgentOverviewFocus::Details
            }
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
                Some(RegionKind::OpsDetailsSplit) => Some(DragTarget::OpsDetails),
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
                let ratio = layout::dev_primary_ratio_from_pointer(
                    snapshot.workspace_body,
                    self.layout.dev_placement,
                    x,
                    y,
                );
                self.layout.dev_primary_ratio = layout::clamp_dev_primary_ratio(
                    snapshot.workspace_body,
                    self.layout.dev_placement,
                    ratio,
                );
            }
            DragTarget::DevSecondary => {
                let ratio = layout::dev_secondary_ratio_from_pointer(
                    snapshot.workspace_body,
                    self.layout.dev_placement,
                    x,
                    y,
                );
                self.layout.dev_secondary_ratio = layout::clamp_dev_secondary_ratio(
                    snapshot.workspace_body,
                    self.layout.dev_placement,
                    ratio,
                );
            }
            DragTarget::OpsDetails => {
                let area = snapshot.workspace_body;
                let ratio = layout::ops_details_ratio_from_pointer(
                    area,
                    self.layout.ops_details_placement,
                    x,
                    y,
                );
                self.layout.ops_details_ratio =
                    layout::clamp_ops_details_ratio(area, self.layout.ops_details_placement, ratio);
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
        let filtered = self.filtered_agents();
        if filtered.is_empty() {
            return;
        }
        let query_rows = CollectionQueryBar {
            dataset: &self.agents.dataset_label(),
            scope: self.agents.query_scope.label(),
            query: &self.agents.query,
            matched: filtered.len(),
            loaded: self.agents.agents.len(),
            more_available: self.agents.has_more(),
            editing: self.mode == TuiMode::AgentFilter,
        }
        .height();
        let notice_rows = u16::from(self.agents.last_error.is_some() && !filtered.is_empty());
        let data_y = list_area
            .y
            .saturating_add(query_rows)
            .saturating_add(notice_rows)
            .saturating_add(1);
        if y < data_y {
            return;
        }
        let owned_rows = filtered
            .iter()
            .map(|agent| agent_table_row(agent))
            .collect::<Vec<_>>();
        let row_cells = owned_rows
            .iter()
            .map(|row| row.iter().map(String::as_str).collect::<Vec<_>>())
            .collect::<Vec<_>>();
        let rows = row_cells.iter().map(Vec::as_slice).collect::<Vec<_>>();
        let mut table_state = self.agents.table.clone();
        table_state.selected = self.agents.selected.min(filtered.len().saturating_sub(1));
        let table = PaneTable {
            columns: &AGENT_COLUMNS,
            rows: &rows,
            state: &table_state,
            decoration: TableDecoration::Zebra,
            cell_tones: None,
            rich_cells: None,
            row_markers: None,
            first_row_index: 0,
        };
        let table_height = list_area.height.saturating_sub(query_rows + notice_rows);
        let window = table.window_from(table_height, self.agents.table_first_visible_row.get());
        let visual_line = y.saturating_sub(data_y) as usize;
        let Some(selected) = table.row_at_visual_line(window.start_row, visual_line) else {
            return;
        };
        self.agents.selected = selected;
        self.agents.table.selected = self.agents.selected;
        self.agents.details_scroll = 0;
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
            TuiActionKind::Plan => {
                self.close_palette();
                self.open_command_form(CommandKind::Plan);
            }
            TuiActionKind::Build => {
                self.close_palette();
                self.open_command_form(CommandKind::Build);
            }
            TuiActionKind::Deploy => {
                self.close_palette();
                self.open_command_form(CommandKind::Deploy);
            }
            TuiActionKind::Clean => {
                self.close_palette();
                self.open_command_form(CommandKind::Clean);
            }
            TuiActionKind::ToggleServer => {
                self.close_palette();
                self.request_server_toggle();
            }
            TuiActionKind::RestartServer => {
                self.close_palette();
                self.open_server_form(true, false);
            }
            TuiActionKind::CleanRestartServer => {
                self.close_palette();
                self.open_server_form(true, true);
            }
            TuiActionKind::FlipPanePlacement => {
                self.layout.dev_placement = self.layout.dev_placement.flipped();
                self.close_palette();
            }
            TuiActionKind::ToggleServerSubview => {
                let visible = self.layout.dev_is_visible(DevView::Server);
                self.layout.set_dev_visible(DevView::Server, !visible);
                self.close_palette();
            }
            TuiActionKind::RefreshAgents => {
                self.close_palette();
                self.refresh_agents(event_tx);
            }
            TuiActionKind::OpenAgentDatasetFilter => {
                self.close_palette();
                self.open_agent_dataset_filter();
            }
            TuiActionKind::OpenAgentColumns => {
                self.close_palette();
                self.agents.column_chooser = Some(ColumnChooserState::new(&self.agents.table));
                self.mode = TuiMode::AgentColumns;
            }
            TuiActionKind::IncludeAgentMatches => {
                self.include_filtered_agents();
                self.close_palette();
            }
            TuiActionKind::ExcludeAgentMatches => {
                self.exclude_filtered_agents();
                self.close_palette();
            }
            TuiActionKind::LoadMoreAgents => {
                self.close_palette();
                self.load_more_agents(event_tx);
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
                self.toggle_agent_details();
                self.close_palette();
            }
            TuiActionKind::CycleAgentMode => {
                self.cycle_agent_mode(event_tx);
                self.close_palette();
            }
            TuiActionKind::CycleOpsView => {
                self.ops_view = self.ops_view.next();
                self.close_palette();
            }
            TuiActionKind::CycleAgentFindScope => {
                self.agents.query_scope = self.agents.query_scope.next();
                self.agents.reset_selection();
                self.close_palette();
            }
            TuiActionKind::SelectOpsRoot(subject) => {
                self.close_palette();
                self.select_ops_root(subject, event_tx);
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
                self.open_repl_form(true);
            }
            TuiActionKind::OpenPalette => {
                self.open_palette(PaletteScope::All);
            }
            TuiActionKind::ShowHelp => {
                self.palette.reset();
                self.open_help();
            }
            TuiActionKind::Quit => {
                self.close_palette();
                self.request_quit();
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
            TuiActionId::Plan | TuiActionId::Build | TuiActionId::Deploy | TuiActionId::Clean
                if let Err(reason) = self.context.capability(DevView::BuildDeploy) =>
            {
                TuiActionAvailability::Unavailable(reason)
            }
            TuiActionId::StartOrFocusRepl | TuiActionId::RestartRepl
                if let Err(reason) = self.context.capability(DevView::Repl) =>
            {
                TuiActionAvailability::Unavailable(reason)
            }
            TuiActionId::Plan | TuiActionId::Build | TuiActionId::Deploy | TuiActionId::Clean
                if self.command_is_running() =>
            {
                TuiActionAvailability::Unavailable("command already running")
            }
            TuiActionId::RefreshAgents | TuiActionId::LoadMoreAgents
                if self.agents.refresh_running =>
            {
                TuiActionAvailability::Unavailable("refresh already running")
            }
            TuiActionId::RefreshAgents | TuiActionId::LoadMoreAgents
                if self.context_executor.is_none() =>
            {
                TuiActionAvailability::Unavailable("context executor unavailable")
            }
            TuiActionId::LoadMoreAgents if !self.agents.has_more() => {
                TuiActionAvailability::Unavailable("no additional agent batch")
            }
            TuiActionId::IncludeAgentMatches | TuiActionId::ExcludeAgentMatches
                if self.filtered_agents().is_empty() =>
            {
                TuiActionAvailability::Unavailable("no loaded agents match")
            }
            TuiActionId::OpenContextPicker if self.context_switch_busy() => {
                TuiActionAvailability::Unavailable("context switch already running")
            }
            TuiActionId::ToggleServer
            | TuiActionId::RestartServer
            | TuiActionId::CleanRestartServer
                if !self.server.available || self.context.capability(DevView::Server).is_err() =>
            {
                TuiActionAvailability::Unavailable("requires an available local server target")
            }
            _ => TuiActionAvailability::Available,
        }
    }

    fn action_target_summary(&self, action: &TuiAction) -> String {
        match action.target() {
            TuiActionTarget::Interface => "TUI session".to_string(),
            TuiActionTarget::ApplicationEnvironment => format!(
                "{}/{}/{}",
                self.context.application, self.context.environment, self.context.server
            ),
            TuiActionTarget::LocalServer => self.context.server.clone(),
            TuiActionTarget::LoadedAgents => {
                format!("{} loaded agents", self.filtered_agents().len())
            }
        }
    }

    fn open_palette(&mut self, scope: PaletteScope) {
        if self.mode != TuiMode::Palette {
            self.palette.reset();
            self.palette.return_mode = match self.mode {
                TuiMode::AgentFilter => PaletteReturnMode::AgentFilter,
                TuiMode::CommandForm => PaletteReturnMode::CommandForm,
                TuiMode::SessionForm => PaletteReturnMode::SessionForm,
                _ => PaletteReturnMode::Default,
            };
        }
        self.palette.scope = scope;
        self.mode = TuiMode::Palette;
    }

    fn open_jobs_palette(&mut self) {
        self.selected_job = 0;
        self.mode = TuiMode::Jobs;
    }

    fn filtered_actions(&self) -> Vec<TuiAction> {
        filtered_actions(&self.palette.query, self.palette.scope, self)
    }

    fn open_leader(&mut self) {
        self.leader = LeaderState {
            group: None,
            return_mode: match self.mode {
                TuiMode::AgentFilter => LeaderReturnMode::AgentFilter,
                TuiMode::Palette => LeaderReturnMode::Palette,
                TuiMode::CommandForm => LeaderReturnMode::CommandForm,
                TuiMode::SessionForm => LeaderReturnMode::SessionForm,
                TuiMode::Repl => LeaderReturnMode::Repl,
                TuiMode::TerminalScrollback => LeaderReturnMode::TerminalScrollback,
                _ => LeaderReturnMode::Normal,
            },
        };
        self.mode = TuiMode::Leader;
    }

    fn close_leader(&mut self) {
        self.mode = match self.leader.return_mode {
            LeaderReturnMode::Normal => self.default_mode(),
            LeaderReturnMode::AgentFilter => TuiMode::AgentFilter,
            LeaderReturnMode::Palette => TuiMode::Palette,
            LeaderReturnMode::CommandForm if self.command_form.is_some() => TuiMode::CommandForm,
            LeaderReturnMode::CommandForm => self.default_mode(),
            LeaderReturnMode::SessionForm if self.session_form.is_some() => TuiMode::SessionForm,
            LeaderReturnMode::SessionForm => self.default_mode(),
            LeaderReturnMode::Repl => TuiMode::Repl,
            LeaderReturnMode::TerminalScrollback => TuiMode::TerminalScrollback,
        };
        self.leader.group = None;
    }

    fn open_context_picker(&mut self) {
        self.context_switcher.last_error = None;
        self.context_switcher.clamp_selection();
        self.mode = TuiMode::ContextPicker;
    }

    fn close_palette(&mut self) {
        self.palette.reset();
        self.mode = match self.palette.return_mode {
            PaletteReturnMode::AgentFilter => TuiMode::AgentFilter,
            PaletteReturnMode::CommandForm if self.command_form.is_some() => TuiMode::CommandForm,
            PaletteReturnMode::SessionForm if self.session_form.is_some() => TuiMode::SessionForm,
            PaletteReturnMode::CommandForm
            | PaletteReturnMode::SessionForm
            | PaletteReturnMode::Default => self.default_mode(),
        };
    }

    fn default_mode(&self) -> TuiMode {
        if self.dev_panel_focused(DevView::BuildDeploy) && self.command_is_running() {
            TuiMode::CommandInteraction
        } else if self.dev_panel_focused(DevView::Repl) && self.repl.is_running() {
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

    fn focus_next_pane(&mut self) {
        match self.active_workspace {
            TuiWorkspace::Dev => {
                let visible = self.layout.visible_dev_views(self.dev_primary);
                let current = visible
                    .iter()
                    .position(|view| *view == self.dev_focus)
                    .unwrap_or(0);
                self.dev_focus = visible[(current + 1) % visible.len()];
            }
            TuiWorkspace::Ops => self.cycle_ops_pane_focus(),
        }
    }

    fn focus_previous_pane(&mut self) {
        match self.active_workspace {
            TuiWorkspace::Dev => {
                let visible = self.layout.visible_dev_views(self.dev_primary);
                let current = visible
                    .iter()
                    .position(|view| *view == self.dev_focus)
                    .unwrap_or(0);
                self.dev_focus = visible[(current + visible.len() - 1) % visible.len()];
            }
            TuiWorkspace::Ops => self.cycle_ops_pane_focus(),
        }
    }

    fn open_selected_agent(&mut self) {
        let identity = self
            .filtered_agents()
            .get(self.agents.selected)
            .map(|agent| agent.identity());
        if let Some(identity) = identity {
            self.ops_subject = OpsSubject::Agent(identity);
            self.ops_view = OpsView::Overview;
            self.agents.details_scroll = 0;
        }
    }

    fn select_ops_root(&mut self, subject: OpsRootSubject, event_tx: Option<&TuiEventSender>) {
        self.active_workspace = TuiWorkspace::Ops;
        self.ops_subject = OpsSubject::Root(subject);
        self.ops_view = OpsView::Overview;
        self.agents.details_scroll = 0;
        match subject {
            OpsRootSubject::Agents => {
                if self.agents.agents.is_empty() && !self.agents.refresh_running {
                    self.refresh_agents(event_tx);
                }
            }
            OpsRootSubject::Deployment | OpsRootSubject::Environment => {
                self.refresh_ops_environment(event_tx)
            }
            OpsRootSubject::Server => {}
        }
    }

    fn handle_layout_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Enter => {
                self.save_preferences();
                self.close_leader();
            }
            KeyCode::Tab => self.focus_next_pane(),
            KeyCode::BackTab => self.focus_previous_pane(),
            KeyCode::Char('f') => match self.active_workspace {
                TuiWorkspace::Dev => {
                    self.layout.dev_placement = self.layout.dev_placement.flipped()
                }
                TuiWorkspace::Ops => {
                    self.layout.ops_details_placement = self.layout.ops_details_placement.flipped()
                }
            },
            KeyCode::Char('b') if self.active_workspace == TuiWorkspace::Dev => {
                self.toggle_dev_subview(DevView::BuildDeploy)
            }
            KeyCode::Char('s') if self.active_workspace == TuiWorkspace::Dev => {
                self.toggle_dev_subview(DevView::Server)
            }
            KeyCode::Char('r') if self.active_workspace == TuiWorkspace::Dev => {
                self.toggle_dev_subview(DevView::Repl)
            }
            KeyCode::Char('d') if self.active_workspace == TuiWorkspace::Ops => {
                self.toggle_agent_details()
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down => {
                let cells = if key.modifiers.contains(KeyModifiers::SHIFT) {
                    5
                } else {
                    1
                };
                self.move_layout_divider(key.code, cells);
            }
            _ => {}
        }
    }

    fn handle_jobs_key(&mut self, key: KeyEvent) {
        let count = self.job_count();
        match key.code {
            KeyCode::Esc => self.mode = self.default_mode(),
            KeyCode::Up => self.selected_job = self.selected_job.saturating_sub(1),
            KeyCode::Down => {
                self.selected_job = self
                    .selected_job
                    .saturating_add(1)
                    .min(count.saturating_sub(1))
            }
            KeyCode::Enter if count > 0 => {
                self.open_dev_workspace(DevView::BuildDeploy);
                self.mode = self.default_mode();
            }
            _ => {}
        }
    }

    fn handle_quit_blocked_key(&mut self, key: KeyEvent) {
        if matches!(key.code, KeyCode::Esc | KeyCode::Enter) {
            self.mode = self.default_mode();
        }
    }

    fn request_quit(&mut self) {
        if self.has_active_work() {
            self.mode = TuiMode::QuitBlocked;
        } else {
            self.should_quit = true;
        }
    }

    fn has_active_work(&self) -> bool {
        self.command_is_running() || self.server.run.is_running() || self.repl.is_running()
    }

    fn job_count(&self) -> usize {
        usize::from(self.command_run.is_some()) + self.completed_jobs.len()
    }

    fn toggle_dev_subview(&mut self, view: DevView) {
        if view == self.dev_primary {
            for other in DevView::ALL {
                if other != self.dev_primary {
                    self.layout.set_dev_visible(other, false);
                }
            }
            self.dev_focus = self.dev_primary;
            return;
        }
        let visible = !self.layout.dev_is_visible(view);
        self.layout.set_dev_visible(view, visible);
        if visible {
            self.dev_focus = view;
        } else if self.dev_focus == view {
            self.dev_focus = self.dev_primary;
        }
    }

    fn move_layout_divider(&mut self, direction: KeyCode, cells: u16) {
        let Some(area) = self
            .layout_snapshot
            .borrow()
            .as_ref()
            .map(|snapshot| snapshot.workspace_body)
        else {
            return;
        };
        match self.active_workspace {
            TuiWorkspace::Ops if self.agent_details_open() => {
                let placement = self.layout.ops_details_placement;
                let delta = physical_ratio_delta(area, placement, direction, cells);
                if delta != 0 {
                    let requested = apply_ratio_delta(self.layout.ops_details_ratio, delta);
                    self.layout.ops_details_ratio =
                        layout::clamp_ops_details_ratio(area, placement, requested);
                }
            }
            TuiWorkspace::Dev => {
                let placement = self.layout.dev_placement;
                let visible = self.layout.visible_dev_views(self.dev_primary);
                if visible.len() < 2 {
                    return;
                }
                let primary_delta = physical_ratio_delta(area, placement, direction, cells);
                if primary_delta != 0 {
                    let requested = apply_ratio_delta(self.layout.dev_primary_ratio, primary_delta);
                    self.layout.dev_primary_ratio =
                        layout::clamp_dev_primary_ratio(area, placement, requested);
                    return;
                }
                if visible.len() == 3 {
                    let cross = placement.flipped();
                    let secondary_delta = physical_ratio_delta(area, cross, direction, cells);
                    if secondary_delta != 0 {
                        let requested =
                            apply_ratio_delta(self.layout.dev_secondary_ratio, secondary_delta);
                        self.layout.dev_secondary_ratio =
                            layout::clamp_dev_secondary_ratio(area, placement, requested);
                    }
                }
            }
            TuiWorkspace::Ops => {}
        }
    }

    fn open_dev_workspace(&mut self, panel: DevView) {
        self.active_workspace = TuiWorkspace::Dev;
        if panel != self.dev_primary && !self.layout.dev_is_visible(panel) {
            self.layout.set_dev_visible(self.dev_primary, false);
            self.dev_primary = panel;
            self.layout.set_dev_visible(panel, true);
        }
        self.dev_focus = panel;
    }

    fn open_ops_workspace(&mut self, event_tx: Option<&TuiEventSender>) {
        self.active_workspace = TuiWorkspace::Ops;
        match self.ops_subject.root() {
            OpsRootSubject::Agents => {
                if self.agents.agents.is_empty() && !self.agents.refresh_running {
                    self.refresh_agents(event_tx);
                }
            }
            OpsRootSubject::Deployment | OpsRootSubject::Environment
                if self.ops_environment.snapshot.is_none() && !self.ops_environment.loading =>
            {
                self.refresh_ops_environment(event_tx)
            }
            OpsRootSubject::Deployment | OpsRootSubject::Environment | OpsRootSubject::Server => {}
        }
    }

    fn dev_panel_focused(&self, panel: DevView) -> bool {
        self.active_workspace == TuiWorkspace::Dev && self.dev_focus == panel
    }

    fn ops_agents_focused(&self) -> bool {
        self.active_workspace == TuiWorkspace::Ops
            && matches!(self.ops_subject, OpsSubject::Root(OpsRootSubject::Agents))
            && self.ops_view == OpsView::Overview
    }

    fn agents_focused(&self) -> bool {
        self.ops_agents_focused()
    }

    fn ops_document_focused(&self) -> bool {
        self.active_workspace == TuiWorkspace::Ops
            && self.ops_view == OpsView::Overview
            && !matches!(self.ops_subject, OpsSubject::Root(OpsRootSubject::Agents))
    }

    fn agent_details_open(&self) -> bool {
        self.agents_focused()
            && self.agents.detail_visible
            && self.agent_details_layout_area().is_none_or(|area| {
                layout::ops_details_visible(area, true, self.layout.ops_details_placement)
            })
    }

    fn agent_details_layout_area(&self) -> Option<Rect> {
        let snapshot = self.layout_snapshot.borrow();
        let snapshot = snapshot.as_ref()?;
        if self.ops_agents_focused() {
            Some(snapshot.workspace_body)
        } else {
            None
        }
    }

    fn agent_list_focused(&self) -> bool {
        self.agents_focused()
            && (!self.agent_details_open() || self.agents.focus == AgentOverviewFocus::List)
    }

    fn agent_details_focused(&self) -> bool {
        self.agent_details_open() && self.agents.focus == AgentOverviewFocus::Details
    }

    fn cycle_ops_pane_focus(&mut self) {
        match self.ops_view {
            OpsView::Overview if self.agent_details_open() => {
                self.agents.focus = match self.agents.focus {
                    AgentOverviewFocus::List => AgentOverviewFocus::Details,
                    AgentOverviewFocus::Details => AgentOverviewFocus::List,
                };
                if self.agents.focus == AgentOverviewFocus::Details {
                    self.mode = TuiMode::Normal;
                }
            }
            OpsView::Metrics => {}
            OpsView::Overview => {}
            OpsView::Activity => {}
        }
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
                self.invalidate_ops_environment();
                self.context_switcher.last_error = None;
                self.mode = self.default_mode();
                if self.active_workspace == TuiWorkspace::Ops {
                    match self.ops_subject.root() {
                        OpsRootSubject::Agents => self.refresh_agents(Some(event_tx)),
                        OpsRootSubject::Deployment | OpsRootSubject::Environment => {
                            self.refresh_ops_environment(Some(event_tx))
                        }
                        OpsRootSubject::Server => {}
                    }
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
        self.agents.reset_selection();
        self.agents.included.clear();
        self.agents.dataset_scope = AgentDatasetScope::All;
        self.agents.dataset_filter = None;
        self.agents.query.clear();
        self.agents.query_scope = AgentQueryScope::AgentId;
        self.agents.focus = AgentOverviewFocus::List;
        self.agents.details_scroll = 0;
        self.agents.last_error = None;
        self.agents.metadata_error = None;
        self.agents.agents.clear();
        self.agents.agent_types.clear();
        self.agents.paging.reset();
        self.agents.stop_refresh_spinner();
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
            self.agents.table.selected = self.agents.selected;
            self.agents.details_scroll = 0;
            self.agents.reveal_selection(count);
        }
    }

    fn select_previous_agent(&mut self) {
        self.agents.selected = self.agents.selected.saturating_sub(1);
        self.agents.table.selected = self.agents.selected;
        self.agents.details_scroll = 0;
        self.agents.reveal_selection(self.filtered_agents().len());
    }

    fn select_next_agent_page(&mut self) {
        let count = self.filtered_agents().len();
        if count > 0 {
            self.agents.selected = self.agents.selected.saturating_add(10).min(count - 1);
            self.agents.table.selected = self.agents.selected;
            self.agents.details_scroll = 0;
            self.agents.reveal_selection(count);
        }
    }

    fn select_previous_agent_page(&mut self) {
        self.agents.selected = self.agents.selected.saturating_sub(10);
        self.agents.table.selected = self.agents.selected;
        self.agents.details_scroll = 0;
        self.agents.reveal_selection(self.filtered_agents().len());
    }

    fn scroll_agent_list(&mut self, amount: usize, up: bool) {
        let count = self.filtered_agents().len();
        if count == 0 {
            return;
        }
        self.agents.selected = if up {
            self.agents.selected.saturating_sub(amount)
        } else {
            self.agents
                .selected
                .saturating_add(amount)
                .min(count.saturating_sub(1))
        };
        self.agents.table.selected = self.agents.selected;
        self.agents.details_scroll = 0;
        self.agents.reveal_selection(count);
    }

    fn select_first_agent(&mut self) {
        self.agents.reset_selection();
    }

    fn select_last_agent(&mut self) {
        self.agents.selected = self.filtered_agents().len().saturating_sub(1);
        self.agents.table.selected = self.agents.selected;
        self.agents.details_scroll = 0;
        self.agents.reveal_selection(self.filtered_agents().len());
    }

    fn include_filtered_agents(&mut self) {
        let identities = self
            .filtered_agents()
            .into_iter()
            .map(AgentListItem::identity)
            .collect::<Vec<_>>();
        self.agents.included.extend(identities);
    }

    fn exclude_filtered_agents(&mut self) {
        let identities = self
            .filtered_agents()
            .into_iter()
            .map(AgentListItem::identity)
            .collect::<Vec<_>>();
        for identity in identities {
            self.agents.included.remove(&identity);
        }
    }

    fn open_agent_dataset_filter(&mut self) {
        self.agents.open_dataset_filter();
        self.mode = TuiMode::AgentDatasetFilter;
    }

    fn reset_agent_dataset(&mut self) {
        self.agents.invalidate_refresh();
        self.agents.reset_selection();
        self.agents.paging.reset();
        self.agents.agents.clear();
        self.agents.last_error = None;
    }

    fn toggle_agent_details(&mut self) {
        self.agents.detail_visible = !self.agents.detail_visible;
        if !self.agents.detail_visible {
            self.agents.focus = AgentOverviewFocus::List;
        }
    }

    fn scroll_agent_details_up(&mut self, amount: usize) {
        self.agents.details_scroll = self.agents.details_scroll.saturating_sub(amount);
    }

    fn scroll_agent_details_down(&mut self, amount: usize) {
        self.agents.details_scroll = self.agents.details_scroll.saturating_add(amount);
        self.agents.clamp_details_scroll();
    }

    fn scroll_agent_details_bottom(&mut self) {
        self.agents.details_scroll = self
            .agents
            .details_content_height
            .get()
            .saturating_sub(self.agents.details_viewport_height.get());
    }

    fn toggle_selected_agent(&mut self) {
        let identity = {
            let filtered = self.filtered_agents();
            filtered
                .get(self.agents.selected)
                .map(|agent| agent.identity())
        };
        let Some(identity) = identity else {
            return;
        };
        if !self.agents.included.remove(&identity) {
            self.agents.included.insert(identity);
        }
    }

    fn filtered_agents(&self) -> Vec<&AgentListItem> {
        self.agents.filtered_agents()
    }

    fn refresh_agents(&mut self, event_tx: Option<&TuiEventSender>) {
        self.start_agent_refresh(event_tx, false);
    }

    fn load_more_agents(&mut self, event_tx: Option<&TuiEventSender>) {
        if self.agents.has_more() {
            self.start_agent_refresh(event_tx, true);
        }
    }

    fn start_agent_refresh(&mut self, event_tx: Option<&TuiEventSender>, append: bool) {
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
        self.agents.refresh_append = append;
        self.agents.refresh_spinner_frame = 0;
        self.agents.stop_refresh_spinner();
        let spinner_stop = Arc::new(AtomicBool::new(false));
        spawn_agent_refresh_spinner(generation, event_tx.clone(), spinner_stop.clone());
        self.agents.refresh_spinner_stop = Some(spinner_stop);
        self.agents.last_error = None;
        self.agents.refresh_context_id = Some(context_executor.current_context_id());
        self.agents.refresh_context_label = Some(self.context.short_label());
        let mode = self.agents.mode;
        let environment_reference = self.selected_environment_reference.clone();
        let dataset_scope = self.agents.dataset_scope.clone();
        let page_cursor = append.then(|| AgentListPageCursor {
            components: self.agents.paging.cursors().clone(),
        });
        let max_count = self.agents.paging.request_limit(append);
        let auth_presenter = Some(tui_auth_presenter(&event_tx));
        context_executor.spawn(
            event_tx,
            auth_presenter,
            move |launch_context| async move {
                let (component_name, agent_type_name) = match &dataset_scope {
                    AgentDatasetScope::All => (None, None),
                    AgentDatasetScope::Component(component) => (
                        Some(golem_common::model::component::ComponentName(
                            component.clone(),
                        )),
                        None,
                    ),
                    AgentDatasetScope::AgentType { agent_type, .. } => (
                        None,
                        Some(golem_common::model::agent::AgentTypeName(
                            agent_type.clone(),
                        )),
                    ),
                };
                let request = AgentListRequest {
                    mode: mode.agent_list_mode(),
                    stable_sort: true,
                    environment_reference: environment_reference.clone(),
                    component_name,
                    agent_type_name,
                    page_cursor,
                    max_count: Some(max_count),
                    ..AgentListRequest::default()
                };
                let context = launch_context.context();
                let page = context.agent_handler().list_agent_metadata(request).await?;
                let agents = page.response.masked(context.masking_config())?;
                let agent_types = if append {
                    None
                } else {
                    let result = async {
                        let environment = context
                            .environment_handler()
                            .resolve_opt_environment_reference_read_only(
                                EnvironmentResolveMode::Any,
                                environment_reference.as_ref(),
                            )
                            .await?;
                        context.app_handler().list_agent_types(&environment).await
                    }
                    .await
                    .map_err(|error| format!("{error:#}"));
                    Some(result)
                };
                Ok(AgentRefreshPayload {
                    agents,
                    cursor: page.cursor,
                    agent_types,
                })
            },
            move |result| TuiEvent::AgentRefreshFinished {
                generation,
                append,
                result,
            },
        );
    }

    fn finish_agent_refresh(
        &mut self,
        generation: u64,
        append: bool,
        result: TuiContextTaskResult<AgentRefreshPayload>,
    ) {
        if generation != self.agents.refresh_generation {
            return;
        }

        let (context_id, result, logs) = result.into_parts();
        if Some(context_id) != self.agents.refresh_context_id {
            return;
        }

        self.agents.stop_refresh_spinner();
        self.agents.refresh_running = false;
        match result {
            Ok(payload) => {
                let selected_identity = self
                    .filtered_agents()
                    .get(self.agents.selected)
                    .map(|agent| agent.identity());
                let cursors = payload.cursor.components;
                let mut items = agent_items_from_metadata_response(payload.agents);
                if append {
                    let mut seen = self
                        .agents
                        .agents
                        .iter()
                        .map(AgentListItem::identity)
                        .collect::<HashSet<_>>();
                    self.agents.agents.extend(
                        items
                            .drain(..)
                            .filter(|agent| seen.insert(agent.identity())),
                    );
                    self.agents.agents.sort_by(|left, right| {
                        left.component
                            .cmp(&right.component)
                            .then_with(|| left.agent_id.cmp(&right.agent_id))
                    });
                } else {
                    self.agents.agents = items;
                }
                self.agents.paging.finish_request(append, cursors);
                if let Some(agent_types) = payload.agent_types {
                    match agent_types {
                        Ok(agent_types) => {
                            self.agents.agent_types = agent_types
                                .into_iter()
                                .filter_map(|agent_type| {
                                    let key = (
                                        agent_type.implemented_by.component_name.clone(),
                                        agent_type.agent_type.type_name.0.clone(),
                                    );
                                    serde_json::to_value(agent_type)
                                        .ok()
                                        .map(|value| (key, value))
                                })
                                .collect();
                            self.agents.metadata_error = None;
                        }
                        Err(error) => self.agents.metadata_error = Some(plain_tui_text(&error)),
                    }
                }
                self.agents.last_error = None;
                if let Some(selected_identity) = selected_identity {
                    self.agents.selected = self
                        .filtered_agents()
                        .iter()
                        .position(|agent| agent.identity() == selected_identity)
                        .unwrap_or(self.agents.selected);
                }
                self.agents.clamp_selection();
                self.agents.clamp_details_scroll();
            }
            Err(error) => {
                self.agents.last_error = Some(agent_refresh_error(error, logs));
            }
        }
    }

    fn refresh_ops_environment(&mut self, event_tx: Option<&TuiEventSender>) {
        if self.context_switch_pending() || self.ops_environment.loading {
            return;
        }
        let Some(event_tx) = event_tx.cloned() else {
            return;
        };
        let Some(context_executor) = self.context_executor.clone() else {
            self.ops_environment.error = Some("TUI context executor is not available".to_string());
            return;
        };

        self.ops_environment.generation = self.ops_environment.generation.saturating_add(1);
        let generation = self.ops_environment.generation;
        self.ops_environment.context_id = Some(context_executor.current_context_id());
        self.ops_environment.loading = true;
        self.ops_environment.error = None;
        let environment_reference = self.selected_environment_reference.clone();
        let auth_presenter = Some(tui_auth_presenter(&event_tx));
        context_executor.spawn(
            event_tx,
            auth_presenter,
            move |launch_context| async move {
                let environment = launch_context
                    .context()
                    .environment_handler()
                    .resolve_opt_environment_reference_read_only(
                        EnvironmentResolveMode::Any,
                        environment_reference.as_ref(),
                    )
                    .await?;
                Ok(ops_environment_snapshot(environment))
            },
            move |result| TuiEvent::OpsEnvironmentRefreshFinished { generation, result },
        );
    }

    fn finish_ops_environment_refresh(
        &mut self,
        generation: u64,
        result: TuiContextTaskResult<OpsEnvironmentSnapshot>,
    ) {
        if generation != self.ops_environment.generation {
            return;
        }
        let (context_id, result, logs) = result.into_parts();
        if Some(context_id) != self.ops_environment.context_id {
            return;
        }
        self.ops_environment.loading = false;
        match result {
            Ok(snapshot) => {
                self.ops_environment.snapshot = Some(snapshot);
                self.ops_environment.error = None;
            }
            Err(error) => {
                self.ops_environment.error = Some(agent_refresh_error(error, logs));
            }
        }
    }

    fn invalidate_ops_environment(&mut self) {
        self.ops_environment.generation = self.ops_environment.generation.saturating_add(1);
        self.ops_environment.context_id = None;
        self.ops_environment.loading = false;
        self.ops_environment.snapshot = None;
        self.ops_environment.error = None;
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
        self.reset_agent_dataset();
        self.refresh_agents(event_tx);
    }

    fn handle_command_interaction_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.cancel_or_force_kill_command(),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cancel_or_force_kill_command();
            }
            KeyCode::PageUp => {
                self.scroll_output_up();
                self.mode = TuiMode::TerminalScrollback;
            }
            KeyCode::PageDown => self.scroll_output_down(),
            KeyCode::Home => self.scroll_output_top(),
            KeyCode::End => self.scroll_output_bottom(),
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.open_palette(PaletteScope::All);
            }
            KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::CONTROL) => self.open_help(),
            _ => {
                if let Some(bytes) = encode_key_for_pty(key)
                    && let Some(run) = self.command_run.as_mut()
                    && let Some(runtime) = run.runtime.as_mut()
                    && let Err(error) = runtime.write_all(&bytes)
                {
                    run.terminal
                        .feed_local_line(format!("failed to send input: {error:#}"));
                }
            }
        }
    }

    fn open_command_form(&mut self, kind: CommandKind) {
        self.open_dev_workspace(DevView::BuildDeploy);
        let key = self.context.short_label();
        let values = self
            .command_form_memory
            .get(&key)
            .cloned()
            .unwrap_or_default();
        self.command_form = Some(CommandReviewForm::new(kind, values));
        self.mode = TuiMode::CommandForm;
    }

    fn handle_command_form_key(&mut self, key: KeyEvent, event_tx: Option<&TuiEventSender>) {
        if key.code == KeyCode::Char('x') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.open_leader();
            return;
        }
        if key.code == KeyCode::Char('p') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.open_palette(PaletteScope::All);
            return;
        }
        if key.code == KeyCode::Char('q') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.request_quit();
            return;
        }
        if key.code == KeyCode::Char('h') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.open_help();
            return;
        }

        let Some(form) = self.command_form.as_mut() else {
            self.mode = self.default_mode();
            return;
        };
        if form.editing {
            match key.code {
                KeyCode::Esc | KeyCode::Enter => form.editing = false,
                KeyCode::Backspace => {
                    command_form_text_value_mut(form).pop();
                }
                KeyCode::Char(character)
                    if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
                {
                    command_form_text_value_mut(form).push(character);
                }
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Esc => {
                self.command_form = None;
                self.mode = self.default_mode();
            }
            KeyCode::Up => form.selected = form.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Tab => {
                form.selected = (form.selected + 1) % form.field_count()
            }
            KeyCode::BackTab => {
                form.selected = (form.selected + form.field_count() - 1) % form.field_count()
            }
            KeyCode::Char(' ') | KeyCode::Enter => {
                if command_form_is_launch(form) {
                    self.launch_command_form(event_tx);
                } else {
                    update_command_form_field(form);
                }
            }
            _ => {}
        }
    }

    fn launch_command_form(&mut self, event_tx: Option<&TuiEventSender>) {
        let Some(form) = self.command_form.take() else {
            return;
        };
        let kind = form.kind;
        let values = form.values;
        let mut remembered = values.clone();
        remembered.yes = false;
        self.command_form_memory
            .insert(self.context.short_label(), remembered);
        self.mode = TuiMode::Normal;
        self.start_command_with_values(kind, values, event_tx);
    }

    #[cfg(test)]
    fn start_command(&mut self, kind: CommandKind, event_tx: Option<&TuiEventSender>) {
        let mut values = CommandFormValues::default();
        values.yes = self.command_options.yes;
        values.post_deploy = if self.command_options.reset {
            PostDeployPolicy::Reset
        } else {
            PostDeployPolicy::None
        };
        self.start_command_with_values(kind, values, event_tx);
    }

    fn start_command_with_values(
        &mut self,
        kind: CommandKind,
        values: CommandFormValues,
        event_tx: Option<&TuiEventSender>,
    ) {
        self.command_options = values.command_options();
        if self.command_is_running() {
            self.append_local_command_line("command already running");
            self.open_dev_workspace(DevView::BuildDeploy);
            return;
        }
        self.archive_current_command();
        if self.dev_context_unavailable() {
            self.open_dev_workspace(DevView::BuildDeploy);
            self.command_run = Some(CommandRun::new(
                self.next_command_id,
                kind,
                self.command_args(kind, &values),
                self.command_options,
                self.context.short_label(),
            ));
            self.next_command_id += 1;
            self.append_local_command_line("selected context is ops-only");
            self.set_command_status(CommandStatus::Failed);
            return;
        }
        let args = self.command_args(kind, &values);
        let command_id = self.next_command_id;
        self.next_command_id += 1;
        self.open_dev_workspace(DevView::BuildDeploy);
        self.mode = TuiMode::CommandInteraction;
        self.command_run = Some(CommandRun::new(
            command_id,
            kind,
            args.clone(),
            self.command_options,
            self.context.short_label(),
        ));
        if let Some((cols, rows)) = self.dev_pty_size(DevView::BuildDeploy)
            && let Some(run) = self.command_run.as_mut()
        {
            run.terminal.resize(rows, cols);
        }
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
            Ok(mut runtime) => {
                if let Some((cols, rows)) = self.dev_pty_size(DevView::BuildDeploy) {
                    let _ = runtime.resize(cols, rows);
                }
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

    fn command_args(&self, kind: CommandKind, values: &CommandFormValues) -> Vec<String> {
        let mut args = match kind {
            CommandKind::Plan => vec!["deploy".to_string(), "--plan".to_string()],
            CommandKind::Build => vec!["build".to_string()],
            CommandKind::Deploy => vec!["deploy".to_string()],
            CommandKind::Clean => vec!["clean".to_string()],
        };
        if kind == CommandKind::Build {
            const STEPS: [&str; 4] = ["check", "build", "add-metadata", "gen-bridge"];
            for (enabled, step) in values.build_steps.into_iter().zip(STEPS) {
                if enabled {
                    args.extend(["--step".to_string(), step.to_string()]);
                }
            }
            if values.skip_check && !values.build_steps.into_iter().any(|enabled| enabled) {
                args.push("--skip-check".to_string());
            }
        }
        if matches!(
            kind,
            CommandKind::Build | CommandKind::Plan | CommandKind::Deploy
        ) && values.force_build
        {
            args.push("--force-build".to_string());
        }
        if matches!(kind, CommandKind::Plan | CommandKind::Deploy) {
            if values.full_diff {
                args.push("--full-diff".to_string());
            }
            if !values.rollback_version.trim().is_empty() {
                args.extend([
                    "--version".to_string(),
                    values.rollback_version.trim().to_string(),
                ]);
            } else if !values.rollback_revision.trim().is_empty() {
                args.extend([
                    "--revision".to_string(),
                    values.rollback_revision.trim().to_string(),
                ]);
            }
            match values.post_deploy {
                PostDeployPolicy::None => {}
                PostDeployPolicy::UpdateAuto => {
                    args.extend(["--update-agents".to_string(), "auto".to_string()])
                }
                PostDeployPolicy::UpdateManual => {
                    args.extend(["--update-agents".to_string(), "manual".to_string()])
                }
                PostDeployPolicy::Redeploy => args.push("--redeploy-agents".to_string()),
                PostDeployPolicy::Reset => args.push("--reset".to_string()),
            }
        }
        if values.yes {
            args.push("--yes".to_string());
        }
        args
    }

    fn command_spec(&self, args: Vec<String>) -> anyhow::Result<NestedCliSpec> {
        self.nested_cli_spec(args)
    }

    fn append_command_output(&mut self, bytes: &[u8]) {
        if let Some(run) = self.command_run.as_mut() {
            run.terminal.feed(bytes);
        }
    }

    fn append_local_command_line(&mut self, line: impl AsRef<str>) {
        if let Some(run) = self.command_run.as_mut() {
            run.terminal.feed_local_line(line);
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
            run.finished_at = Some(Instant::now());
            run.runtime = None;
            run.stop_spinner();
            run.terminal.feed_local_line(match run.status {
                CommandStatus::Succeeded => "command succeeded".to_string(),
                CommandStatus::Failed => {
                    format!("command failed with exit code {}", exit.code.unwrap_or(1))
                }
                CommandStatus::Killed => "command killed".to_string(),
                CommandStatus::Running | CommandStatus::Cancelling => "command exited".to_string(),
            });
        }
        self.trim_completed_job_capacity();
        if self.mode == TuiMode::CommandInteraction
            || (self.mode == TuiMode::TerminalScrollback && self.dev_focus == DevView::BuildDeploy)
        {
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
                        run.terminal
                            .feed_local_line(format!("failed to send cancel: {error:#}"));
                    }
                }
                run.status = CommandStatus::Cancelling;
                run.stop_requested_at = Some(Instant::now());
                run.terminal.feed_local_line("cancel requested");
            }
            CommandStatus::Cancelling => {
                if !run.force_stop_available() {
                    let remaining = FORCE_STOP_DELAY
                        .saturating_sub(run.stop_requested_at.expect("cancelling job").elapsed())
                        .as_secs_f32();
                    run.terminal
                        .feed_local_line(format!("force stop is available in {remaining:.1}s"));
                    return;
                }
                if let Some(runtime) = run.runtime.as_mut()
                    && let Err(error) = runtime.kill()
                {
                    run.terminal
                        .feed_local_line(format!("failed to force kill: {error:#}"));
                }
                run.runtime = None;
                run.status = CommandStatus::Killed;
                run.finished_at = Some(Instant::now());
                run.stop_spinner();
                run.terminal.feed_local_line("force killed");
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
        self.agents.stop_refresh_spinner();
        if let Some(stop) = self.agents.auto_refresh_stop.take() {
            stop.store(true, Ordering::Relaxed);
        }
    }

    fn save_preferences(&mut self) {
        if let Err(error) = write_tui_preferences(
            &self.preferences_path,
            &TuiPreferences::from_state(&self.layout, &self.agents.table),
        ) {
            self.preferences_error = Some(format!("failed to save TUI preferences: {error:#}"));
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

    fn archive_current_command(&mut self) {
        let Some(run) = self.command_run.take() else {
            return;
        };
        debug_assert!(!matches!(
            run.status,
            CommandStatus::Running | CommandStatus::Cancelling
        ));
        self.completed_jobs.push_front(run);
        self.trim_completed_job_capacity();
    }

    fn trim_completed_job_capacity(&mut self) {
        let current_completed = usize::from(self.command_run.as_ref().is_some_and(|run| {
            !matches!(
                run.status,
                CommandStatus::Running | CommandStatus::Cancelling
            )
        }));
        let history_limit = COMPLETED_JOB_LIMIT.saturating_sub(current_completed);
        while self.completed_jobs.len() > history_limit {
            self.completed_jobs.pop_back();
        }
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

    fn resize_nested_sessions(&mut self, cols: u16, rows: u16) {
        let snapshot = layout::compute(LayoutInput {
            area: Rect::new(0, 0, cols, rows),
            active_workspace: self.active_workspace,
            dev_primary: self.dev_primary,
            focused_dev_view: self.dev_focus,
            mode: self.mode,
            context_picker_rows: context_picker_layout_rows(&self.context_switcher),
            context_picker_selected: self.context_switcher.selected,
            context_picker_prefix_height: context_picker_prefix_height(&self.context_switcher),
            agent_details_visible: self.ops_agents_focused() && self.agents.detail_visible,
            layout: self.layout.clone(),
        });
        self.layout_snapshot.replace(Some(snapshot.clone()));
        if let Some(run) = self.command_run.as_mut()
            && let Some(area) = snapshot.region(RegionKind::DevViewBody(DevView::BuildDeploy))
        {
            let (pty_cols, pty_rows) = pty_size_for_region(area);
            run.terminal.resize(pty_rows, pty_cols);
            if let Some(runtime) = run.runtime.as_mut() {
                let _ = runtime.resize(pty_cols, pty_rows);
            }
        }
        if let Some(area) = snapshot.region(RegionKind::DevViewBody(DevView::Server)) {
            let (pty_cols, pty_rows) = pty_size_for_region(area);
            self.server.run.terminal.resize(pty_rows, pty_cols);
            if let Some(runtime) = self.server.run.runtime.as_mut() {
                let _ = runtime.resize(pty_cols, pty_rows);
            }
        }
        if let Some(area) = snapshot.region(RegionKind::DevViewBody(DevView::Repl)) {
            let (pty_cols, pty_rows) = pty_size_for_region(area);
            self.repl.run.screen.resize(pty_rows, pty_cols);
            if let Some(runtime) = self.repl.run.runtime.as_mut() {
                let _ = runtime.resize(pty_cols, pty_rows);
            }
        }
    }

    fn scroll_output_up(&mut self) {
        self.scroll_output_up_by(10);
    }

    fn scroll_output_up_by(&mut self, amount: usize) {
        if let Some(run) = self.command_run.as_mut() {
            run.terminal.scroll_up(amount);
        }
    }

    fn scroll_output_down(&mut self) {
        self.scroll_output_down_by(10);
    }

    fn scroll_output_down_by(&mut self, amount: usize) {
        if let Some(run) = self.command_run.as_mut() {
            run.terminal.scroll_down(amount);
        }
    }

    fn scroll_output_top(&mut self) {
        if let Some(run) = self.command_run.as_mut() {
            run.terminal.scroll_top();
        }
    }

    fn scroll_output_bottom(&mut self) {
        if let Some(run) = self.command_run.as_mut() {
            run.terminal.scroll_bottom();
        }
    }

    fn request_server_toggle(&mut self) {
        self.open_dev_workspace(DevView::Server);
        if self.server.run.is_running() {
            self.stop_server();
        } else {
            self.open_server_form(false, false);
        }
    }

    fn open_server_form(&mut self, restart: bool, clean: bool) {
        self.open_dev_workspace(DevView::Server);
        let key = self.context.short_label();
        let mut values = self
            .server_form_memory
            .get(&key)
            .cloned()
            .unwrap_or_else(|| ServerFormValues::new(&self.context.config_dir));
        values.clean |= clean;
        values.yes = false;
        self.session_form = Some(SessionReviewForm::Server {
            values,
            selected: 0,
            editing: false,
            restart,
            error: None,
        });
        self.mode = TuiMode::SessionForm;
    }

    fn open_repl_form(&mut self, restart: bool) {
        self.open_dev_workspace(DevView::Repl);
        let key = self.context.short_label();
        let mut values = self.repl_form_memory.get(&key).cloned().unwrap_or_default();
        values.yes = false;
        self.session_form = Some(SessionReviewForm::Repl {
            values,
            selected: 0,
            restart,
        });
        self.mode = TuiMode::SessionForm;
    }

    fn handle_session_form_key(&mut self, key: KeyEvent, event_tx: Option<&TuiEventSender>) {
        if key.code == KeyCode::Char('x') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.open_leader();
            return;
        }
        if key.code == KeyCode::Char('p') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.open_palette(PaletteScope::All);
            return;
        }
        if key.code == KeyCode::Char('q') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.request_quit();
            return;
        }
        if key.code == KeyCode::Char('h') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.open_help();
            return;
        }
        let Some(form) = self.session_form.as_mut() else {
            self.mode = self.default_mode();
            return;
        };

        if let SessionReviewForm::Server {
            values,
            selected,
            editing,
            error,
            ..
        } = form
            && *editing
        {
            match key.code {
                KeyCode::Esc | KeyCode::Enter => *editing = false,
                KeyCode::Backspace => {
                    server_form_text_mut(values, *selected).pop();
                    *error = None;
                }
                KeyCode::Char(character)
                    if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
                {
                    server_form_text_mut(values, *selected).push(character);
                    *error = None;
                }
                _ => {}
            }
            return;
        }

        let count = form.field_count();
        match key.code {
            KeyCode::Esc => {
                self.session_form = None;
                self.mode = self.default_mode();
            }
            KeyCode::Up => form.set_selected(form.selected().saturating_sub(1)),
            KeyCode::Down | KeyCode::Tab => form.set_selected((form.selected() + 1) % count),
            KeyCode::BackTab => form.set_selected((form.selected() + count - 1) % count),
            KeyCode::Char(' ') | KeyCode::Enter => {
                if form.selected() + 1 == count {
                    self.launch_session_form(event_tx);
                } else {
                    update_session_form_field(form);
                }
            }
            _ => {}
        }
    }

    fn launch_session_form(&mut self, event_tx: Option<&TuiEventSender>) {
        let Some(mut form) = self.session_form.take() else {
            return;
        };
        match &mut form {
            SessionReviewForm::Server {
                values,
                restart,
                error,
                ..
            } => {
                if let Err(message) = validate_server_form(values) {
                    *error = Some(message);
                    self.session_form = Some(form);
                    return;
                }
                let values = values.clone();
                let mut remembered = values.clone();
                remembered.yes = false;
                self.server_form_memory
                    .insert(self.context.short_label(), remembered);
                self.mode = TuiMode::Normal;
                if *restart && self.server.run.is_running() {
                    self.pending_server_restart = Some(values);
                    self.stop_server();
                } else {
                    self.start_server_with_values(values, event_tx);
                }
            }
            SessionReviewForm::Repl {
                values, restart, ..
            } => {
                let values = values.clone();
                let mut remembered = values.clone();
                remembered.yes = false;
                self.repl_form_memory
                    .insert(self.context.short_label(), remembered);
                self.mode = TuiMode::Normal;
                if *restart && self.repl.is_running() {
                    self.pending_repl_restart = Some(values);
                    self.stop_repl();
                } else {
                    self.start_repl_with_values(values, event_tx);
                }
            }
        }
    }

    fn start_server_with_values(
        &mut self,
        values: ServerFormValues,
        event_tx: Option<&TuiEventSender>,
    ) {
        if self.local_server_unavailable() {
            self.server
                .run
                .terminal
                .feed_local_line("local server is unavailable");
            self.open_dev_workspace(DevView::Server);
            return;
        }
        if self.server.run.is_running() {
            self.server
                .run
                .terminal
                .feed_local_line("server already running");
            self.open_dev_workspace(DevView::Server);
            return;
        }

        let clean = values.clean;
        let args = server_args_from_values(&values);
        let ports_file = Some(PathBuf::from(values.ports_file.trim()));
        let ports_file_before_modified = ports_file
            .as_ref()
            .and_then(|path| std::fs::metadata(path).ok())
            .and_then(|metadata| metadata.modified().ok());
        self.open_dev_workspace(DevView::Server);
        self.server.run = ServerRun::new(
            self.server.next_id,
            clean,
            args.clone(),
            self.server.launch_label.clone(),
            ports_file,
            ports_file_before_modified,
        );
        if let Some((cols, rows)) = self.dev_pty_size(DevView::Server) {
            self.server.run.terminal.resize(rows, cols);
        }
        self.server.next_id += 1;
        self.server
            .run
            .terminal
            .feed_local_line(format!("$ {}", command_display(&args)));

        let Some(event_tx) = event_tx else {
            return;
        };

        let spec = match self.server_spec(args) {
            Ok(spec) => spec,
            Err(error) => {
                self.server
                    .run
                    .terminal
                    .feed_local_line(format!("failed to prepare server: {error:#}"));
                self.server.run.status = ServerStatus::Failed;
                return;
            }
        };

        match spawn_nested_cli(spec, event_tx.clone(), NestedCliTarget::Server) {
            Ok(mut runtime) => {
                if let Some((cols, rows)) = self.dev_pty_size(DevView::Server) {
                    let _ = runtime.resize(cols, rows);
                }
                self.server.run.runtime = Some(runtime);
                let stop = Arc::new(AtomicBool::new(false));
                spawn_server_spinner(self.server.run.id, event_tx.clone(), stop.clone());
                self.server.run.spinner_stop = Some(stop);
            }
            Err(error) => {
                self.server
                    .run
                    .terminal
                    .feed_local_line(format!("failed to start server: {error:#}"));
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
                            .terminal
                            .feed_local_line(format!("failed to stop server: {error:#}"));
                    }
                }
                self.server.run.status = ServerStatus::Stopping;
                self.server.run.stop_requested_at = Some(Instant::now());
                self.server
                    .run
                    .terminal
                    .feed_local_line("server stop requested");
            }
            ServerStatus::Stopping => {
                if !self
                    .server
                    .run
                    .stop_requested_at
                    .is_some_and(|requested| requested.elapsed() >= FORCE_STOP_DELAY)
                {
                    self.server.run.terminal.feed_local_line(
                        "force stop becomes available five seconds after the first stop",
                    );
                    return;
                }
                if let Some(runtime) = self.server.run.runtime.as_mut()
                    && let Err(error) = runtime.kill()
                {
                    self.server
                        .run
                        .terminal
                        .feed_local_line(format!("failed to force kill server: {error:#}"));
                }
                self.server.run.runtime = None;
                self.server.run.status = ServerStatus::Stopped;
                self.server.run.stop_requested_at = None;
                self.server.run.stop_spinner();
                self.server
                    .run
                    .terminal
                    .feed_local_line("server force killed");
            }
            ServerStatus::Stopped | ServerStatus::Failed => {}
        }
    }

    fn finish_server(&mut self, exit: CommandExit, event_tx: &TuiEventSender) {
        let pending_restart = self.pending_server_restart.take();
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
            .terminal
            .feed_local_line(match self.server.run.status {
                ServerStatus::Stopped => "server stopped".to_string(),
                ServerStatus::Failed => {
                    format!("server exited with code {}", exit.code.unwrap_or(1))
                }
                ServerStatus::Starting | ServerStatus::Running | ServerStatus::Stopping => {
                    "server exited".to_string()
                }
            });

        if let Some(values) = pending_restart {
            self.start_server_with_values(values, Some(event_tx));
        }
        self.resume_pending_context_switch_if_ready(Some(event_tx));
    }

    fn handle_server_spinner_tick(&mut self, server_id: u64) {
        if self.server.run.id == server_id && self.server.run.is_running() {
            self.server.run.spinner_frame = self.server.run.spinner_frame.wrapping_add(1);
            if self.server.run.status == ServerStatus::Starting {
                if let Some(ports) = fresh_bound_server_ports(&self.server.run) {
                    self.server.run.bound_ports = Some(ports);
                    self.server.run.status = ServerStatus::Running;
                }
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
        self.server.run.terminal.scroll_up(amount);
    }

    fn scroll_server_down_by(&mut self, amount: usize) {
        self.server.run.terminal.scroll_down(amount);
    }

    fn start_or_focus_repl(&mut self, _event_tx: Option<&TuiEventSender>) {
        self.open_dev_workspace(DevView::Repl);
        if self.dev_context_unavailable() {
            self.repl.run.last_error = Some("selected context is ops-only".to_string());
            self.mode = TuiMode::Normal;
            return;
        }
        if self.repl.is_running() {
            self.mode = TuiMode::Repl;
            return;
        }

        self.open_repl_form(false);
    }

    fn start_repl_with_values(
        &mut self,
        values: ReplFormValues,
        event_tx: Option<&TuiEventSender>,
    ) {
        let args = repl_args_from_values(&values);
        let (screen_cols, screen_rows) = self.dev_pty_size(DevView::Repl).unwrap_or_else(|| {
            let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
            let (rows, cols) = repl_screen_size_for_terminal(cols, rows);
            (cols, rows)
        });
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
        self.open_dev_workspace(DevView::Repl);
        if self.repl.is_running() {
            self.mode = TuiMode::Repl;
        }
    }

    fn handle_repl_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Char('x') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.open_leader();
            return;
        }

        if key.code == KeyCode::PageUp {
            self.repl.run.screen.scroll_up(10);
            self.mode = TuiMode::TerminalScrollback;
            return;
        }

        if let Some(bytes) = encode_key_for_pty(key)
            && let Some(runtime) = self.repl.run.runtime.as_mut()
            && let Err(error) = runtime.write_all(&bytes)
        {
            self.repl.run.last_error = Some(format!("failed to send REPL input: {error:#}"));
        }
    }

    fn handle_terminal_scrollback_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Char('x') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.open_leader();
            return;
        }
        if key.code == KeyCode::Char('q') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.request_quit();
            return;
        }
        match key.code {
            KeyCode::Esc => {
                self.scroll_terminal_bottom();
                self.mode = self.default_mode();
            }
            KeyCode::Up => self.scroll_terminal_up(1),
            KeyCode::Down => self.scroll_terminal_down(1),
            KeyCode::PageUp => self.scroll_terminal_up(10),
            KeyCode::PageDown => self.scroll_terminal_down(10),
            KeyCode::Home => self.scroll_terminal_top(),
            KeyCode::End => self.scroll_terminal_bottom(),
            _ => {}
        }
    }

    fn scroll_terminal_up(&mut self, amount: usize) {
        match self.dev_focus {
            DevView::BuildDeploy => self.scroll_output_up_by(amount),
            DevView::Server => self.scroll_server_up_by(amount),
            DevView::Repl => self.repl.run.screen.scroll_up(amount),
        }
    }

    fn scroll_terminal_down(&mut self, amount: usize) {
        match self.dev_focus {
            DevView::BuildDeploy => self.scroll_output_down_by(amount),
            DevView::Server => self.scroll_server_down_by(amount),
            DevView::Repl => self.repl.run.screen.scroll_down(amount),
        }
    }

    fn scroll_terminal_top(&mut self) {
        match self.dev_focus {
            DevView::BuildDeploy => self.scroll_output_top(),
            DevView::Server => self.server.run.terminal.scroll_top(),
            DevView::Repl => self.repl.run.screen.scroll_top(),
        }
    }

    fn scroll_terminal_bottom(&mut self) {
        match self.dev_focus {
            DevView::BuildDeploy => self.scroll_output_bottom(),
            DevView::Server => self.server.run.terminal.scroll_bottom(),
            DevView::Repl => self.repl.run.screen.scroll_bottom(),
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
        let pending_restart = self.pending_repl_restart.take();
        if self.mode == TuiMode::Repl
            || (self.mode == TuiMode::TerminalScrollback && self.dev_focus == DevView::Repl)
            || (self.mode == TuiMode::Leader && self.leader.return_mode == LeaderReturnMode::Repl)
        {
            self.mode = TuiMode::Normal;
        }
        if let Some(values) = pending_restart {
            self.start_repl_with_values(values, Some(event_tx));
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
                self.repl.run.stop_requested_at = Some(Instant::now());
            }
            ReplStatus::Stopping => {
                if !self
                    .repl
                    .run
                    .stop_requested_at
                    .is_some_and(|requested| requested.elapsed() >= FORCE_STOP_DELAY)
                {
                    self.repl.run.last_error = Some(
                        "force stop becomes available five seconds after the first stop"
                            .to_string(),
                    );
                    return;
                }
                if let Some(runtime) = self.repl.run.runtime.as_mut()
                    && let Err(error) = runtime.kill()
                {
                    self.repl.run.last_error =
                        Some(format!("failed to force kill REPL: {error:#}"));
                }
                self.repl.run.runtime = None;
                self.repl.run.status = ReplStatus::Stopped;
                self.repl.run.stop_requested_at = None;
            }
            ReplStatus::Stopped | ReplStatus::Failed => {}
        }
    }

    fn cleanup_repl(&mut self) {
        if let Some(runtime) = self.repl.run.runtime.as_mut() {
            let _ = runtime.kill();
        }
    }

    fn dev_pty_size(&self, view: DevView) -> Option<(u16, u16)> {
        self.layout_snapshot
            .borrow()
            .as_ref()
            .and_then(|snapshot| snapshot.region(RegionKind::DevViewBody(view)))
            .map(pty_size_for_region)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TuiMode {
    Normal,
    Leader,
    Layout,
    Jobs,
    QuitBlocked,
    Palette,
    ContextPicker,
    ContextSwitchConfirm,
    Help,
    AgentFilter,
    AgentDatasetFilter,
    AgentColumns,
    CommandForm,
    SessionForm,
    CommandInteraction,
    Repl,
    TerminalScrollback,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum LeaderGroup {
    #[default]
    Workspaces,
    Views,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum LeaderReturnMode {
    #[default]
    Normal,
    AgentFilter,
    Palette,
    CommandForm,
    SessionForm,
    Repl,
    TerminalScrollback,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct LeaderState {
    group: Option<LeaderGroup>,
    return_mode: LeaderReturnMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandKind {
    Plan,
    Build,
    Deploy,
    Clean,
}

impl CommandKind {
    fn title(self) -> &'static str {
        match self {
            Self::Plan => "plan",
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum PostDeployPolicy {
    #[default]
    None,
    UpdateAuto,
    UpdateManual,
    Redeploy,
    Reset,
}

impl PostDeployPolicy {
    fn next(self) -> Self {
        match self {
            Self::None => Self::UpdateAuto,
            Self::UpdateAuto => Self::UpdateManual,
            Self::UpdateManual => Self::Redeploy,
            Self::Redeploy => Self::Reset,
            Self::Reset => Self::None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::UpdateAuto => "update auto",
            Self::UpdateManual => "update manual",
            Self::Redeploy => "redeploy (state loss)",
            Self::Reset => "reset environment (destructive)",
        }
    }
}

#[derive(Debug, Clone, Default)]
struct CommandFormValues {
    build_steps: [bool; 4],
    skip_check: bool,
    force_build: bool,
    full_diff: bool,
    rollback_version: String,
    rollback_revision: String,
    post_deploy: PostDeployPolicy,
    yes: bool,
}

impl CommandFormValues {
    fn command_options(&self) -> CommandOptions {
        CommandOptions {
            yes: self.yes,
            reset: self.post_deploy == PostDeployPolicy::Reset,
        }
    }
}

#[derive(Debug, Clone)]
struct CommandReviewForm {
    kind: CommandKind,
    values: CommandFormValues,
    selected: usize,
    editing: bool,
}

impl CommandReviewForm {
    fn new(kind: CommandKind, mut values: CommandFormValues) -> Self {
        values.yes = false;
        Self {
            kind,
            values,
            selected: 0,
            editing: false,
        }
    }

    fn field_count(&self) -> usize {
        match self.kind {
            CommandKind::Build => 8,
            CommandKind::Plan | CommandKind::Deploy => 7,
            CommandKind::Clean => 2,
        }
    }
}

fn command_form_is_launch(form: &CommandReviewForm) -> bool {
    form.selected + 1 == form.field_count()
}

fn command_form_text_value_mut(form: &mut CommandReviewForm) -> &mut String {
    if form.selected == 3 {
        &mut form.values.rollback_revision
    } else {
        &mut form.values.rollback_version
    }
}

fn update_command_form_field(form: &mut CommandReviewForm) {
    match form.kind {
        CommandKind::Build => match form.selected {
            0..=3 => {
                form.values.build_steps[form.selected] = !form.values.build_steps[form.selected];
                if form.values.build_steps[form.selected] {
                    form.values.skip_check = false;
                }
            }
            4 => {
                form.values.skip_check = !form.values.skip_check;
                if form.values.skip_check {
                    form.values.build_steps = [false; 4];
                }
            }
            5 => form.values.force_build = !form.values.force_build,
            6 => form.values.yes = !form.values.yes,
            _ => {}
        },
        CommandKind::Plan | CommandKind::Deploy => match form.selected {
            0 => form.values.full_diff = !form.values.full_diff,
            1 => form.values.force_build = !form.values.force_build,
            2 => {
                form.values.rollback_revision.clear();
                form.editing = true;
            }
            3 => {
                form.values.rollback_version.clear();
                form.editing = true;
            }
            4 => form.values.post_deploy = form.values.post_deploy.next(),
            5 => form.values.yes = !form.values.yes,
            _ => {}
        },
        CommandKind::Clean => {
            if form.selected == 0 {
                form.values.yes = !form.values.yes;
            }
        }
    }
}

struct CommandRun {
    id: u64,
    kind: CommandKind,
    status: CommandStatus,
    options: CommandOptions,
    args: Vec<String>,
    launch_context: String,
    terminal: TerminalScreen,
    runtime: Option<NestedCliRuntime>,
    spinner_frame: usize,
    spinner_stop: Option<Arc<AtomicBool>>,
    exit_code: Option<i32>,
    started_at: Instant,
    finished_at: Option<Instant>,
    stop_requested_at: Option<Instant>,
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
            terminal: TerminalScreen::new(20, 80),
            runtime: None,
            spinner_frame: 0,
            spinner_stop: None,
            exit_code: None,
            started_at: Instant::now(),
            finished_at: None,
            stop_requested_at: None,
        }
    }

    fn stop_spinner(&mut self) {
        if let Some(stop) = self.spinner_stop.take() {
            stop.store(true, Ordering::Relaxed);
        }
    }

    fn elapsed(&self) -> Duration {
        self.finished_at
            .unwrap_or_else(Instant::now)
            .saturating_duration_since(self.started_at)
    }

    fn force_stop_available(&self) -> bool {
        self.stop_requested_at
            .is_some_and(|requested| requested.elapsed() >= FORCE_STOP_DELAY)
    }
}

struct LocalServerService {
    available: bool,
    cli_args: Vec<String>,
    launch_label: String,
    next_id: u64,
    run: ServerRun,
}

struct ReplState {
    run: ReplRun,
}

struct AgentsState {
    mode: AgentModeFilter,
    query: String,
    query_scope: AgentQueryScope,
    dataset_scope: AgentDatasetScope,
    dataset_filter: Option<AgentDatasetFilterState>,
    selected: usize,
    table: PaneTableState,
    included: HashSet<AgentIdentity>,
    column_chooser: Option<ColumnChooserState>,
    detail_visible: bool,
    focus: AgentOverviewFocus,
    details_scroll: usize,
    details_content_height: Cell<usize>,
    details_viewport_height: Cell<usize>,
    table_viewport_width: Cell<u16>,
    table_viewport_height: Cell<u16>,
    table_first_visible_row: Cell<usize>,
    table_virtual_width: Cell<u16>,
    auto_refresh: bool,
    refresh_running: bool,
    refresh_generation: u64,
    refresh_context_id: Option<TuiContextId>,
    refresh_context_label: Option<String>,
    refresh_spinner_frame: usize,
    refresh_spinner_stop: Option<Arc<AtomicBool>>,
    auto_refresh_stop: Option<Arc<AtomicBool>>,
    last_error: Option<String>,
    metadata_error: Option<String>,
    paging: CursorCollectionState<Option<ScanCursor>>,
    refresh_append: bool,
    agents: Vec<AgentListItem>,
    agent_types: BTreeMap<(String, String), Value>,
}

impl Default for AgentsState {
    fn default() -> Self {
        Self {
            mode: AgentModeFilter::Durable,
            query: String::new(),
            query_scope: AgentQueryScope::AgentId,
            dataset_scope: AgentDatasetScope::All,
            dataset_filter: None,
            selected: 0,
            table: PaneTableState::new(&AGENT_COLUMNS, 0),
            included: HashSet::new(),
            column_chooser: None,
            detail_visible: true,
            focus: AgentOverviewFocus::List,
            details_scroll: 0,
            details_content_height: Cell::new(0),
            details_viewport_height: Cell::new(0),
            table_viewport_width: Cell::new(0),
            table_viewport_height: Cell::new(0),
            table_first_visible_row: Cell::new(0),
            table_virtual_width: Cell::new(0),
            auto_refresh: false,
            refresh_running: false,
            refresh_generation: 0,
            refresh_context_id: None,
            refresh_context_label: None,
            refresh_spinner_frame: 0,
            refresh_spinner_stop: None,
            auto_refresh_stop: None,
            last_error: None,
            metadata_error: None,
            paging: CursorCollectionState::new(AGENT_PAGE_SIZE),
            refresh_append: false,
            agents: Vec::new(),
            agent_types: BTreeMap::new(),
        }
    }
}

impl AgentsState {
    fn filtered_agents(&self) -> Vec<&AgentListItem> {
        let query = self.query.trim();
        if query.is_empty() {
            return self.agents.iter().collect();
        }

        let query = query.to_lowercase();
        self.agents
            .iter()
            .filter(|agent| {
                self.query_scope
                    .value(agent)
                    .to_lowercase()
                    .contains(&query)
            })
            .collect()
    }

    fn clamp_selection(&mut self) {
        let count = self.filtered_agents().len();
        if count == 0 {
            self.selected = 0;
        } else {
            self.selected = self.selected.min(count - 1);
        }
        self.table.selected = self.selected;
        self.reveal_selection(count);
    }

    fn reset_selection(&mut self) {
        self.selected = 0;
        self.table.selected = 0;
        self.table_first_visible_row.set(0);
        self.details_scroll = 0;
    }

    fn reveal_selection(&self, count: usize) {
        if count == 0 {
            self.table_first_visible_row.set(0);
            return;
        }
        let capacity = self.table_viewport_height.get().saturating_sub(1).max(1) as usize;
        let mut first = self
            .table_first_visible_row
            .get()
            .min(count.saturating_sub(1));
        if self.selected < first {
            first = self.selected;
        } else if self.selected >= first.saturating_add(capacity) {
            first = self.selected.saturating_add(1).saturating_sub(capacity);
        }
        first = first.min(count.saturating_sub(capacity));
        self.table_first_visible_row.set(first);
    }

    fn selected_agent<'a>(&self, filtered: &'a [&'a AgentListItem]) -> Option<&'a AgentListItem> {
        filtered.get(self.selected).copied()
    }

    fn has_more(&self) -> bool {
        self.paging.has_more()
    }

    fn dataset_label(&self) -> String {
        self.dataset_scope.label()
    }

    fn open_dataset_filter(&mut self) {
        self.dataset_filter = Some(AgentDatasetFilterState::new(
            &self.dataset_scope,
            &self.agent_types,
            &self.agents,
        ));
    }

    fn clamp_details_scroll(&mut self) {
        let max = self
            .details_content_height
            .get()
            .saturating_sub(self.details_viewport_height.get());
        self.details_scroll = self.details_scroll.min(max);
    }

    fn invalidate_refresh(&mut self) {
        self.stop_refresh_spinner();
        self.refresh_running = false;
        self.refresh_context_id = None;
        self.refresh_generation = self.refresh_generation.saturating_add(1);
    }

    fn stop_refresh_spinner(&mut self) {
        if let Some(stop) = self.refresh_spinner_stop.take() {
            stop.store(true, Ordering::Relaxed);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentOverviewFocus {
    List,
    Details,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentQueryScope {
    AgentId,
    Component,
    AgentType,
}

impl AgentQueryScope {
    fn next(self) -> Self {
        match self {
            Self::AgentId => Self::Component,
            Self::Component => Self::AgentType,
            Self::AgentType => Self::AgentId,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::AgentId => "AgentID",
            Self::Component => "Component",
            Self::AgentType => "Agent type",
        }
    }

    fn value<'a>(self, agent: &'a AgentListItem) -> &'a str {
        match self {
            Self::AgentId => &agent.agent_id,
            Self::Component => agent.component.as_deref().unwrap_or(""),
            Self::AgentType => agent.agent_type.as_deref().unwrap_or(""),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum AgentDatasetScope {
    All,
    Component(String),
    AgentType {
        component: String,
        agent_type: String,
    },
}

impl AgentDatasetScope {
    fn label(&self) -> String {
        match self {
            Self::All => "component:any · type:any".to_string(),
            Self::Component(component) => format!("component:{component} · type:any"),
            Self::AgentType {
                component,
                agent_type,
            } => {
                format!("component:{component} · type:{agent_type}")
            }
        }
    }

    fn matches_query(&self, query: &str) -> bool {
        match self {
            Self::All => "any".contains(query),
            Self::Component(component) => component.to_lowercase().contains(query),
            Self::AgentType {
                component,
                agent_type,
            } => {
                component.to_lowercase().contains(query)
                    || agent_type.to_lowercase().contains(query)
            }
        }
    }
}

#[derive(Clone, Debug)]
struct AgentDatasetFilterState {
    choices: Vec<AgentDatasetScope>,
    query: String,
    selected: usize,
}

impl AgentDatasetFilterState {
    fn new(
        current: &AgentDatasetScope,
        agent_types: &BTreeMap<(String, String), Value>,
        agents: &[AgentListItem],
    ) -> Self {
        let mut choices = vec![AgentDatasetScope::All];
        let mut types = agent_types
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        types.extend(agents.iter().filter_map(|agent| {
            agent
                .component
                .as_ref()
                .zip(agent.agent_type.as_ref())
                .map(|(component, agent_type)| (component.clone(), agent_type.clone()))
        }));
        let components = types
            .iter()
            .map(|(component, _)| component.clone())
            .collect::<std::collections::BTreeSet<_>>();
        choices.extend(components.into_iter().map(AgentDatasetScope::Component));
        choices.extend(types.into_iter().map(|(component, agent_type)| {
            AgentDatasetScope::AgentType {
                component,
                agent_type,
            }
        }));
        if !choices.contains(current) {
            choices.push(current.clone());
        }
        let selected = choices
            .iter()
            .position(|choice| choice == current)
            .unwrap_or(0);
        Self {
            choices,
            query: String::new(),
            selected,
        }
    }

    fn filtered_indices(&self) -> Vec<usize> {
        let query = self.query.trim().to_lowercase();
        self.choices
            .iter()
            .enumerate()
            .filter_map(|(index, choice)| {
                if query.is_empty() || choice.matches_query(&query) {
                    Some(index)
                } else {
                    None
                }
            })
            .collect()
    }

    fn selected_choice(&self) -> Option<&AgentDatasetScope> {
        let index = self.filtered_indices().get(self.selected).copied()?;
        self.choices.get(index)
    }

    fn reset_filtered_selection(&mut self) {
        self.selected = 0;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpsRootSubject {
    Agents,
    Deployment,
    Environment,
    Server,
}

impl OpsRootSubject {
    fn label(self) -> &'static str {
        match self {
            Self::Agents => "Agents",
            Self::Deployment => "Deployment",
            Self::Environment => "Environment",
            Self::Server => "Server",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum OpsSubject {
    Root(OpsRootSubject),
    Agent(AgentIdentity),
}

impl OpsSubject {
    fn root(&self) -> OpsRootSubject {
        match self {
            Self::Root(root) => *root,
            Self::Agent(_) => OpsRootSubject::Agents,
        }
    }

    fn label(&self) -> String {
        match self {
            Self::Root(root) => root.label().to_string(),
            Self::Agent(agent) => agent.agent_id.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpsView {
    Overview,
    Activity,
    Metrics,
}

impl OpsView {
    fn next(self) -> Self {
        match self {
            Self::Overview => Self::Activity,
            Self::Activity => Self::Metrics,
            Self::Metrics => Self::Overview,
        }
    }
}

#[cfg(feature = "tui-preview")]
#[derive(Clone, Copy)]
struct FakeOtlpSeries {
    name: &'static str,
    instrument: &'static str,
    unit: &'static str,
    latest: &'static str,
    aggregation: &'static str,
    scope: &'static str,
    resource: &'static str,
    attributes: &'static str,
    samples: &'static [u64],
}

#[cfg(feature = "tui-preview")]
const FAKE_OTLP_COLUMNS: [PaneTableColumn<'static>; 3] = [
    PaneTableColumn {
        id: "metric",
        title: "Metric",
        width: 28,
        required: true,
        default_visible: true,
        policy: CellPolicy::Ellipsis,
    },
    PaneTableColumn {
        id: "instrument",
        title: "Instrument",
        width: 11,
        required: true,
        default_visible: true,
        policy: CellPolicy::Ellipsis,
    },
    PaneTableColumn {
        id: "latest",
        title: "Latest",
        width: 13,
        required: true,
        default_visible: true,
        policy: CellPolicy::Ellipsis,
    },
];

#[cfg(feature = "tui-preview")]
const FAKE_OTLP_SERIES: [FakeOtlpSeries; 7] = [
    FakeOtlpSeries {
        name: "golem.agent.invocations",
        instrument: "Counter",
        unit: "{invocation}",
        latest: "12,418",
        aggregation: "sum · 15m",
        scope: "golem.agent.runtime@demo",
        resource: "service.name=checkout-agent",
        attributes: "component=checkout, agent.type=CartAgent",
        samples: &[
            12, 14, 15, 19, 22, 20, 24, 31, 29, 34, 39, 37, 45, 48, 51, 58,
        ],
    },
    FakeOtlpSeries {
        name: "golem.agent.invocation.duration",
        instrument: "Histogram",
        unit: "ms",
        latest: "42 ms p95",
        aggregation: "p95 · 15m",
        scope: "golem.agent.runtime@demo",
        resource: "service.name=checkout-agent",
        attributes: "component=checkout, agent.type=CartAgent",
        samples: &[
            28, 31, 30, 36, 33, 39, 41, 37, 44, 49, 46, 43, 40, 45, 42, 42,
        ],
    },
    FakeOtlpSeries {
        name: "golem.agent.failures",
        instrument: "Counter",
        unit: "{failure}",
        latest: "7",
        aggregation: "sum · 15m",
        scope: "golem.agent.runtime@demo",
        resource: "service.name=checkout-agent",
        attributes: "component=checkout, error.kind=timeout",
        samples: &[0, 0, 1, 0, 0, 1, 0, 2, 0, 0, 0, 1, 0, 1, 0, 1],
    },
    FakeOtlpSeries {
        name: "golem.agent.memory.usage",
        instrument: "Gauge",
        unit: "MiBy",
        latest: "18.6 MiB",
        aggregation: "last · 1m",
        scope: "golem.agent.runtime@demo",
        resource: "service.name=checkout-agent",
        attributes: "component=checkout, agent.id=cart-17",
        samples: &[
            14, 15, 15, 16, 17, 16, 17, 18, 19, 18, 18, 19, 20, 19, 18, 19,
        ],
    },
    FakeOtlpSeries {
        name: "demo.cart.items",
        instrument: "UpDownCounter",
        unit: "{item}",
        latest: "3",
        aggregation: "last · 1m",
        scope: "demo.checkout@0.1.0",
        resource: "service.name=checkout-agent",
        attributes: "component=checkout, cart.region=eu-central",
        samples: &[1, 1, 2, 2, 4, 3, 3, 5, 4, 2, 3, 6, 5, 4, 4, 3],
    },
    FakeOtlpSeries {
        name: "demo.orders.completed",
        instrument: "Counter",
        unit: "{order}",
        latest: "942",
        aggregation: "sum · 15m",
        scope: "demo.orders@0.1.0",
        resource: "service.name=order-agent",
        attributes: "component=orders, agent.type=OrderAgent",
        samples: &[
            31, 33, 36, 35, 39, 41, 45, 44, 48, 52, 51, 55, 58, 61, 63, 66,
        ],
    },
    FakeOtlpSeries {
        name: "demo.payment.queue.depth",
        instrument: "Gauge",
        unit: "{request}",
        latest: "11",
        aggregation: "last · 1m",
        scope: "demo.payments@0.1.0",
        resource: "service.name=payment-agent",
        attributes: "component=payments, queue=settlement",
        samples: &[4, 6, 5, 8, 12, 10, 7, 9, 14, 18, 15, 13, 12, 10, 9, 11],
    },
];

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct AgentIdentity {
    component: String,
    agent_id: String,
}

const AGENT_COLUMNS: [PaneTableColumn<'static>; 7] = [
    PaneTableColumn {
        id: "agent_id",
        title: "AgentID",
        width: 32,
        required: true,
        default_visible: true,
        policy: CellPolicy::Ellipsis,
    },
    PaneTableColumn {
        id: "status",
        title: "Status",
        width: 14,
        required: true,
        default_visible: true,
        policy: CellPolicy::Ellipsis,
    },
    PaneTableColumn {
        id: "type",
        title: "Type",
        width: 22,
        required: false,
        default_visible: true,
        policy: CellPolicy::Ellipsis,
    },
    PaneTableColumn {
        id: "component",
        title: "Component",
        width: 28,
        required: false,
        default_visible: true,
        policy: CellPolicy::Ellipsis,
    },
    PaneTableColumn {
        id: "revision",
        title: "Revision",
        width: 10,
        required: false,
        default_visible: false,
        policy: CellPolicy::Ellipsis,
    },
    PaneTableColumn {
        id: "pending",
        title: "Pending",
        width: 10,
        required: false,
        default_visible: false,
        policy: CellPolicy::Ellipsis,
    },
    PaneTableColumn {
        id: "created_at",
        title: "Created at",
        width: 24,
        required: false,
        default_visible: false,
        policy: CellPolicy::Ellipsis,
    },
];

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

    fn agent_list_mode(self) -> AgentListMode {
        match self {
            Self::Durable => AgentListMode::Durable,
            Self::Ephemeral => AgentListMode::Ephemeral,
            Self::All => AgentListMode::All,
        }
    }
}

#[derive(Clone, Debug)]
struct AgentListItem {
    agent_id: String,
    component: Option<String>,
    agent_type: Option<String>,
    revision: String,
    pending: String,
    created_at: String,
    status: AgentStatus,
    last_error_kind: Option<OplogErrorKind>,
    raw: Value,
}

impl AgentListItem {
    fn identity(&self) -> AgentIdentity {
        AgentIdentity {
            component: self.component.clone().unwrap_or_default(),
            agent_id: self.agent_id.clone(),
        }
    }

    fn status_label(&self) -> String {
        if self.last_error_kind == Some(OplogErrorKind::Recovery) {
            "Unavailable".to_string()
        } else {
            self.status.to_string()
        }
    }

    fn status_tone(&self) -> CellTone {
        if self.last_error_kind == Some(OplogErrorKind::Recovery) {
            return CellTone::Muted;
        }
        match self.status {
            AgentStatus::Running => CellTone::Success,
            AgentStatus::Idle => CellTone::Info,
            AgentStatus::Suspended | AgentStatus::Retrying => CellTone::Warning,
            AgentStatus::Interrupted | AgentStatus::Failed => CellTone::Error,
            AgentStatus::Exited => CellTone::Muted,
        }
    }
}

fn agent_table_row(agent: &AgentListItem) -> Vec<String> {
    vec![
        agent.agent_id.clone(),
        agent.status_label(),
        agent.agent_type.clone().unwrap_or_else(|| "-".to_string()),
        agent.component.clone().unwrap_or_else(|| "-".to_string()),
        agent.revision.clone(),
        agent.pending.clone(),
        agent.created_at.clone(),
    ]
}

fn agent_id_highlight_tone(kind: AgentIdHighlightKind) -> Option<CellTone> {
    match kind {
        AgentIdHighlightKind::Str => Some(CellTone::Success),
        AgentIdHighlightKind::Num => Some(CellTone::Info),
        AgentIdHighlightKind::Lit => Some(CellTone::Warning),
        AgentIdHighlightKind::Open
        | AgentIdHighlightKind::Close
        | AgentIdHighlightKind::Comma
        | AgentIdHighlightKind::Punct => Some(CellTone::Muted),
        AgentIdHighlightKind::Ident => None,
    }
}

fn agent_id_table_cell(agent_id: &str) -> PaneTableCell {
    PaneTableCell {
        spans: highlight_agent_id(agent_id)
            .into_iter()
            .map(|span| PaneTableCellSpan {
                text: span.text.to_string(),
                tone: span.kind.and_then(agent_id_highlight_tone),
            })
            .collect(),
    }
}

fn agent_id_line_spans(agent_id: &str) -> Vec<Span<'static>> {
    highlight_agent_id(agent_id)
        .into_iter()
        .map(|span| {
            let color = span
                .kind
                .and_then(agent_id_highlight_tone)
                .map_or(theme().text, |tone| tone.color(&theme()));
            Span::styled(span.text.to_string(), Style::default().fg(color))
        })
        .collect()
}

pub(super) struct AgentRefreshPayload {
    agents: AgentsMetadataResponseView,
    cursor: AgentListPageCursor,
    agent_types: Option<Result<Vec<DeployedRegisteredAgentType>, String>>,
}

#[derive(Clone, Debug)]
pub(super) struct OpsEnvironmentSnapshot {
    account_id: String,
    application_id: String,
    application_name: String,
    environment_id: String,
    environment_name: String,
    environment_revision: String,
    diff_model_version: u32,
    compatibility_check: bool,
    tool_compatibility_mode: String,
    version_check: bool,
    security_overrides: bool,
    deployment: Option<OpsDeploymentSnapshot>,
}

#[derive(Clone, Debug)]
struct OpsDeploymentSnapshot {
    current_revision: String,
    deployment_revision: String,
    version: String,
    hash: String,
}

#[derive(Default)]
struct OpsEnvironmentState {
    generation: u64,
    context_id: Option<TuiContextId>,
    loading: bool,
    snapshot: Option<OpsEnvironmentSnapshot>,
    error: Option<String>,
}

fn ops_environment_snapshot(
    environment: crate::model::environment::ResolvedEnvironmentIdentity,
) -> OpsEnvironmentSnapshot {
    let server_environment = environment.server_environment;
    let deployment =
        server_environment
            .current_deployment
            .map(|deployment| OpsDeploymentSnapshot {
                current_revision: deployment.revision.to_string(),
                deployment_revision: deployment.deployment_revision.to_string(),
                version: deployment.deployment_version.0,
                hash: deployment.deployment_hash.to_string(),
            });
    OpsEnvironmentSnapshot {
        account_id: environment.account_id.to_string(),
        application_id: environment.application_id.to_string(),
        application_name: environment.application_name.0,
        environment_id: environment.environment_id.to_string(),
        environment_name: environment.environment_name.0,
        environment_revision: server_environment.revision.to_string(),
        diff_model_version: server_environment.diff_model_version,
        compatibility_check: server_environment.compatibility_check,
        tool_compatibility_mode: format!("{:?}", server_environment.tool_compatibility_mode),
        version_check: server_environment.version_check,
        security_overrides: server_environment.security_overrides,
        deployment,
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
    terminal: TerminalScreen,
    runtime: Option<NestedCliRuntime>,
    spinner_frame: usize,
    spinner_stop: Option<Arc<AtomicBool>>,
    exit_code: Option<i32>,
    ports_file: Option<PathBuf>,
    ports_file_before_modified: Option<SystemTime>,
    bound_ports: Option<BoundServerPorts>,
    stop_requested_at: Option<Instant>,
}

impl Default for ServerRun {
    fn default() -> Self {
        Self {
            id: 0,
            status: ServerStatus::Stopped,
            clean: false,
            args: server_args(false),
            launch_context: String::new(),
            terminal: TerminalScreen::new(20, 80),
            runtime: None,
            spinner_frame: 0,
            spinner_stop: None,
            exit_code: None,
            ports_file: None,
            ports_file_before_modified: None,
            bound_ports: None,
            stop_requested_at: None,
        }
    }
}

impl ServerRun {
    fn new(
        id: u64,
        clean: bool,
        args: Vec<String>,
        launch_context: String,
        ports_file: Option<PathBuf>,
        ports_file_before_modified: Option<SystemTime>,
    ) -> Self {
        Self {
            id,
            status: ServerStatus::Starting,
            clean,
            args,
            launch_context,
            terminal: TerminalScreen::new(20, 80),
            runtime: None,
            spinner_frame: 0,
            spinner_stop: None,
            exit_code: None,
            ports_file,
            ports_file_before_modified,
            bound_ports: None,
            stop_requested_at: None,
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

#[derive(Debug, Clone)]
struct ServerFormValues {
    system_memory_override: String,
    router_address: String,
    router_port: String,
    custom_request_port: String,
    mcp_port: String,
    ports_file: String,
    data_dir: String,
    agent_filesystem_root: String,
    clean: bool,
    yes: bool,
}

impl ServerFormValues {
    fn new(config_dir: &str) -> Self {
        Self {
            system_memory_override: String::new(),
            router_address: String::new(),
            router_port: String::new(),
            custom_request_port: String::new(),
            mcp_port: String::new(),
            ports_file: PathBuf::from(config_dir)
                .join("tui-server-ports.json")
                .display()
                .to_string(),
            data_dir: String::new(),
            agent_filesystem_root: String::new(),
            clean: false,
            yes: false,
        }
    }
}

#[derive(Debug, Clone)]
struct ReplFormValues {
    language: String,
    stream_logs: bool,
    auto_imports: bool,
    post_deploy: PostDeployPolicy,
    yes: bool,
}

impl Default for ReplFormValues {
    fn default() -> Self {
        Self {
            language: "typescript".to_string(),
            stream_logs: true,
            auto_imports: true,
            post_deploy: PostDeployPolicy::None,
            yes: false,
        }
    }
}

#[derive(Debug, Clone)]
enum SessionReviewForm {
    Server {
        values: ServerFormValues,
        selected: usize,
        editing: bool,
        restart: bool,
        error: Option<String>,
    },
    Repl {
        values: ReplFormValues,
        selected: usize,
        restart: bool,
    },
}

impl SessionReviewForm {
    fn selected(&self) -> usize {
        match self {
            Self::Server { selected, .. } | Self::Repl { selected, .. } => *selected,
        }
    }

    fn field_count(&self) -> usize {
        match self {
            Self::Server { .. } => 11,
            Self::Repl { .. } => 6,
        }
    }

    fn set_selected(&mut self, value: usize) {
        match self {
            Self::Server { selected, .. } | Self::Repl { selected, .. } => *selected = value,
        }
    }
}

fn server_form_text_mut(values: &mut ServerFormValues, selected: usize) -> &mut String {
    match selected {
        0 => &mut values.system_memory_override,
        1 => &mut values.router_address,
        2 => &mut values.router_port,
        3 => &mut values.custom_request_port,
        4 => &mut values.mcp_port,
        5 => &mut values.ports_file,
        6 => &mut values.data_dir,
        _ => &mut values.agent_filesystem_root,
    }
}

fn update_session_form_field(form: &mut SessionReviewForm) {
    match form {
        SessionReviewForm::Server {
            values,
            selected,
            editing,
            ..
        } => match *selected {
            0..=7 => *editing = true,
            8 => values.clean = !values.clean,
            9 => values.yes = !values.yes,
            _ => {}
        },
        SessionReviewForm::Repl {
            values, selected, ..
        } => match *selected {
            0 => {}
            1 => values.stream_logs = !values.stream_logs,
            2 => values.auto_imports = !values.auto_imports,
            3 => values.post_deploy = values.post_deploy.next(),
            4 => values.yes = !values.yes,
            _ => {}
        },
    }
}

fn validate_server_form(values: &ServerFormValues) -> Result<(), String> {
    for (label, value) in [
        ("router port", &values.router_port),
        ("custom request port", &values.custom_request_port),
        ("MCP port", &values.mcp_port),
    ] {
        if !value.trim().is_empty() && value.trim().parse::<u16>().is_err() {
            return Err(format!("{label} must be a number from 0 to 65535"));
        }
    }
    if values.ports_file.trim().is_empty() {
        return Err("ports file is required so bound ports can be observed".to_string());
    }
    Ok(())
}

fn push_option(args: &mut Vec<String>, name: &str, value: &str) {
    if !value.trim().is_empty() {
        args.extend([name.to_string(), value.trim().to_string()]);
    }
}

fn server_args_from_values(values: &ServerFormValues) -> Vec<String> {
    let mut args = vec!["server".to_string(), "run".to_string()];
    push_option(
        &mut args,
        "--system-memory-override",
        &values.system_memory_override,
    );
    push_option(&mut args, "--router-addr", &values.router_address);
    push_option(&mut args, "--router-port", &values.router_port);
    push_option(
        &mut args,
        "--custom-request-port",
        &values.custom_request_port,
    );
    push_option(&mut args, "--mcp-port", &values.mcp_port);
    push_option(&mut args, "--ports-file", &values.ports_file);
    push_option(&mut args, "--data-dir", &values.data_dir);
    push_option(
        &mut args,
        "--agent-filesystem-root",
        &values.agent_filesystem_root,
    );
    if values.clean {
        args.push("--clean".to_string());
    }
    if values.yes {
        args.push("--yes".to_string());
    }
    args
}

fn repl_args_from_values(values: &ReplFormValues) -> Vec<String> {
    let mut args = vec!["repl".to_string()];
    push_option(&mut args, "--language", &values.language);
    if !values.stream_logs {
        args.push("--disable-stream".to_string());
    }
    if !values.auto_imports {
        args.push("--disable-auto-imports".to_string());
    }
    match values.post_deploy {
        PostDeployPolicy::None => {}
        PostDeployPolicy::UpdateAuto => {
            args.extend(["--update-agents".to_string(), "auto".to_string()])
        }
        PostDeployPolicy::UpdateManual => {
            args.extend(["--update-agents".to_string(), "manual".to_string()])
        }
        PostDeployPolicy::Redeploy => args.push("--redeploy-agents".to_string()),
        PostDeployPolicy::Reset => args.push("--reset".to_string()),
    }
    if values.yes {
        args.push("--yes".to_string());
    }
    args
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct BoundServerPorts {
    router_port: u16,
    custom_request_port: u16,
    mcp_port: u16,
}

struct ReplRun {
    status: ReplStatus,
    args: Vec<String>,
    launch_context: String,
    screen: TerminalScreen,
    runtime: Option<NestedCliRuntime>,
    exit_code: Option<i32>,
    last_error: Option<String>,
    stop_requested_at: Option<Instant>,
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
            stop_requested_at: None,
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
            stop_requested_at: None,
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

fn fresh_bound_server_ports(run: &ServerRun) -> Option<BoundServerPorts> {
    let path = run.ports_file.as_ref()?;
    let metadata = std::fs::metadata(path).ok()?;
    let modified = metadata.modified().ok()?;
    if run
        .ports_file_before_modified
        .is_some_and(|before| modified <= before)
    {
        return None;
    }
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

fn agent_items_from_metadata_response(response: AgentsMetadataResponseView) -> Vec<AgentListItem> {
    response
        .agents
        .into_iter()
        .map(agent_item_from_metadata)
        .collect()
}

fn agent_item_from_metadata(agent: AgentMetadataView) -> AgentListItem {
    let agent_id = agent.agent_id.0.clone();
    let component = Some(agent.component_name.0.clone());
    let agent_type = agent_type_from_agent_name(&agent_id);
    let status = agent.status.clone();
    let last_error_kind = agent.last_error_kind.clone();
    let revision = agent.component_revision.to_string();
    let pending = agent.pending_invocation_count.to_string();
    let created_at = agent.created_at.to_string();
    let raw = serde_json::to_value(agent).unwrap_or(Value::Null);

    AgentListItem {
        agent_id,
        component,
        agent_type,
        revision,
        pending,
        created_at,
        status,
        last_error_kind,
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
    let error = plain_tui_text(&error);
    if logs.is_empty() {
        error
    } else {
        let logs = logs
            .iter()
            .map(|line| plain_tui_text(line))
            .collect::<Vec<_>>()
            .join("\n");
        format!("{error}\n{logs}")
    }
}

fn plain_tui_text(text: &str) -> String {
    strip_ansi_escapes::strip_str(text)
}

fn physical_ratio_delta(
    area: Rect,
    placement: layout::PanePlacement,
    direction: KeyCode,
    cells: u16,
) -> i16 {
    let total = match placement {
        layout::PanePlacement::Right => area.width,
        layout::PanePlacement::Bottom => area.height,
    }
    .max(1);
    let magnitude = cells.saturating_mul(100).div_ceil(total).max(1) as i16;
    match (placement, direction) {
        (layout::PanePlacement::Right, KeyCode::Left)
        | (layout::PanePlacement::Bottom, KeyCode::Up) => -magnitude,
        (layout::PanePlacement::Right, KeyCode::Right)
        | (layout::PanePlacement::Bottom, KeyCode::Down) => magnitude,
        _ => 0,
    }
}

fn apply_ratio_delta(ratio: u16, delta: i16) -> u16 {
    (ratio as i16 + delta).clamp(0, 100) as u16
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TuiPreferences {
    version: u32,
    dev_placement: String,
    dev_primary_ratio: u16,
    dev_secondary_ratio: u16,
    ops_details_placement: String,
    ops_details_ratio: u16,
    agent_columns: Vec<String>,
}

impl TuiPreferences {
    const VERSION: u32 = 1;

    fn from_state(layout: &TuiLayoutState, table: &PaneTableState) -> Self {
        Self {
            version: Self::VERSION,
            dev_placement: layout.dev_placement.label().to_string(),
            dev_primary_ratio: layout.dev_primary_ratio,
            dev_secondary_ratio: layout.dev_secondary_ratio,
            ops_details_placement: layout.ops_details_placement.label().to_string(),
            ops_details_ratio: layout.ops_details_ratio,
            agent_columns: table.visible_columns().to_vec(),
        }
    }

    fn apply(&self, layout: &mut TuiLayoutState, table: &mut PaneTableState) {
        if self.version != Self::VERSION {
            return;
        }
        layout.dev_placement =
            placement_from_preference(&self.dev_placement).unwrap_or(layout::PanePlacement::Right);
        layout.dev_primary_ratio = self.dev_primary_ratio.min(100);
        layout.dev_secondary_ratio = self.dev_secondary_ratio.min(100);
        layout.ops_details_placement = placement_from_preference(&self.ops_details_placement)
            .unwrap_or(layout::PanePlacement::Right);
        layout.ops_details_ratio = self.ops_details_ratio.min(100);
        table.restore_visible_columns(&AGENT_COLUMNS, &self.agent_columns);
    }
}

fn placement_from_preference(value: &str) -> Option<layout::PanePlacement> {
    match value {
        "right" => Some(layout::PanePlacement::Right),
        "bottom" => Some(layout::PanePlacement::Bottom),
        _ => None,
    }
}

fn tui_preferences_path(config_dir: &str) -> PathBuf {
    PathBuf::from(config_dir)
        .join("tui")
        .join("preferences.json")
}

fn load_tui_preferences(path: &std::path::Path) -> (Option<TuiPreferences>, Option<String>) {
    let content = match std::fs::read(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return (None, None),
        Err(error) => {
            return (
                None,
                Some(format!("failed to read TUI preferences: {error}")),
            );
        }
    };
    match serde_json::from_slice::<TuiPreferences>(&content) {
        Ok(preferences) if preferences.version == TuiPreferences::VERSION => {
            (Some(preferences), None)
        }
        Ok(preferences) => (
            None,
            Some(format!(
                "unsupported TUI preference version {}",
                preferences.version
            )),
        ),
        Err(error) => (None, Some(format!("invalid TUI preferences: {error}"))),
    }
}

fn write_tui_preferences(
    path: &std::path::Path,
    preferences: &TuiPreferences,
) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("TUI preference path has no parent"))?;
    std::fs::create_dir_all(parent)?;
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, serde_json::to_vec_pretty(preferences)?)?;
    std::fs::rename(temporary, path)?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TuiWorkspace {
    Dev,
    Ops,
}

impl TuiWorkspace {
    pub(super) const ALL: [Self; 2] = [Self::Dev, Self::Ops];

    pub(super) fn title(self) -> &'static str {
        match self {
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
pub(super) enum DevView {
    BuildDeploy,
    Server,
    Repl,
}

impl DevView {
    pub(super) const ALL: [Self; 3] = [Self::BuildDeploy, Self::Server, Self::Repl];

    pub(super) fn title(self) -> &'static str {
        match self {
            Self::BuildDeploy => "Build & Deploy",
            Self::Repl => "REPL",
            Self::Server => "Server",
        }
    }

    pub(super) fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|panel| *panel == self)
            .unwrap_or(0)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum PaletteScope {
    #[default]
    All,
    Context,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum PaletteReturnMode {
    #[default]
    Default,
    AgentFilter,
    CommandForm,
    SessionForm,
}

#[derive(Debug, Clone, Default)]
struct CommandPalette {
    query: String,
    selected: usize,
    scope: PaletteScope,
    return_mode: PaletteReturnMode,
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
            error: Some(plain_tui_text(&error)),
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

    fn stable_id(&self) -> String {
        format!("{}/{}/{}", self.application, self.environment, self.server)
    }

    fn provenance(&self) -> &'static str {
        if self.application != "no application manifest" && self.dev_eligible {
            "manifest application environment"
        } else if self.uses_local_server {
            "built-in local target"
        } else {
            "selected remote or profile target"
        }
    }

    fn capability(&self, view: DevView) -> Result<(), &'static str> {
        match view {
            DevView::BuildDeploy if !self.dev_eligible => {
                Err("requires a manifest-backed application")
            }
            DevView::BuildDeploy if !self.uses_local_server => {
                Err("remote Dev mutations are not enabled in this slice")
            }
            DevView::Server if !self.uses_local_server => Err("requires a local server target"),
            DevView::Repl if !self.dev_eligible => {
                Err("requires a selected application environment")
            }
            _ => Ok(()),
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

#[cfg(feature = "tui-preview")]
#[derive(Clone, Copy, Eq, PartialEq)]
enum DesignLabScene {
    OpsAgents,
    OpsMetricsDemo,
    ActivityTimeline,
    ActivityJournal,
    ShellDefault,
    ShellScrollable,
    ContentDensity,
    ContentLong,
    ContentScrolling,
    ContentSplit,
    SplitFocus,
    SplitScrollable,
    ThreePaneScrollable,
    ShortcutLeader,
    OverlaySearch,
    OverlayDecision,
    OverlayError,
    OverlayNested,
    TableDecoration,
    TableLong,
    TableDetails,
    TableColumns,
}

#[cfg(feature = "tui-preview")]
impl DesignLabScene {
    fn parse(name: &str) -> anyhow::Result<Self> {
        match name {
            "ops-agents" => Ok(Self::OpsAgents),
            "ops-metrics-demo" => Ok(Self::OpsMetricsDemo),
            "activity-timeline" => Ok(Self::ActivityTimeline),
            "activity-journal" => Ok(Self::ActivityJournal),
            "shell-default" => Ok(Self::ShellDefault),
            "shell-compact" => Ok(Self::ShellDefault),
            "shell-scrollbar" => Ok(Self::ShellScrollable),
            "content-density" => Ok(Self::ContentDensity),
            "content-long" => Ok(Self::ContentLong),
            "content-scrolling" => Ok(Self::ContentScrolling),
            "content-split" => Ok(Self::ContentSplit),
            "split-focus" => Ok(Self::SplitFocus),
            "split-scrollbars" => Ok(Self::SplitScrollable),
            "three-pane-scrollbars" => Ok(Self::ThreePaneScrollable),
            "shortcut-leader" => Ok(Self::ShortcutLeader),
            "overlay-search" => Ok(Self::OverlaySearch),
            "overlay-decision" => Ok(Self::OverlayDecision),
            "overlay-error" => Ok(Self::OverlayError),
            "overlay-nested" => Ok(Self::OverlayNested),
            "table-decoration" => Ok(Self::TableDecoration),
            "table-long" => Ok(Self::TableLong),
            "table-details" => Ok(Self::TableDetails),
            "table-columns" => Ok(Self::TableColumns),
            _ => anyhow::bail!("unknown TUI design-lab scene: {name}"),
        }
    }
}

fn render_tree(frame: &mut Frame<'_>, app: &TuiApp) {
    let context_picker_rows = context_picker_layout_rows(&app.context_switcher);
    let snapshot = layout::compute(LayoutInput {
        area: frame.area(),
        active_workspace: app.active_workspace,
        dev_primary: app.dev_primary,
        focused_dev_view: app.dev_focus,
        mode: app.mode,
        context_picker_rows,
        context_picker_selected: app.context_switcher.selected,
        context_picker_prefix_height: context_picker_prefix_height(&app.context_switcher),
        agent_details_visible: app.ops_agents_focused() && app.agents.detail_visible,
        layout: app.layout.clone(),
    });
    app.layout_snapshot.replace(Some(snapshot.clone()));

    render_shell_header(frame, snapshot.header, app);

    match app.active_workspace {
        TuiWorkspace::Dev => render_dev_workspace(frame, snapshot.workspace_body, app),
        TuiWorkspace::Ops => render_ops_workspace(frame, snapshot.workspace_body, app),
    }
    render_shell_footer(frame, snapshot.footer, app);

    match app.mode {
        TuiMode::Normal => {}
        TuiMode::Leader => render_leader_hint(frame, app),
        TuiMode::Layout => {}
        TuiMode::Jobs => render_jobs(frame, app),
        TuiMode::QuitBlocked => render_quit_blocked(frame, app),
        TuiMode::ContextPicker => render_context_picker(frame, app),
        TuiMode::ContextSwitchConfirm => render_context_switch_confirm(frame, app),
        TuiMode::AgentFilter => {}
        TuiMode::AgentDatasetFilter => render_agent_dataset_filter(frame, app),
        TuiMode::AgentColumns => render_agent_columns(frame, app),
        TuiMode::CommandForm => render_command_form(frame, app),
        TuiMode::SessionForm => render_session_form(frame, app),
        TuiMode::CommandInteraction => {}
        TuiMode::Repl => {}
        TuiMode::TerminalScrollback => {}
        TuiMode::Palette => render_palette(frame, app),
        TuiMode::Help => render_help(frame, app),
    }
}

fn render_ops_workspace(frame: &mut Frame<'_>, area: Rect, app: &TuiApp) {
    frame.render_widget(Paragraph::new("").style(surface_style()), area);
    match (&app.ops_subject, app.ops_view) {
        (OpsSubject::Root(OpsRootSubject::Agents), OpsView::Overview) => {
            render_ops_agents_overview(frame, area, app)
        }
        (OpsSubject::Agent(identity), OpsView::Overview) => {
            render_ops_agent_overview(frame, area, app, identity)
        }
        (OpsSubject::Root(OpsRootSubject::Deployment), OpsView::Overview) => {
            render_ops_deployment_overview(frame, area, app)
        }
        (OpsSubject::Root(OpsRootSubject::Environment), OpsView::Overview) => {
            render_ops_environment_overview(frame, area, app)
        }
        (OpsSubject::Root(OpsRootSubject::Server), OpsView::Overview) => {
            render_ops_server_overview(frame, area, app)
        }
        (_, OpsView::Activity) => render_ops_unavailable(
            frame,
            area,
            &format!("{} · Activity", app.ops_subject.label()),
            "Activity scope and oplog providers are not implemented yet.",
        ),
        (_, OpsView::Metrics) => render_ops_unavailable(
            frame,
            area,
            &format!("{} · Metrics", app.ops_subject.label()),
            "No production Metrics provider is configured. Fake OTLP data is preview-only.",
        ),
    }
}

fn render_ops_unavailable(frame: &mut Frame<'_>, area: Rect, title: &str, message: &str) {
    let [header, body] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .areas(area);
    PaneHeader {
        title,
        focus: PaneFocus::Active,
        left_connector: Some("├"),
        ending: PaneEnding::Top,
    }
    .render(frame, header, &theme());
    frame.render_widget(
        Paragraph::new(
            Notice {
                kind: NoticeKind::Info,
                message,
            }
            .line(&theme()),
        )
        .style(surface_style()),
        body,
    );
}

fn render_ops_agent_overview(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &TuiApp,
    identity: &AgentIdentity,
) {
    let agent = app
        .agents
        .agents
        .iter()
        .find(|agent| agent.identity() == *identity);
    let title = format!("Agents / {} · Overview", identity.agent_id);
    let lines = build_ops_agent_details_lines(
        agent,
        app.agents.included.len(),
        &app.agents.agent_types,
        app.agents.metadata_error.as_deref(),
        area.width,
    );
    render_ops_document(frame, area, app, &title, lines);
}

fn render_ops_deployment_overview(frame: &mut Frame<'_>, area: Rect, app: &TuiApp) {
    let mut lines = ops_environment_status_lines(app);
    if let Some(snapshot) = &app.ops_environment.snapshot {
        lines.push(
            SectionHeading {
                title: "Current deployment",
                detail: None,
            }
            .line(&theme()),
        );
        match &snapshot.deployment {
            Some(deployment) => {
                for (label, value) in [
                    ("Application", snapshot.application_name.as_str()),
                    ("Environment", snapshot.environment_name.as_str()),
                    ("Revision", deployment.deployment_revision.as_str()),
                    ("Version", deployment.version.as_str()),
                    ("Hash", deployment.hash.as_str()),
                    ("Environment rev", deployment.current_revision.as_str()),
                ] {
                    lines.push(
                        FieldRow {
                            label,
                            value,
                            label_width: 16,
                        }
                        .line(&theme()),
                    );
                }
            }
            None => lines.push(
                Notice {
                    kind: NoticeKind::Empty,
                    message: "The selected environment has no current deployment",
                }
                .line(&theme()),
            ),
        }
    }
    render_ops_document(frame, area, app, "Deployment · Overview", lines);
}

fn render_ops_environment_overview(frame: &mut Frame<'_>, area: Rect, app: &TuiApp) {
    let mut lines = ops_environment_status_lines(app);
    if let Some(snapshot) = &app.ops_environment.snapshot {
        lines.push(
            SectionHeading {
                title: "Selected environment",
                detail: None,
            }
            .line(&theme()),
        );
        let diff_model_version = snapshot.diff_model_version.to_string();
        let compatibility_check = snapshot.compatibility_check.to_string();
        let version_check = snapshot.version_check.to_string();
        let security_overrides = snapshot.security_overrides.to_string();
        for (label, value) in [
            ("Application", snapshot.application_name.as_str()),
            ("Environment", snapshot.environment_name.as_str()),
            ("Account ID", snapshot.account_id.as_str()),
            ("Application ID", snapshot.application_id.as_str()),
            ("Environment ID", snapshot.environment_id.as_str()),
            ("Revision", snapshot.environment_revision.as_str()),
            ("Diff model", diff_model_version.as_str()),
            ("Compatibility", compatibility_check.as_str()),
            ("Tool mode", snapshot.tool_compatibility_mode.as_str()),
            ("Version check", version_check.as_str()),
            ("Security", security_overrides.as_str()),
        ] {
            lines.push(
                FieldRow {
                    label,
                    value,
                    label_width: 16,
                }
                .line(&theme()),
            );
        }
    }
    render_ops_document(frame, area, app, "Environment · Overview", lines);
}

fn render_ops_server_overview(frame: &mut Frame<'_>, area: Rect, app: &TuiApp) {
    let stable_id = app.context.stable_id();
    let provenance = app.context.provenance();
    let process = app.server.run.status.title();
    let bound_ports = app.server.run.bound_ports.as_ref().map_or_else(
        || "not observed".to_string(),
        |ports| {
            format!(
                "router {} · custom {} · MCP {}",
                ports.router_port, ports.custom_request_port, ports.mcp_port
            )
        },
    );
    let mut lines = vec![
        SectionHeading {
            title: "Selected server target",
            detail: None,
        }
        .line(&theme()),
    ];
    for (label, value) in [
        ("Target ID", stable_id.as_str()),
        ("Endpoint", app.context.server.as_str()),
        ("Provenance", provenance),
        ("Local process", process),
        ("Bound ports", bound_ports.as_str()),
    ] {
        lines.push(
            FieldRow {
                label,
                value,
                label_width: 14,
            }
            .line(&theme()),
        );
    }
    lines.push(Line::default());
    lines.push(
        Notice {
            kind: NoticeKind::Info,
            message: "Process and ports-file evidence is shown without claiming HTTP readiness",
        }
        .line(&theme()),
    );
    render_ops_document(frame, area, app, "Server · Overview", lines);
}

fn ops_environment_status_lines(app: &TuiApp) -> Vec<Line<'static>> {
    if app.ops_environment.loading && app.ops_environment.snapshot.is_none() {
        return vec![
            Notice {
                kind: NoticeKind::Info,
                message: "Loading selected environment through the typed read-only provider",
            }
            .line(&theme()),
        ];
    }
    if let Some(error) = &app.ops_environment.error {
        let message = if app.ops_environment.snapshot.is_some() {
            format!("Refresh failed; showing the last successful result: {error}")
        } else {
            format!("Environment lookup failed: {error}")
        };
        return vec![
            Notice {
                kind: NoticeKind::Error,
                message: &message,
            }
            .line(&theme()),
        ];
    }
    Vec::new()
}

fn render_ops_document(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &TuiApp,
    title: &str,
    lines: Vec<Line<'static>>,
) {
    let [header, body] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .areas(area);
    PaneHeader {
        title,
        focus: PaneFocus::Active,
        left_connector: Some("├"),
        ending: PaneEnding::Top,
    }
    .render(frame, header, &theme());
    let pane_layout = PaneLayout::horizontal(body, &[1]);
    let Some(region) = pane_layout.regions(&[lines.len()]).into_iter().next() else {
        return;
    };
    app.agents.details_content_height.set(lines.len());
    app.agents
        .details_viewport_height
        .set(region.content_area.height as usize);
    ScrollableDocument {
        lines: &lines,
        offset: app.agents.details_scroll,
    }
    .render(frame, region, &theme());
    pane_layout.render_body_boundaries(frame, &[region], &theme());
}

fn render_ops_agents_overview(frame: &mut Frame<'_>, area: Rect, app: &TuiApp) {
    let chrome = PaneChrome { area };
    let snapshot = app.layout_snapshot.borrow();
    let list_pane = snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.region(RegionKind::OpsList))
        .unwrap_or(area);
    let details_pane = snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.region(RegionKind::OpsDetails));
    let divider = snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.region(RegionKind::OpsDetailsSplit));
    drop(snapshot);
    let [list_header, list_body] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .areas(list_pane);
    chrome.render_header(
        frame,
        list_pane,
        "Agents · Overview",
        if app.agents.focus == AgentOverviewFocus::List || details_pane.is_none() {
            PaneFocus::Active
        } else {
            PaneFocus::Idle
        },
        &theme(),
    );
    let details_parts = details_pane.map(|pane| {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(1)])
            .areas::<2>(pane)
    });
    if let Some([details_header, _]) = details_parts {
        let details_pane = details_pane.expect("details parts require a details pane");
        chrome.render_header(
            frame,
            details_pane,
            "Details",
            if app.agents.focus == AgentOverviewFocus::Details {
                PaneFocus::Active
            } else {
                PaneFocus::Idle
            },
            &theme(),
        );
        debug_assert_eq!(
            details_header,
            Rect {
                height: details_pane.height.min(1),
                ..details_pane
            }
        );
    }
    let (refresh_label, refresh_tone) = if app.agents.refresh_running {
        (
            if app.agents.refresh_append {
                format!(
                    "{} loading next {AGENT_PAGE_SIZE}",
                    spinner_symbol(app.agents.refresh_spinner_frame)
                )
            } else {
                format!(
                    "{} refreshing",
                    spinner_symbol(app.agents.refresh_spinner_frame)
                )
            },
            PaneStatusTone::Loading,
        )
    } else if app.agents.auto_refresh {
        ("auto 5s".to_string(), PaneStatusTone::Active)
    } else {
        ("auto off".to_string(), PaneStatusTone::Idle)
    };
    PaneHeaderStatus {
        label: &refresh_label,
        tone: refresh_tone,
    }
    .render(frame, list_header, &theme());

    let filtered = app.filtered_agents();
    let owned_rows = filtered
        .iter()
        .map(|agent| agent_table_row(agent))
        .collect::<Vec<_>>();
    let row_cells = owned_rows
        .iter()
        .map(|row| row.iter().map(String::as_str).collect::<Vec<_>>())
        .collect::<Vec<_>>();
    let rows = row_cells.iter().map(Vec::as_slice).collect::<Vec<_>>();
    let owned_tones = filtered
        .iter()
        .map(|agent| {
            vec![
                None,
                Some(agent.status_tone()),
                None,
                None,
                None,
                None,
                None,
            ]
        })
        .collect::<Vec<_>>();
    let tones = owned_tones.iter().map(Vec::as_slice).collect::<Vec<_>>();
    let owned_rich_cells = filtered
        .iter()
        .map(|agent| {
            vec![
                Some(agent_id_table_cell(&agent.agent_id)),
                None,
                None,
                None,
                None,
                None,
                None,
            ]
        })
        .collect::<Vec<_>>();
    let rich_cells = owned_rich_cells
        .iter()
        .map(Vec::as_slice)
        .collect::<Vec<_>>();
    let row_markers = filtered
        .iter()
        .map(|agent| {
            if app.agents.included.contains(&agent.identity()) {
                "✓"
            } else {
                ""
            }
        })
        .collect::<Vec<_>>();
    let mut table_state = app.agents.table.clone();
    table_state.selected = app.agents.selected.min(filtered.len().saturating_sub(1));
    let table_content_height = PaneTable {
        columns: &AGENT_COLUMNS,
        rows: &rows,
        state: &table_state,
        decoration: TableDecoration::Zebra,
        cell_tones: Some(&tones),
        rich_cells: Some(&rich_cells),
        row_markers: Some(&row_markers),
        first_row_index: 0,
    }
    .height();
    let query_bar = CollectionQueryBar {
        dataset: &app.agents.dataset_label(),
        scope: app.agents.query_scope.label(),
        query: &app.agents.query,
        matched: filtered.len(),
        loaded: app.agents.agents.len(),
        more_available: app.agents.has_more(),
        editing: app.mode == TuiMode::AgentFilter,
    };
    let query_height = query_bar.height() as usize;
    let stale_result_notice = usize::from(app.agents.last_error.is_some() && !filtered.is_empty());
    let table_height = table_content_height
        .saturating_add(stale_result_notice)
        .saturating_add(query_height);
    let details_body = details_parts.map(|[_, body]| body);
    let details_width = details_body.map_or(0, |body| chrome.content_area(body).width);
    let selected_agent = app.agents.selected_agent(&filtered);
    let mut details_lines = build_ops_agent_details_lines(
        selected_agent,
        app.agents.included.len(),
        &app.agents.agent_types,
        app.agents.metadata_error.as_deref(),
        details_width,
    );
    let table_region = chrome.region(list_body, table_height);
    let mut details_region = details_body.map(|body| chrome.region(body, details_lines.len()));
    if let Some(region) = details_region
        && region.scrollbar.is_some()
    {
        details_lines = build_ops_agent_details_lines(
            selected_agent,
            app.agents.included.len(),
            &app.agents.agent_types,
            app.agents.metadata_error.as_deref(),
            region.content_area.width,
        );
        details_region = details_body.map(|body| chrome.region(body, details_lines.len()));
    }

    let mut table_area = table_region.content_area;
    let query_area = Rect {
        height: table_area.height.min(query_height as u16),
        ..table_area
    };
    frame.render_widget(
        Paragraph::new(query_bar.lines(&theme())).style(surface_style()),
        query_area,
    );
    table_area.y = table_area.y.saturating_add(query_area.height);
    table_area.height = table_area.height.saturating_sub(query_area.height);
    if let Some(error) = &app.agents.last_error {
        let message = if filtered.is_empty() {
            format!("Agent refresh failed: {error}")
        } else {
            format!("Refresh failed; showing the last successful result: {error}")
        };
        let notice_area = Rect {
            height: table_area.height.min(1),
            ..table_area
        };
        frame.render_widget(
            Paragraph::new(
                Notice {
                    kind: NoticeKind::Error,
                    message: &message,
                }
                .line(&theme()),
            )
            .style(surface_style()),
            notice_area,
        );
        if !filtered.is_empty() {
            table_area.y = table_area.y.saturating_add(notice_area.height);
            table_area.height = table_area.height.saturating_sub(notice_area.height);
        }
    }

    let data_viewport_width = table_area.width.saturating_sub(2);
    app.agents.table_viewport_width.set(data_viewport_width);
    app.agents.table_viewport_height.set(table_area.height);
    let resolved_columns =
        responsive_table_columns(&AGENT_COLUMNS, &rows, &table_state, data_viewport_width);
    let virtual_width = pane_table_virtual_width(&resolved_columns, &table_state);
    app.agents.table_virtual_width.set(virtual_width);
    table_state.clamp_offset_to_width(virtual_width, data_viewport_width);
    let table_window = PaneTable {
        columns: &resolved_columns,
        rows: &rows,
        state: &table_state,
        decoration: TableDecoration::Zebra,
        cell_tones: Some(&tones),
        rich_cells: Some(&rich_cells),
        row_markers: Some(&row_markers),
        first_row_index: 0,
    }
    .window_from(table_area.height, app.agents.table_first_visible_row.get());
    app.agents
        .table_first_visible_row
        .set(table_window.start_row);

    if app.agents.last_error.is_some() && filtered.is_empty() {
    } else if app.agents.refresh_running && filtered.is_empty() {
        frame.render_widget(
            Paragraph::new(Notice::animated_loading(
                "Refreshing agents",
                spinner_symbol(app.agents.refresh_spinner_frame),
                &theme(),
            ))
            .style(surface_style()),
            table_area,
        );
    } else if filtered.is_empty() {
        frame.render_widget(
            Paragraph::new(
                Notice {
                    kind: NoticeKind::Empty,
                    message: "No agents match the current filter",
                }
                .line(&theme()),
            )
            .style(surface_style()),
            table_area,
        );
    } else {
        let start = table_window.start_row;
        let visible_rows = &rows[start..];
        let visible_tones = &tones[start..];
        let visible_rich_cells = &rich_cells[start..];
        let visible_markers = &row_markers[start..];
        table_state.selected = table_window.selected_row;
        PaneTable {
            columns: &resolved_columns,
            rows: visible_rows,
            state: &table_state,
            decoration: TableDecoration::Zebra,
            cell_tones: Some(visible_tones),
            rich_cells: Some(visible_rich_cells),
            row_markers: Some(visible_markers),
            first_row_index: start,
        }
        .render(frame, table_area, &theme());
    }

    chrome.render_body_boundary(frame, list_body, &theme());
    if let Some(slot) = table_region.scrollbar {
        let mut state = ScrollbarState::new(table_height)
            .position(
                query_height
                    .saturating_add(stale_result_notice)
                    .saturating_add(table_window.selected_line_offset),
            )
            .viewport_content_length(table_region.content_area.height as usize);
        render_scrollbar(frame, slot, &mut state, &theme());
    }
    if let (Some(details_body), Some(details_region)) = (details_body, details_region) {
        app.agents.details_content_height.set(details_lines.len());
        app.agents
            .details_viewport_height
            .set(details_region.content_area.height as usize);
        ScrollableDocument {
            lines: &details_lines,
            offset: app.agents.details_scroll,
        }
        .render(frame, details_region, &theme());
        chrome.render_body_boundary(frame, details_body, &theme());
    }
    if let Some(divider) = divider {
        chrome.render_dividers(frame, &[divider], &theme());
    }
}

fn build_ops_agent_details_lines(
    agent: Option<&AgentListItem>,
    included_count: usize,
    agent_types: &BTreeMap<(String, String), Value>,
    metadata_error: Option<&str>,
    width: u16,
) -> Vec<Line<'static>> {
    let mut lines = vec![
        SectionHeading {
            title: "Selected agent",
            detail: None,
        }
        .line(&theme()),
        Line::default(),
    ];
    if let Some(agent) = agent {
        let value_width = width.saturating_sub(12) as usize;
        let formatted_agent_id =
            format_agent_id_for_terminal(&agent.agent_id, false, Some(value_width.max(20)));
        let mut id_lines = formatted_agent_id.lines();
        if let Some(first) = id_lines.next() {
            let mut spans = vec![
                Span::styled(fit("AgentID", 10), Style::default().fg(theme().text_muted)),
                Span::raw("  "),
            ];
            spans.extend(agent_id_line_spans(first));
            lines.push(Line::from(spans));
        }
        for line in id_lines {
            let mut spans = vec![Span::raw(" ".repeat(12))];
            spans.extend(agent_id_line_spans(line));
            lines.push(Line::from(spans));
        }
        let status = agent.status_label();
        lines.push(Line::from(vec![
            Span::styled(fit("Status", 10), Style::default().fg(theme().text_muted)),
            Span::raw("  "),
            Span::styled(
                status,
                Style::default().fg(agent.status_tone().color(&theme())),
            ),
        ]));
        for (label, value) in [
            ("Type", agent.agent_type.as_deref().unwrap_or("-")),
            ("Component", agent.component.as_deref().unwrap_or("-")),
        ] {
            lines.push(
                FieldRow {
                    label,
                    value,
                    label_width: 10,
                }
                .line(&theme()),
            );
        }
        lines.push(Line::default());
        lines.push(
            Notice {
                kind: NoticeKind::Info,
                message: &format!("{included_count} agents included for cross-agent views"),
            }
            .line(&theme()),
        );
        lines.push(Line::default());
        lines.push(
            SectionHeading {
                title: "Agent metadata",
                detail: None,
            }
            .line(&theme()),
        );
        lines.extend(JsonDocument { value: &agent.raw }.lines(width, &theme()));
        lines.push(Line::default());
        lines.push(
            SectionHeading {
                title: "Agent type metadata",
                detail: None,
            }
            .line(&theme()),
        );
        match agent
            .component
            .as_ref()
            .zip(agent.agent_type.as_ref())
            .and_then(|(component, agent_type)| {
                agent_types.get(&(component.clone(), agent_type.clone()))
            }) {
            Some(metadata) => lines.extend(JsonDocument { value: metadata }.lines(width, &theme())),
            None if metadata_error.is_some() => lines.push(
                Notice {
                    kind: NoticeKind::Warning,
                    message: metadata_error.unwrap_or("Agent type metadata is unavailable"),
                }
                .line(&theme()),
            ),
            None => lines.push(
                Notice {
                    kind: NoticeKind::Unavailable,
                    message: "No deployed metadata found for this agent type",
                }
                .line(&theme()),
            ),
        }
    } else {
        lines.push(
            Notice {
                kind: NoticeKind::Empty,
                message: "Select an agent to inspect it",
            }
            .line(&theme()),
        );
    }
    lines
}

#[cfg(feature = "tui-preview")]
fn fake_otlp_details_visible(area: Rect) -> bool {
    area.width >= 72
}

#[cfg(feature = "tui-preview")]
fn fake_otlp_weights(show_details: bool) -> Vec<u32> {
    if show_details { vec![56, 44] } else { vec![1] }
}

#[cfg(feature = "tui-preview")]
fn render_fake_otlp_explorer_body(
    frame: &mut Frame<'_>,
    area: Rect,
    selected: usize,
    included_agents: usize,
) {
    let selected = selected.min(FAKE_OTLP_SERIES.len().saturating_sub(1));
    let series = &FAKE_OTLP_SERIES[selected];
    let show_details = fake_otlp_details_visible(area);
    let weights = fake_otlp_weights(show_details);
    let layout = PaneLayout::horizontal(area, &weights);
    let rows = FAKE_OTLP_SERIES
        .iter()
        .map(|series| [series.name, series.instrument, series.latest])
        .collect::<Vec<_>>();
    let row_cells = rows.iter().map(|row| row.as_slice()).collect::<Vec<_>>();
    let table_state = PaneTableState::new(&FAKE_OTLP_COLUMNS, selected);
    let table = PaneTable {
        columns: &FAKE_OTLP_COLUMNS,
        rows: &row_cells,
        state: &table_state,
        decoration: TableDecoration::Zebra,
        cell_tones: None,
        rich_cells: None,
        row_markers: None,
        first_row_index: 0,
    };
    let details_width = layout
        .panes
        .get(1)
        .map(|pane| pane.width)
        .unwrap_or(area.width);
    let details_lines = fake_otlp_details_lines(series, included_agents, details_width);
    let content_heights = if show_details {
        vec![table.height().saturating_add(1), details_lines.len()]
    } else {
        vec![table.height().saturating_add(1)]
    };
    let regions = layout.regions(&content_heights);
    let Some(table_region) = regions.first() else {
        return;
    };

    let notice_area = Rect {
        height: table_region.content_area.height.min(1),
        ..table_region.content_area
    };
    frame.render_widget(
        Paragraph::new(
            Notice {
                kind: NoticeKind::Warning,
                message: "FAKE DATA — deterministic OTLP explorer demo",
            }
            .line(&theme()),
        )
        .style(surface_style()),
        notice_area,
    );
    let table_area = Rect {
        y: table_region
            .content_area
            .y
            .saturating_add(notice_area.height),
        height: table_region
            .content_area
            .height
            .saturating_sub(notice_area.height),
        ..table_region.content_area
    };
    let resolved_columns = responsive_table_columns(
        &FAKE_OTLP_COLUMNS,
        &row_cells,
        &table_state,
        table_area.width.saturating_sub(2),
    );
    let window = table.window(table_area.height);
    let start = window.start_row.min(row_cells.len());
    let visible_rows = &row_cells[start..];
    let mut visible_state = table_state.clone();
    visible_state.selected = window.selected_row;
    PaneTable {
        columns: &resolved_columns,
        rows: visible_rows,
        state: &visible_state,
        decoration: TableDecoration::Zebra,
        cell_tones: None,
        rich_cells: None,
        row_markers: None,
        first_row_index: start,
    }
    .render(frame, table_area, &theme());

    layout.render_body_boundaries(frame, &regions, &theme());
    if let Some(slot) = table_region.scrollbar {
        let mut state = ScrollbarState::new(table.height().saturating_add(1))
            .position(window.selected_line_offset.saturating_add(1))
            .viewport_content_length(table_region.content_area.height as usize);
        render_scrollbar(frame, slot, &mut state, &theme());
    }
    if show_details && let Some(details_region) = regions.get(1) {
        ScrollableDocument {
            lines: &details_lines,
            offset: 0,
        }
        .render(frame, *details_region, &theme());
    }
}

#[cfg(feature = "tui-preview")]
fn fake_otlp_details_lines(
    series: &FakeOtlpSeries,
    included_agents: usize,
    width: u16,
) -> Vec<Line<'static>> {
    let mut lines = vec![
        Notice {
            kind: NoticeKind::Warning,
            message: "FAKE DATA — generated for UI review; not received from this context",
        }
        .line(&theme()),
        Line::default(),
        SectionHeading {
            title: "Selected OTLP series",
            detail: Some("demo only"),
        }
        .line(&theme()),
    ];
    for (label, value) in [
        ("Metric", series.name),
        ("Instrument", series.instrument),
        ("Unit", series.unit),
        ("Latest", series.latest),
        ("Query", series.aggregation),
        ("Scope", series.scope),
        ("Resource", series.resource),
    ] {
        lines.push(
            FieldRow {
                label,
                value,
                label_width: 11,
            }
            .line(&theme()),
        );
    }
    lines.push(
        FieldRow {
            label: "Selection",
            value: &format!("{included_agents} explicitly included agents"),
            label_width: 11,
        }
        .line(&theme()),
    );
    lines.extend([
        Line::default(),
        SectionHeading {
            title: "15 minute trend",
            detail: Some("deterministic samples"),
        }
        .line(&theme()),
        Line::from(Span::styled(
            fake_otlp_sparkline(series.samples, width.saturating_sub(2) as usize),
            Style::default().fg(theme().accent),
        )),
        Line::from(Span::styled(
            "-15m                                               now",
            Style::default().fg(theme().text_muted),
        )),
        Line::default(),
        SectionHeading {
            title: "Attributes",
            detail: None,
        }
        .line(&theme()),
        Line::from(Span::styled(
            series.attributes.to_string(),
            Style::default().fg(theme().text_secondary),
        )),
        Line::default(),
        Notice {
            kind: NoticeKind::Info,
            message: "No OTLP receiver or query store is read by this demo",
        }
        .line(&theme()),
    ]);
    lines
}

#[cfg(feature = "tui-preview")]
fn fake_otlp_sparkline(samples: &[u64], width: usize) -> String {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    if samples.is_empty() || width == 0 {
        return String::new();
    }
    let width = width.min(64);
    let min = samples.iter().copied().min().unwrap_or_default();
    let max = samples.iter().copied().max().unwrap_or(min);
    (0..width)
        .map(|index| {
            let sample_index = if width == 1 {
                samples.len() - 1
            } else {
                index.saturating_mul(samples.len() - 1) / (width - 1)
            };
            let value = samples[sample_index];
            let level = if max == min {
                3
            } else {
                value.saturating_sub(min).saturating_mul(7) / max.saturating_sub(min)
            };
            BARS[level as usize]
        })
        .collect()
}

fn render_shell_header(frame: &mut Frame<'_>, area: Rect, app: &TuiApp) {
    ContextHeader {
        pairs: &[
            ContextPair {
                label: "app",
                value: &app.context.application,
            },
            ContextPair {
                label: "env",
                value: &app.context.environment,
            },
            ContextPair {
                label: "server",
                value: &app.context.server,
            },
        ],
    }
    .render(frame, area, &theme());
}

fn render_shell_footer(frame: &mut Frame<'_>, area: Rect, app: &TuiApp) {
    if area.height == 0 {
        return;
    }
    let rows = (area.y..area.bottom())
        .map(|y| Rect::new(area.x, y, area.width, 1))
        .collect::<Vec<_>>();
    let workspace_items = [
        WorkspaceItem {
            key: "",
            label: "Dev",
            active: app.active_workspace == TuiWorkspace::Dev,
        },
        WorkspaceItem {
            key: "",
            label: "Ops",
            active: app.active_workspace == TuiWorkspace::Ops,
        },
    ];
    WorkspaceSelector {
        items: &workspace_items,
        junctions: &[],
    }
    .render(frame, rows[0], &theme());

    let global = [
        KeyHint {
            key: "ctrl+x",
            label: "Menu",
        },
        KeyHint {
            key: "ctrl+p",
            label: "All actions",
        },
        KeyHint {
            key: "ctrl+h",
            label: "Help",
        },
        KeyHint {
            key: "ctrl+q",
            label: "Quit",
        },
    ];
    if let Some(row) = rows.get(1) {
        ShortcutRow {
            items: &global,
            active: false,
            left_glyph: if rows.len() == 2 { "└" } else { "│" },
            fallback: Some(global[0]),
        }
        .render(frame, *row, &theme());
    }
    if let Some(row) = rows.get(2) {
        let dev = [
            KeyHint {
                key: "tab / shift+tab",
                label: "Focus pane",
            },
            KeyHint {
                key: "ctrl+x l",
                label: "Layout",
            },
            KeyHint {
                key: "ctrl+x a",
                label: "Actions",
            },
        ];
        let ops = [
            KeyHint {
                key: "type",
                label: "Find loaded",
            },
            KeyHint {
                key: "enter",
                label: "Open subject",
            },
            KeyHint {
                key: "tab / shift+tab",
                label: "Focus pane",
            },
            KeyHint {
                key: "ctrl+x l",
                label: "Layout",
            },
            KeyHint {
                key: "ctrl+x a",
                label: "Actions",
            },
        ];
        let layout_mode = [
            KeyHint {
                key: "arrows / shift+arrows",
                label: "Resize 1 / 5",
            },
            KeyHint {
                key: "f",
                label: "Flip",
            },
            KeyHint {
                key: "enter / esc",
                label: "Done",
            },
        ];
        let scrollback_mode = [
            KeyHint {
                key: "↑/↓ / pgup/pgdn",
                label: "Scroll",
            },
            KeyHint {
                key: "home / end",
                label: "Oldest / latest",
            },
            KeyHint {
                key: "esc",
                label: "Return to live input",
            },
        ];
        let items = if app.mode == TuiMode::Layout {
            layout_mode.as_slice()
        } else if app.mode == TuiMode::TerminalScrollback {
            scrollback_mode.as_slice()
        } else if app.active_workspace == TuiWorkspace::Dev {
            dev.as_slice()
        } else {
            ops.as_slice()
        };
        ShortcutRow {
            items,
            active: false,
            left_glyph: "└",
            fallback: None,
        }
        .render(frame, *row, &theme());
    }
}

fn render_agent_columns(frame: &mut Frame<'_>, app: &TuiApp) {
    let Some(chooser) = app.agents.column_chooser.as_ref() else {
        return;
    };
    let area = adaptive_data_popup_rect(frame.area(), 40, 10);
    let content = OverlayFrame::content_area(area);
    let choice_capacity = content.height.saturating_sub(3).max(1) as usize;
    let overflow = AGENT_COLUMNS.len() > choice_capacity;
    let table_width = content.width.saturating_sub(u16::from(overflow)) as usize;
    let widths = decision_table_widths(table_width, 65);
    let start = chooser
        .selected
        .saturating_sub(choice_capacity.saturating_sub(1));
    let mut lines = chooser.lines(&AGENT_COLUMNS, &widths, start, choice_capacity, &theme());
    lines.push(Line::default());
    let footer = shortcut_line(
        &[
            KeyHint {
                key: "↑/↓",
                label: "Navigate",
            },
            KeyHint {
                key: "space",
                label: "Toggle",
            },
            KeyHint {
                key: "enter",
                label: "Apply",
            },
            KeyHint {
                key: "esc",
                label: "Cancel",
            },
        ],
        &theme(),
        Alignment::Center,
    );
    let lines = overlay_lines_with_footer(lines, footer, content.height);
    OverlayFrame {
        title: Some("Columns"),
    }
    .render(frame, area, lines, &theme());
    if overflow {
        render_overlay_scrollbar(
            frame,
            Rect {
                y: content.y.saturating_add(1),
                height: content.height.saturating_sub(3),
                ..content
            },
            AGENT_COLUMNS.len(),
            chooser.selected,
        );
    }
}

fn render_agent_dataset_filter(frame: &mut Frame<'_>, app: &TuiApp) {
    let Some(filter) = app.agents.dataset_filter.as_ref() else {
        return;
    };
    let area = adaptive_data_popup_rect(frame.area(), 48, 12);
    let content = OverlayFrame::content_area(area);
    let visible_count = content.height.saturating_sub(6).max(1) as usize;
    let filtered_indices = filter.filtered_indices();
    let overflow = filtered_indices.len() > visible_count;
    let table_width = content.width.saturating_sub(u16::from(overflow)) as usize;
    let widths = decision_table_widths(table_width, 50);
    let start = filter
        .selected
        .saturating_sub(visible_count.saturating_sub(1));
    let query_text = if filter.query.is_empty() {
        "type component or agent type"
    } else {
        &filter.query
    };
    let mut lines = vec![
        Notice {
            kind: NoticeKind::Info,
            message: "Server-side filter; applied before loading agents",
        }
        .line(&theme()),
        Line::from(vec![
            Span::styled("Find     ", Style::default().fg(theme().text_muted)),
            Span::styled(
                query_text.to_string(),
                Style::default().fg(if filter.query.is_empty() {
                    theme().text_faint
                } else {
                    theme().input_text
                }),
            ),
            Span::styled(
                format!(
                    "   {}/{} choices",
                    filtered_indices.len(),
                    filter.choices.len()
                ),
                Style::default().fg(theme().text_muted),
            ),
        ]),
        Line::default(),
        DecisionTableRow {
            cells: &["Component", "Agent type"],
            widths: &widths,
            header: true,
            selectable: true,
            selected: false,
        }
        .line(&theme()),
    ];
    if filtered_indices.is_empty() {
        lines.push(
            Notice {
                kind: NoticeKind::Empty,
                message: "No component or agent type matches",
            }
            .line(&theme()),
        );
    } else {
        lines.extend(
            filtered_indices
                .iter()
                .enumerate()
                .skip(start)
                .take(visible_count)
                .filter_map(|(visible_index, choice_index)| {
                    let choice = filter.choices.get(*choice_index)?;
                    let (component, agent_type) = match choice {
                        AgentDatasetScope::All => ("Any", "Any"),
                        AgentDatasetScope::Component(component) => (component.as_str(), "Any"),
                        AgentDatasetScope::AgentType {
                            component,
                            agent_type,
                        } => (component.as_str(), agent_type.as_str()),
                    };
                    Some(
                        DecisionTableRow {
                            cells: &[component, agent_type],
                            widths: &widths,
                            header: false,
                            selectable: true,
                            selected: visible_index == filter.selected,
                        }
                        .line(&theme()),
                    )
                }),
        );
    }
    lines.push(Line::default());
    let footer = shortcut_line(
        &[
            KeyHint {
                key: "type",
                label: "Find",
            },
            KeyHint {
                key: "ctrl+u",
                label: "Clear",
            },
            KeyHint {
                key: "↑/↓",
                label: "Navigate",
            },
            KeyHint {
                key: "enter",
                label: "Apply",
            },
            KeyHint {
                key: "esc",
                label: "Cancel",
            },
        ],
        &theme(),
        Alignment::Center,
    );
    let lines = overlay_lines_with_footer(lines, footer, content.height);
    OverlayFrame {
        title: Some("Agent dataset"),
    }
    .render(frame, area, lines, &theme());
    if overflow {
        render_overlay_scrollbar(
            frame,
            Rect {
                y: content.y.saturating_add(4),
                height: content.height.saturating_sub(6),
                ..content
            },
            filtered_indices.len(),
            filter.selected,
        );
    }
}

fn decision_table_widths(total_width: usize, first_percent: usize) -> [usize; 2] {
    let available = total_width.saturating_sub(3);
    let first = available.saturating_mul(first_percent) / 100;
    [first, available.saturating_sub(first)]
}

#[cfg(feature = "tui-preview")]
pub(super) fn render_preview_buffer(
    scene: &str,
    variant: TuiVisualVariant,
    width: u16,
    height: u16,
) -> anyhow::Result<ratatui::buffer::Buffer> {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let scene = DesignLabScene::parse(scene)?;
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend)?;
    let visual = TuiVisualStyle::for_variant(variant);
    terminal.draw(|frame| with_style(&visual, || render_design_lab(frame, scene)))?;
    Ok(terminal.backend().buffer().clone())
}

#[cfg(all(feature = "tui-preview", test))]
pub(super) fn render_production_preview_buffer(
    width: u16,
    height: u16,
    styled: bool,
) -> anyhow::Result<ratatui::buffer::Buffer> {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let app = preview_production_app();
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend)?;
    terminal.draw(|frame| {
        if styled {
            render_styled(frame, &app, &TuiVisualStyle::production());
        } else {
            render(frame, &app);
        }
    })?;
    Ok(terminal.backend().buffer().clone())
}

#[cfg(feature = "tui-preview")]
fn render_design_lab(frame: &mut Frame<'_>, scene: DesignLabScene) {
    if matches!(theme().footer_layout, FooterLayoutStyle::Joined) {
        render_design_lab_with_unified_footer(frame, scene);
        render_design_lab_overlay_for_scene(frame, scene);
        return;
    }

    let [header, navigation, separator, body, footer] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .areas(frame.area());

    render_design_lab_header(frame, header);
    render_design_lab_navigation(frame, navigation);
    render_design_lab_separator(frame, separator, scene);
    render_design_lab_body(frame, body, scene);
    render_design_lab_footer(frame, footer, scene);

    render_design_lab_overlay_for_scene(frame, scene);
}

#[cfg(feature = "tui-preview")]
fn render_design_lab_overlay_for_scene(frame: &mut Frame<'_>, scene: DesignLabScene) {
    match scene {
        DesignLabScene::OverlaySearch => render_design_lab_search(frame),
        DesignLabScene::OverlayDecision => render_design_lab_decision(frame),
        DesignLabScene::OverlayError => render_design_lab_error(frame),
        DesignLabScene::OverlayNested => {
            render_design_lab_decision(frame);
            render_design_lab_nested_error(frame);
        }
        _ => {}
    }
}

#[cfg(feature = "tui-preview")]
fn render_design_lab_with_unified_footer(frame: &mut Frame<'_>, scene: DesignLabScene) {
    let compact = design_lab_compact_shell(frame.area());
    let contextual = design_lab_contextual_hints(scene);
    let contextual_rows = if compact {
        0
    } else {
        ShortcutRow::pack(&contextual, frame.area().width, 2).len() as u16
    };
    let footer_height = 2 + contextual_rows + u16::from(scene == DesignLabScene::ShortcutLeader);
    let [header, separator, body, footer] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(footer_height),
        ])
        .areas(frame.area());
    render_design_lab_header(frame, header);
    render_design_lab_separator(frame, separator, scene);
    render_design_lab_body(frame, body, scene);
    render_design_lab_unified_footer(frame, footer, scene, compact);
}

#[cfg(feature = "tui-preview")]
fn design_lab_compact_shell(area: Rect) -> bool {
    area.width < 72 || area.height < 20
}

#[cfg(feature = "tui-preview")]
fn render_design_lab_header(frame: &mut Frame<'_>, area: Rect) {
    if theme().footer_layout == FooterLayoutStyle::Joined {
        ContextHeader {
            pairs: &[
                ContextPair {
                    label: "app",
                    value: "preview-app",
                },
                ContextPair {
                    label: "env",
                    value: "local",
                },
                ContextPair {
                    label: "server",
                    value: "local",
                },
            ],
        }
        .render(frame, area, &theme());
        return;
    }
    let filled = theme().fill_header;
    let row_style = if filled {
        header_style()
    } else if theme().shared_chrome_background {
        footer_style()
    } else {
        tabs_style()
    };
    let label_style = if filled {
        header_segment_label_style(0)
    } else {
        Style::default().fg(theme().text_muted)
    };
    let value_style = if filled {
        header_segment_value_style(0)
    } else {
        Style::default()
            .fg(theme().text)
            .add_modifier(Modifier::BOLD)
    };
    let logo_text = match theme().chrome_text {
        ChromeTextStyle::Filled | ChromeTextStyle::PlainPadded => " GOLEM ",
    };
    let logo_style = match theme().chrome_text {
        ChromeTextStyle::Filled => Style::default()
            .fg(theme().background)
            .bg(theme().accent)
            .add_modifier(Modifier::BOLD),
        ChromeTextStyle::PlainPadded => Style::default()
            .fg(theme().accent)
            .add_modifier(Modifier::BOLD),
    };
    let mut spans = vec![
        Span::styled(
            format!(
                "{}{}",
                if theme().footer_layout == FooterLayoutStyle::Joined {
                    "┌"
                } else {
                    theme().rail_glyph
                },
                theme().header_rail_gap
            ),
            row_style.fg(theme().border_subtle),
        ),
        Span::styled(logo_text, logo_style),
    ];
    if theme().footer_layout == FooterLayoutStyle::Joined {
        spans.push(Span::styled(
            "·",
            Style::default().fg(theme().border_subtle),
        ));
    }
    let metadata_separator = match theme().header_separator {
        HeaderSeparatorStyle::Divider => " │ ",
        HeaderSeparatorStyle::Dot => " · ",
    };
    match theme().header_metadata {
        HeaderMetadataStyle::Labels => spans.extend([
            Span::styled(" app: ", label_style),
            Span::styled("preview-app", value_style),
            Span::styled("  env: ", label_style),
            Span::styled("local", value_style),
            Span::styled("  server: ", label_style),
            Span::styled("local", value_style),
        ]),
        HeaderMetadataStyle::Dividers => {
            push_design_lab_context_pair(
                &mut spans,
                "app",
                "preview-app",
                true,
                label_style,
                value_style,
            );
            spans.push(Span::styled(
                metadata_separator,
                Style::default().fg(theme().border_subtle),
            ));
            push_design_lab_context_pair(
                &mut spans,
                "env",
                "local",
                false,
                label_style,
                value_style,
            );
            spans.push(Span::styled(
                metadata_separator,
                Style::default().fg(theme().border_subtle),
            ));
            push_design_lab_context_pair(
                &mut spans,
                "server",
                "local",
                false,
                label_style,
                value_style,
            );
        }
    }
    frame.render_widget(Paragraph::new(Line::from(spans)).style(row_style), area);
}

#[cfg(feature = "tui-preview")]
fn push_design_lab_context_pair(
    spans: &mut Vec<Span<'static>>,
    label: &'static str,
    value: &'static str,
    first: bool,
    delimiter_style: Style,
    value_style: Style,
) {
    let label = if first {
        format!(" {label}")
    } else {
        label.to_string()
    };
    spans.push(Span::styled(format!("{label} "), delimiter_style));
    spans.push(Span::styled(value, value_style));
}

#[cfg(feature = "tui-preview")]
fn render_design_lab_navigation(frame: &mut Frame<'_>, area: Rect) {
    let line = Line::from(vec![
        Span::styled(
            format!("{}{}", theme().rail_glyph, theme().header_rail_gap),
            tabs_rail_style(),
        ),
        shortcut_text_span("[1]".to_string()),
        Span::styled(
            " Overview",
            Style::default()
                .fg(theme().accent_hover)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        ),
        Span::raw("   "),
        shortcut_text_span("[2]".to_string()),
        Span::styled(" Workbench", Style::default().fg(theme().text_muted)),
        Span::raw("   "),
        shortcut_text_span("[3]".to_string()),
        Span::styled(" Resources", Style::default().fg(theme().text_muted)),
        Span::raw("  "),
        Span::styled("● active", Style::default().fg(theme().success)),
    ]);
    frame.render_widget(Paragraph::new(line).style(tabs_style()), area);
    render_design_lab_rail(frame, area, tabs_rail_style(), false);
}

#[cfg(feature = "tui-preview")]
fn render_design_lab_separator(frame: &mut Frame<'_>, area: Rect, scene: DesignLabScene) {
    if design_lab_joined_titles() {
        let (specs, weights) = design_lab_top_panes(scene);
        PaneLayout::horizontal(area, &weights).render_headers(frame, &specs, &theme());
        return;
    }
    let (glyph, style) = match theme().pane_edges {
        PaneEdgeStyle::Production => (" ", separator_style()),
        PaneEdgeStyle::Shared => ("─", surface_style().fg(theme().border_subtle)),
    };
    frame.render_widget(
        Paragraph::new(glyph.repeat(area.width as usize)).style(style),
        area,
    );
    render_design_lab_rail(frame, area, style.fg(theme().border_subtle), false);
}

#[cfg(feature = "tui-preview")]
fn design_lab_top_panes(scene: DesignLabScene) -> (Vec<PaneSpec<'static>>, Vec<u32>) {
    match scene {
        DesignLabScene::OpsAgents => (
            vec![
                PaneSpec {
                    title: "Agents · Overview",
                    focus: PaneFocus::Active,
                },
                PaneSpec {
                    title: "Details",
                    focus: PaneFocus::Idle,
                },
            ],
            vec![60, 40],
        ),
        DesignLabScene::OpsMetricsDemo => (
            vec![
                PaneSpec {
                    title: "Agents · Metrics · FAKE OTLP",
                    focus: PaneFocus::Active,
                },
                PaneSpec {
                    title: "Series detail · FAKE",
                    focus: PaneFocus::Idle,
                },
            ],
            vec![56, 44],
        ),
        DesignLabScene::ActivityTimeline => (
            vec![PaneSpec {
                title: "Agents · Activity · Timeline",
                focus: PaneFocus::Active,
            }],
            vec![1],
        ),
        DesignLabScene::ActivityJournal => (
            vec![PaneSpec {
                title: "Agents · Activity · Journal",
                focus: PaneFocus::Active,
            }],
            vec![1],
        ),
        DesignLabScene::SplitFocus | DesignLabScene::SplitScrollable => (
            vec![
                PaneSpec {
                    title: "Focused panel",
                    focus: PaneFocus::Active,
                },
                PaneSpec {
                    title: "Secondary",
                    focus: PaneFocus::Idle,
                },
            ],
            vec![60, 40],
        ),
        DesignLabScene::ThreePaneScrollable => (
            vec![
                PaneSpec {
                    title: "Left",
                    focus: PaneFocus::Active,
                },
                PaneSpec {
                    title: "Middle",
                    focus: PaneFocus::Idle,
                },
                PaneSpec {
                    title: "Right",
                    focus: PaneFocus::Idle,
                },
            ],
            vec![1, 1, 1],
        ),
        DesignLabScene::ContentSplit => (
            vec![
                PaneSpec {
                    title: "Resources",
                    focus: PaneFocus::Active,
                },
                PaneSpec {
                    title: "Status",
                    focus: PaneFocus::Idle,
                },
            ],
            vec![60, 40],
        ),
        DesignLabScene::ContentDensity
        | DesignLabScene::ContentLong
        | DesignLabScene::ContentScrolling
        | DesignLabScene::TableDecoration
        | DesignLabScene::TableLong
        | DesignLabScene::TableColumns => (
            vec![PaneSpec {
                title: if matches!(
                    scene,
                    DesignLabScene::TableDecoration
                        | DesignLabScene::TableLong
                        | DesignLabScene::TableColumns
                ) {
                    "Resources"
                } else {
                    "Content"
                },
                focus: PaneFocus::Active,
            }],
            vec![1],
        ),
        DesignLabScene::TableDetails => (
            vec![
                PaneSpec {
                    title: "Resources",
                    focus: PaneFocus::Active,
                },
                PaneSpec {
                    title: "Details",
                    focus: PaneFocus::Idle,
                },
            ],
            vec![65, 35],
        ),
        _ => (
            vec![PaneSpec {
                title: "Overview",
                focus: PaneFocus::Active,
            }],
            vec![1],
        ),
    }
}

#[cfg(feature = "tui-preview")]
fn design_lab_joined_titles() -> bool {
    theme().pane_titles != PaneTitleStyle::Production
}

#[cfg(feature = "tui-preview")]
fn render_design_lab_joined_header(
    frame: &mut Frame<'_>,
    area: Rect,
    connector: Option<&'static str>,
    title: &str,
    focused: bool,
    _hint: &str,
    right_ending: &'static str,
) {
    let ending = match right_ending {
        "┐" => PaneEnding::Top,
        "┤" => PaneEnding::Stacked,
        _ => PaneEnding::Continue,
    };
    PaneHeader {
        title,
        focus: if focused {
            PaneFocus::Active
        } else {
            PaneFocus::Idle
        },
        left_connector: connector,
        ending,
    }
    .render(frame, area, &theme());
}

#[cfg(feature = "tui-preview")]
fn design_lab_joined_rule_style(focused: bool) -> Style {
    let _ = focused;
    surface_style().fg(theme().border_subtle)
}

#[cfg(feature = "tui-preview")]
fn render_design_lab_surface(frame: &mut Frame<'_>, area: Rect) {
    frame.render_widget(Paragraph::new("").style(surface_style()), area);
    render_design_lab_rail(frame, area, surface_rail_style(), false);
}

#[cfg(feature = "tui-preview")]
fn render_design_lab_rail(frame: &mut Frame<'_>, area: Rect, style: Style, focused: bool) {
    let glyph = if focused {
        theme().focus_rail_glyph
    } else {
        theme().rail_glyph
    };
    for y in area.y..area.y.saturating_add(area.height) {
        frame.buffer_mut()[(area.x, y)]
            .set_symbol(glyph)
            .set_style(style);
    }
}

#[cfg(feature = "tui-preview")]
fn render_design_lab_body(frame: &mut Frame<'_>, area: Rect, scene: DesignLabScene) {
    match scene {
        DesignLabScene::OpsAgents => render_design_lab_table_details(frame, area),
        DesignLabScene::OpsMetricsDemo => render_design_lab_metrics_demo(frame, area),
        DesignLabScene::ActivityTimeline => render_design_lab_activity(frame, area, false),
        DesignLabScene::ActivityJournal => render_design_lab_activity(frame, area, true),
        DesignLabScene::ShellDefault
        | DesignLabScene::ShellScrollable
        | DesignLabScene::ShortcutLeader
        | DesignLabScene::OverlaySearch
        | DesignLabScene::OverlayDecision
        | DesignLabScene::OverlayError
        | DesignLabScene::OverlayNested => render_design_lab_shell_content(frame, area),
        DesignLabScene::ContentDensity => {
            render_design_lab_single_content(frame, area, false, false)
        }
        DesignLabScene::ContentLong => render_design_lab_single_content(frame, area, false, true),
        DesignLabScene::ContentScrolling => {
            render_design_lab_surface(frame, area);
            let layout = PaneLayout::horizontal(area, &[1]);
            let regions = layout.regions(&[28]);
            if let Some(region) = regions.first() {
                render_design_lab_content(frame, region.content_area, true, true);
                layout.render_body_boundaries(frame, &regions, &theme());
                if let Some(slot) = region.scrollbar {
                    render_design_lab_scrollbar(frame, slot, 8, 28, area.height as usize);
                }
            }
        }
        DesignLabScene::ContentSplit => render_design_lab_content_split(frame, area),
        DesignLabScene::SplitFocus => render_design_lab_splits(frame, area, false),
        DesignLabScene::SplitScrollable => render_design_lab_splits(frame, area, true),
        DesignLabScene::ThreePaneScrollable => render_design_lab_three_scrollbars(frame, area),
        DesignLabScene::TableDecoration => render_design_lab_table_decoration(frame, area),
        DesignLabScene::TableLong => render_design_lab_table(frame, area, true),
        DesignLabScene::TableDetails => render_design_lab_table_details(frame, area),
        DesignLabScene::TableColumns => {
            render_design_lab_table(frame, area, false);
            render_design_lab_column_chooser(frame);
        }
    }
    if scene == DesignLabScene::ShellScrollable {
        let layout = PaneLayout::horizontal(area, &[1]);
        let regions = layout.regions(&[36]);
        if let Some(slot) = regions.first().and_then(|region| region.scrollbar) {
            render_design_lab_scrollbar(frame, slot, 11, 36, area.height as usize);
        }
    }
}

#[cfg(feature = "tui-preview")]
fn render_design_lab_single_content(
    frame: &mut Frame<'_>,
    area: Rect,
    scrolling: bool,
    long_content: bool,
) {
    render_design_lab_surface(frame, area);
    let layout = PaneLayout::horizontal(area, &[1]);
    let regions = layout.regions(&[0]);
    if let Some(region) = regions.first() {
        render_design_lab_content(frame, region.content_area, scrolling, long_content);
        layout.render_body_boundaries(frame, &regions, &theme());
    }
}

#[cfg(feature = "tui-preview")]
fn render_design_lab_scrollbar(
    frame: &mut Frame<'_>,
    slot: PaneScrollbarSlot,
    position: usize,
    total: usize,
    viewport: usize,
) {
    let mut state = ScrollbarState::new(total)
        .position(position)
        .viewport_content_length(viewport);
    render_scrollbar(frame, slot, &mut state, &theme());
}

#[cfg(feature = "tui-preview")]
fn render_design_lab_three_scrollbars(frame: &mut Frame<'_>, area: Rect) {
    render_design_lab_surface(frame, area);
    let layout = PaneLayout::horizontal(area, &[1, 1, 1]);
    let regions = layout.regions(&[40, 40, 40]);
    for region in &regions {
        if region.content_area.width == 0 || region.content_area.height == 0 {
            continue;
        }
        let label = format!("{:?} pane content reaches its usable edge", region.role);
        frame.render_widget(
            Paragraph::new(label).style(surface_style()),
            region.content_area,
        );
    }
    layout.render_body_boundaries(frame, &regions, &theme());
    for (region, position) in regions.iter().zip([5, 12, 20]) {
        if let Some(slot) = region.scrollbar {
            render_design_lab_scrollbar(frame, slot, position, 40, area.height as usize);
        }
    }
}

#[cfg(feature = "tui-preview")]
const PANE_TABLE_COLUMNS: &[PaneTableColumn<'static>] = &[
    PaneTableColumn {
        id: "name",
        title: "Name",
        width: 20,
        required: true,
        default_visible: true,
        policy: CellPolicy::Ellipsis,
    },
    PaneTableColumn {
        id: "state",
        title: "State",
        width: 10,
        required: true,
        default_visible: true,
        policy: CellPolicy::Ellipsis,
    },
    PaneTableColumn {
        id: "kind",
        title: "Kind",
        width: 14,
        required: false,
        default_visible: true,
        policy: CellPolicy::Ellipsis,
    },
    PaneTableColumn {
        id: "owner",
        title: "Owner",
        width: 18,
        required: false,
        default_visible: false,
        policy: CellPolicy::Ellipsis,
    },
    PaneTableColumn {
        id: "description",
        title: "Description",
        width: 34,
        required: false,
        default_visible: true,
        policy: CellPolicy::WrapSelected,
    },
];

#[cfg(feature = "tui-preview")]
const PANE_TABLE_ROWS: &[&[&str]] = &[
    &[
        "checkout-service",
        "running",
        "agent",
        "payments",
        "Processes checkout requests and coordinates durable payment workflows across regions.",
    ],
    &[
        "order-history",
        "idle",
        "component",
        "fulfilment",
        "Stores a durable customer order timeline with delivery and refund events.",
    ],
    &[
        "payment-reconciliation",
        "failed",
        "worker",
        "finance",
        "Reconciles provider settlements against captured payments and reports mismatches.",
    ],
    &[
        "inventory",
        "running",
        "agent",
        "catalog",
        "Reserves stock while orders move through confirmation and fulfilment.",
    ],
    &[
        "email-notifications",
        "idle",
        "component",
        "engagement",
        "Delivers transactional messages for order state changes.",
    ],
    &[
        "fraud-review",
        "attention",
        "worker",
        "risk",
        "Queues suspicious payments for a manual decision.",
    ],
    &[
        "shipping-quotes",
        "running",
        "agent",
        "fulfilment",
        "Requests carrier estimates and records the selected service.",
    ],
    &[
        "returns",
        "idle",
        "component",
        "support",
        "Coordinates return labels, inspection, and refund eligibility.",
    ],
];

#[cfg(feature = "tui-preview")]
fn render_design_lab_table_decoration(frame: &mut Frame<'_>, area: Rect) {
    render_design_lab_surface(frame, area);
    let layout = PaneLayout::horizontal(area, &[1]);
    let regions = layout.regions(&[0]);
    let Some(region) = regions.first() else {
        return;
    };
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Ratio(1, 3),
            Constraint::Ratio(1, 3),
            Constraint::Ratio(1, 3),
        ])
        .split(region.content_area);
    for (section, title, decoration) in [
        (sections[0], "Minimal", TableDecoration::Minimal),
        (sections[1], "Cell rules", TableDecoration::Rules),
        (sections[2], "Odd / even", TableDecoration::Zebra),
    ] {
        let [heading, table] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .areas(section);
        frame.render_widget(
            Paragraph::new(title).style(
                surface_style()
                    .fg(theme().text_muted)
                    .add_modifier(Modifier::BOLD),
            ),
            heading,
        );
        let state = PaneTableState::new(PANE_TABLE_COLUMNS, 0);
        PaneTable {
            columns: PANE_TABLE_COLUMNS,
            rows: &PANE_TABLE_ROWS[..PANE_TABLE_ROWS.len().min(3)],
            state: &state,
            decoration,
            cell_tones: None,
            rich_cells: None,
            row_markers: None,
            first_row_index: 0,
        }
        .render(frame, table, &theme());
    }
    layout.render_body_boundaries(frame, &regions, &theme());
}

#[cfg(feature = "tui-preview")]
fn render_design_lab_table(frame: &mut Frame<'_>, area: Rect, panned: bool) {
    render_design_lab_surface(frame, area);
    let layout = PaneLayout::horizontal(area, &[1]);
    let mut state = PaneTableState::new(PANE_TABLE_COLUMNS, 0);
    state.set_column_visible(PANE_TABLE_COLUMNS, "owner", true);
    if panned {
        state.horizontal_offset = 18;
    }
    let table = PaneTable {
        columns: PANE_TABLE_COLUMNS,
        rows: PANE_TABLE_ROWS,
        state: &state,
        decoration: TableDecoration::Zebra,
        cell_tones: None,
        rich_cells: None,
        row_markers: None,
        first_row_index: 0,
    };
    let table_height = table.height();
    let regions = layout.regions(&[table_height]);
    let Some(region) = regions.first() else {
        return;
    };
    table.render(frame, region.content_area, &theme());
    layout.render_body_boundaries(frame, &regions, &theme());
    if let Some(slot) = region.scrollbar {
        render_design_lab_scrollbar(frame, slot, 0, table_height, area.height as usize);
    }
}

#[cfg(feature = "tui-preview")]
fn render_design_lab_table_details(frame: &mut Frame<'_>, area: Rect) {
    frame.render_widget(Paragraph::new("").style(surface_style()), area);
    let layout = PaneLayout::horizontal(area, &[65, 35]);
    let regions = layout.regions(&[32, 28]);
    let [table_region, details_region] = regions.as_slice() else {
        return;
    };
    let state = PaneTableState::new(PANE_TABLE_COLUMNS, 1);
    PaneTable {
        columns: PANE_TABLE_COLUMNS,
        rows: PANE_TABLE_ROWS,
        state: &state,
        decoration: TableDecoration::Zebra,
        cell_tones: None,
        rich_cells: None,
        row_markers: None,
        first_row_index: 0,
    }
    .render(frame, table_region.content_area, &theme());
    frame.render_widget(
        Paragraph::new(vec![
            SectionHeading {
                title: "order-history",
                detail: Some("selected"),
            }
            .line(&theme()),
            Line::default(),
            FieldRow {
                label: "State",
                value: "idle",
                label_width: 9,
            }
            .line(&theme()),
            FieldRow {
                label: "Kind",
                value: "component",
                label_width: 9,
            }
            .line(&theme()),
            FieldRow {
                label: "Owner",
                value: "fulfilment",
                label_width: 9,
            }
            .line(&theme()),
            Line::default(),
            Notice {
                kind: NoticeKind::Info,
                message: "Selection remains in the table; Ctrl+Right focuses details.",
            }
            .line(&theme()),
        ])
        .style(surface_style())
        .wrap(Wrap { trim: false }),
        details_region.content_area,
    );
    layout.render_body_boundaries(frame, &regions, &theme());
    for (region, position, total) in [(table_region, 4, 32), (details_region, 7, 28)] {
        if let Some(slot) = region.scrollbar {
            render_design_lab_scrollbar(
                frame,
                slot,
                position,
                total,
                region.pane_area.height as usize,
            );
        }
    }
}

#[cfg(feature = "tui-preview")]
fn render_design_lab_metrics_demo(frame: &mut Frame<'_>, area: Rect) {
    frame.render_widget(Paragraph::new("").style(surface_style()), area);
    render_fake_otlp_explorer_body(frame, area, 1, 4);
}

#[cfg(feature = "tui-preview")]
fn render_design_lab_activity(frame: &mut Frame<'_>, area: Rect, journal: bool) {
    frame.render_widget(Paragraph::new("").style(surface_style()), area);
    let layout = PaneLayout::horizontal(area, &[1]);
    let regions = layout.regions(&[20]);
    let Some(region) = regions.first() else {
        return;
    };
    let lines = if journal {
        vec![
            SectionHeading {
                title: "Journal",
                detail: Some("3 explicitly included agents"),
            }
            .line(&theme()),
            Line::default(),
            FieldRow {
                label: "checkout/a-17",
                value: "#481  exported-function-invoked  invocation=01J…",
                label_width: 18,
            }
            .line(&theme()),
            FieldRow {
                label: "orders/a-04",
                value: "#932  imported-function-invoked  rpc=01J…",
                label_width: 18,
            }
            .line(&theme()),
            Notice {
                kind: NoticeKind::Info,
                message: "Per-agent oplog index is authoritative; timestamps do not establish global causality.",
            }
            .line(&theme()),
        ]
    } else {
        vec![
            SectionHeading {
                title: "Timeline",
                detail: Some("3 explicitly included agents"),
            }
            .line(&theme()),
            Line::default(),
            Notice {
                kind: NoticeKind::Active,
                message: "checkout/a-17 started invocation 01J…",
            }
            .line(&theme()),
            Notice {
                kind: NoticeKind::Info,
                message: "orders/a-04 called checkout/a-17 through durable RPC.",
            }
            .line(&theme()),
            Notice {
                kind: NoticeKind::Success,
                message: "checkout/a-17 completed in 184 ms.",
            }
            .line(&theme()),
        ]
    };
    frame.render_widget(
        Paragraph::new(lines)
            .style(surface_style())
            .wrap(Wrap { trim: false }),
        region.content_area,
    );
    layout.render_body_boundaries(frame, &regions, &theme());
}

#[cfg(feature = "tui-preview")]
fn render_design_lab_column_chooser(frame: &mut Frame<'_>) {
    let table_state = PaneTableState::new(PANE_TABLE_COLUMNS, 0);
    let mut chooser = ColumnChooserState::new(&table_state);
    chooser.selected = 2;
    let mut lines = chooser.lines(
        PANE_TABLE_COLUMNS,
        &[24, 12],
        0,
        PANE_TABLE_COLUMNS.len(),
        &theme(),
    );
    lines.push(Line::default());
    lines.push(shortcut_line(
        &[
            KeyHint {
                key: "↑/↓",
                label: "Navigate",
            },
            KeyHint {
                key: "space",
                label: "Toggle",
            },
            KeyHint {
                key: "enter",
                label: "Apply",
            },
            KeyHint {
                key: "esc",
                label: "Cancel",
            },
        ],
        &theme(),
        Alignment::Center,
    ));
    OverlayFrame {
        title: Some("Columns"),
    }
    .render(
        frame,
        centered_rect_fixed(54, 12, frame.area()),
        lines,
        &theme(),
    );
}

#[cfg(feature = "tui-preview")]
fn design_lab_content_prefix() -> Span<'static> {
    let width = if theme().header_rail_gap.is_empty() {
        1
    } else {
        2
    };
    Span::styled(" ".repeat(width), surface_style())
}

#[cfg(feature = "tui-preview")]
fn design_lab_prefixed_line(text: impl Into<String>) -> Line<'static> {
    Line::from(vec![design_lab_content_prefix(), Span::raw(text.into())])
}

#[cfg(feature = "tui-preview")]
fn render_design_lab_shell_content(frame: &mut Frame<'_>, area: Rect) {
    render_design_lab_surface(frame, area);
    let lines = vec![
        Line::from(vec![
            design_lab_content_prefix(),
            Span::styled(
                "Shell foundation",
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::default(),
        design_lab_prefixed_line(
            "Review identity, context, location, state, and persistent actions.",
        ),
        design_lab_prefixed_line(
            "Product workflows and workspace names remain intentionally provisional.",
        ),
        design_lab_prefixed_line(
            "Long content can use every column through the terminal-facing edge: 0123456789 abcdefghijklmnopqrstuvwxyz ABCDEFGHIJKLMNOPQRSTUVWXYZ",
        ),
        Line::default(),
        Line::from(vec![
            design_lab_content_prefix(),
            Span::styled("●", Style::default().fg(theme().success)),
            Span::raw(" running   "),
            Span::styled("○", Style::default().fg(theme().text_muted)),
            Span::raw(" idle   "),
            Span::styled("×", Style::default().fg(theme().error)),
            Span::raw(" failed   "),
            Span::styled("!", Style::default().fg(theme().marker)),
            Span::raw(" attention"),
        ]),
    ];
    frame.render_widget(Paragraph::new(lines).style(surface_style()), area);
    render_design_lab_rail(frame, area, surface_rail_style(), false);
}

#[cfg(feature = "tui-preview")]
fn render_design_lab_content(
    frame: &mut Frame<'_>,
    area: Rect,
    scrolling: bool,
    long_content: bool,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let content = area;
    let available = content.width as usize;
    let name_width = available.saturating_sub(23).clamp(8, 24);
    let widths = [name_width, 9, 11];
    let mut rows = vec![
        design_lab_content_line(
            SectionHeading {
                title: "Resources",
                detail: Some(if scrolling {
                    "3 of 28 items"
                } else {
                    "3 items"
                }),
            }
            .line(&theme()),
        ),
        design_lab_content_line(
            ContentTableRow {
                cells: &["Name", "State", "Kind"],
                widths: &widths,
                header: true,
                selected: false,
            }
            .line(&theme()),
        ),
        design_lab_content_line(
            ContentTableRow {
                cells: &["checkout", "running", "agent"],
                widths: &widths,
                header: false,
                selected: true,
            }
            .line(&theme()),
        ),
        design_lab_content_line(
            ContentTableRow {
                cells: &["orders", "idle", "agent"],
                widths: &widths,
                header: false,
                selected: false,
            }
            .line(&theme()),
        ),
        design_lab_content_line(
            ContentTableRow {
                cells: &["payments", "failed", "component"],
                widths: &widths,
                header: false,
                selected: false,
            }
            .line(&theme()),
        ),
        Line::default(),
        design_lab_content_line(
            FieldRow {
                label: "Context",
                value: "preview-app / local",
                label_width: 10,
            }
            .line(&theme()),
        ),
        design_lab_content_line(
            FieldRow {
                label: "Revision",
                value: "18",
                label_width: 10,
            }
            .line(&theme()),
        ),
        Line::default(),
        design_lab_content_line(
            Notice {
                kind: NoticeKind::Active,
                message: "Build #18 is producing output.",
            }
            .line(&theme()),
        ),
        design_lab_content_line(
            Notice {
                kind: NoticeKind::Success,
                message: "The latest component revision is deployed.",
            }
            .line(&theme()),
        ),
        design_lab_content_line(
            Notice {
                kind: NoticeKind::Loading,
                message: "Refreshing resources…",
            }
            .line(&theme()),
        ),
        design_lab_content_line(
            Notice {
                kind: NoticeKind::Warning,
                message: "Cached metadata may be stale.",
            }
            .line(&theme()),
        ),
        design_lab_content_line(
            Notice {
                kind: NoticeKind::Unavailable,
                message: "Deploy — select a context first",
            }
            .line(&theme()),
        ),
        design_lab_content_line(
            Notice {
                kind: NoticeKind::Error,
                message: "Connection refused; existing data remains visible.",
            }
            .line(&theme()),
        ),
        design_lab_content_line(
            Notice {
                kind: NoticeKind::Empty,
                message: "No completed jobs in this context.",
            }
            .line(&theme()),
        ),
        design_lab_content_line(
            Notice {
                kind: NoticeKind::Info,
                message: "Selection remains usable while metadata refreshes.",
            }
            .line(&theme()),
        ),
        Line::default(),
        design_lab_content_line(
            SectionHeading {
                title: "Output",
                detail: Some("build #18"),
            }
            .line(&theme()),
        ),
        design_lab_content_line(
            OutputLine {
                stream: "out",
                text: "Compiling component checkout",
            }
            .line(available, &theme()),
        ),
        design_lab_content_line(
            OutputLine {
                stream: "err",
                text: "warning: cached artifact is stale",
            }
            .line(available, &theme()),
        ),
    ];
    if long_content {
        rows.insert(1, design_lab_content_line(FieldRow {
            label: "Location",
            value: "projects/acme/environments/development-west/components/checkout-service-with-a-very-long-name",
            label_width: 10,
        }.line(&theme())));
    }
    frame.render_widget(Paragraph::new(rows).style(surface_style()), content);
}

#[cfg(feature = "tui-preview")]
fn design_lab_content_line(line: Line<'static>) -> Line<'static> {
    line
}

#[cfg(feature = "tui-preview")]
fn render_design_lab_content_split(frame: &mut Frame<'_>, area: Rect) {
    let layout = PaneLayout::horizontal(area, &[60, 40]);
    let regions = layout.regions(&[0, 0]);
    let [primary, secondary] = regions.as_slice() else {
        return;
    };
    frame.render_widget(Paragraph::new("").style(surface_style()), area);
    render_design_lab_content(frame, primary.content_area, false, false);
    layout.render_body_boundaries(frame, &regions, &theme());
    let width = secondary.content_area.width as usize;
    let mut status = Vec::new();
    for marker in [
        StatusMarker {
            kind: StatusKind::Running,
            label: "running",
        },
        StatusMarker {
            kind: StatusKind::Idle,
            label: "idle",
        },
        StatusMarker {
            kind: StatusKind::Failed,
            label: "failed",
        },
        StatusMarker {
            kind: StatusKind::Attention,
            label: "attention",
        },
    ] {
        status.extend(marker.spans(&theme()));
        status.push(Span::raw("  "));
    }
    frame.render_widget(
        Paragraph::new(vec![
            SectionHeading {
                title: "Status",
                detail: None,
            }
            .line(&theme()),
            Line::from(status),
            Line::default(),
            Notice {
                kind: NoticeKind::Unavailable,
                message: "Details hidden in this narrow pane.",
            }
            .line(&theme()),
            Line::default(),
            OutputLine {
                stream: "out",
                text: "Primary content keeps selection and output visible.",
            }
            .line(width, &theme()),
        ])
        .style(surface_style())
        .wrap(Wrap { trim: false }),
        secondary.content_area,
    );
}

#[cfg(feature = "tui-preview")]
fn render_design_lab_splits(frame: &mut Frame<'_>, area: Rect, scrolling: bool) {
    render_design_lab_surface(frame, area);
    let layout = PaneLayout::horizontal(area, &[60, 40]);
    let regions = layout.regions(&[if scrolling { 48 } else { 0 }, 0]);
    let [primary, secondary] = regions.as_slice() else {
        return;
    };
    let [upper, horizontal_handle, lower] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(55),
            Constraint::Length(1),
            Constraint::Min(3),
        ])
        .areas(secondary.pane_area);
    let upper = PaneRegion::new(
        1,
        PaneRole::Right,
        upper,
        if scrolling { 24 } else { 0 },
        PaneResizeDividers {
            before: secondary.resize_dividers.before,
            after: Some(horizontal_handle),
        },
    );
    let lower = PaneRegion::new(
        2,
        PaneRole::Right,
        lower,
        if scrolling { 32 } else { 0 },
        PaneResizeDividers {
            before: Some(horizontal_handle),
            after: None,
        },
    );

    render_design_lab_panel(
        frame,
        primary.pane_area,
        primary.content_area,
        "Focused panel",
        true,
        "tab next",
        &[
            "Focus uses shape and contrast, not color alone.",
            "The body keeps a quiet surface behind dense content.",
            "Long primary-pane content reaches its edge without reserving a scrollbar column: 0123456789",
        ],
        true,
        scrolling,
    );
    layout.render_body_boundaries(frame, &regions, &theme());
    render_design_lab_panel(
        frame,
        upper.pane_area,
        upper.content_area,
        "Secondary",
        false,
        "",
        &[
            "Unfocused panel",
            "Stable identity",
            "Secondary content reaches the right edge 0123456789",
        ],
        false,
        scrolling,
    );
    if design_lab_joined_titles() {
        render_design_lab_joined_lower_header(frame, horizontal_handle);
    } else {
        render_design_lab_split_handle(frame, horizontal_handle);
    }
    render_design_lab_panel(
        frame,
        lower.pane_area,
        lower.content_area,
        "Activity",
        false,
        "",
        &[
            "● running   00:12",
            "Activity content reaches the right edge 0123456789",
        ],
        false,
        scrolling,
    );
    if scrolling {
        for (region, position, total) in [(primary, 14, 48), (&upper, 7, 24), (&lower, 18, 32)] {
            if let Some(slot) = region.scrollbar {
                render_design_lab_scrollbar(
                    frame,
                    slot,
                    position,
                    total,
                    region.pane_area.height as usize,
                );
            }
        }
    }
}

#[cfg(feature = "tui-preview")]
fn render_design_lab_panel(
    frame: &mut Frame<'_>,
    area: Rect,
    content_area: Rect,
    title: &str,
    focused: bool,
    hint: &str,
    lines: &[&str],
    outer_boundary: bool,
    truncate_lines: bool,
) {
    let has_rail = design_lab_panel_has_rail(focused, outer_boundary);
    frame.render_widget(Paragraph::new("").style(surface_style()), area);
    if has_rail {
        render_design_lab_rail(frame, area, surface_rail_style(), false);
    }
    let body = if design_lab_joined_titles() {
        content_area
    } else {
        let [title_area, body] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(1)])
            .areas(area);
        render_design_lab_panel_title(frame, title_area, title, focused, hint, has_rail);
        Rect {
            x: content_area.x,
            width: content_area.width,
            ..body
        }
    };
    frame.render_widget(
        Paragraph::new(
            lines
                .iter()
                .map(|line| {
                    let text = if truncate_lines {
                        fit(line, body.width as usize)
                    } else {
                        (*line).to_string()
                    };
                    design_lab_panel_line(text, false)
                })
                .collect::<Vec<_>>(),
        )
        .style(surface_style()),
        body,
    );
}

#[cfg(feature = "tui-preview")]
fn render_design_lab_joined_lower_header(frame: &mut Frame<'_>, area: Rect) {
    if area.x > 0 {
        frame.buffer_mut()[(area.x - 1, area.y)]
            .set_symbol("├")
            .set_style(design_lab_joined_rule_style(false));
    }
    render_design_lab_joined_header(frame, area, None, "Activity", false, "", "┤");
}

#[cfg(feature = "tui-preview")]
fn design_lab_panel_has_rail(_focused: bool, outer_boundary: bool) -> bool {
    match theme().pane_edges {
        PaneEdgeStyle::Shared => outer_boundary,
        PaneEdgeStyle::Production => true,
    }
}

#[cfg(feature = "tui-preview")]
fn design_lab_panel_line(text: impl Into<String>, has_rail: bool) -> Line<'static> {
    let prefix = if has_rail {
        design_lab_content_prefix()
    } else {
        Span::raw("")
    };
    Line::from(vec![prefix, Span::raw(text.into())])
}

#[cfg(feature = "tui-preview")]
fn render_design_lab_split_handle(frame: &mut Frame<'_>, area: Rect) {
    let glyph = match theme().pane_edges {
        PaneEdgeStyle::Shared => {
            if area.width == 1 {
                "│"
            } else {
                "─"
            }
        }
        PaneEdgeStyle::Production => " ",
    };
    frame.render_widget(
        Paragraph::new(if area.width == 1 {
            glyph.to_string()
        } else {
            glyph.repeat(area.width as usize)
        })
        .style(surface_style().fg(theme().border_subtle)),
        area,
    );
    if area.width == 1 {
        for y in area.y..area.y.saturating_add(area.height) {
            frame.buffer_mut()[(area.x, y)]
                .set_symbol(glyph)
                .set_style(surface_style().fg(theme().border_subtle));
        }
    }
}

#[cfg(feature = "tui-preview")]
fn render_design_lab_panel_title(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    focused: bool,
    hint: &str,
    has_rail: bool,
) {
    let background = match theme().pane_titles {
        PaneTitleStyle::Production if focused => command_status_bg_style(),
        PaneTitleStyle::Production => tabs_style(),
        PaneTitleStyle::HintNone => surface_style(),
    };
    let rail = if focused {
        command_rail_style()
    } else {
        tabs_rail_style()
    };
    let mut spans = Vec::new();
    if has_rail {
        spans.push(Span::styled(
            format!(
                "{}{}",
                if focused {
                    theme().focus_rail_glyph
                } else {
                    theme().rail_glyph
                },
                theme().header_rail_gap
            ),
            rail,
        ));
    }
    let marker_style = if focused {
        Style::default().fg(theme().accent_hover)
    } else {
        Style::default().fg(theme().text_muted)
    };
    let title_style = if focused {
        Style::default()
            .fg(theme().accent_hover)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme().text_muted)
    };
    spans.extend([
        Span::styled(
            if focused {
                theme().focus_marker
            } else {
                theme().idle_marker
            },
            marker_style,
        ),
        Span::raw(" "),
        Span::styled(title.to_string(), title_style),
    ]);
    if !hint.is_empty() {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            hint.to_string(),
            Style::default().fg(theme().text_muted),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)).style(background), area);
    if has_rail {
        render_design_lab_rail(frame, area, rail, focused);
    }
}

#[cfg(feature = "tui-preview")]
fn render_design_lab_footer(frame: &mut Frame<'_>, area: Rect, scene: DesignLabScene) {
    let line = if matches!(scene, DesignLabScene::ShortcutLeader) {
        Line::from(vec![
            Span::styled(
                format!("{} ", theme().focus_rail_glyph),
                command_rail_style(),
            ),
            shortcut_span("w"),
            Span::raw(" workspace  "),
            shortcut_span("f"),
            Span::raw(" focus  "),
            shortcut_span("r"),
            Span::raw(" run  "),
            shortcut_span("l"),
            Span::raw(" layout  "),
            shortcut_span("e"),
            Span::raw(" context  "),
            shortcut_span("?"),
            Span::raw(" help"),
        ])
    } else {
        Line::from(vec![
            Span::styled(format!("{} ", theme().rail_glyph), footer_rail_style()),
            shortcut_span("ctrl+p"),
            Span::raw(" Commands   "),
            shortcut_span("ctrl+x"),
            Span::raw(" More   "),
            shortcut_span("?"),
            Span::raw(" Help   "),
            shortcut_span("q"),
            Span::raw(" Quit"),
        ])
    };
    let style = if matches!(scene, DesignLabScene::ShortcutLeader) {
        command_status_bg_style()
    } else {
        footer_style()
    };
    frame.render_widget(Paragraph::new(line).style(style), area);
    render_design_lab_rail(
        frame,
        area,
        if matches!(scene, DesignLabScene::ShortcutLeader) {
            command_rail_style()
        } else {
            footer_rail_style()
        },
        matches!(scene, DesignLabScene::ShortcutLeader),
    );
}

#[cfg(feature = "tui-preview")]
fn render_design_lab_unified_footer(
    frame: &mut Frame<'_>,
    area: Rect,
    scene: DesignLabScene,
    compact: bool,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let mut rows = (area.y..area.y.saturating_add(area.height))
        .map(|y| Rect::new(area.x, y, area.width, 1))
        .collect::<Vec<_>>();
    let navigation = rows.remove(0);
    render_design_lab_footer_navigation(frame, navigation, true, scene);

    let leader = if scene == DesignLabScene::ShortcutLeader {
        rows.pop()
    } else {
        None
    };
    let global = [
        KeyHint {
            key: "ctrl+p",
            label: "Commands",
        },
        KeyHint {
            key: "ctrl+h",
            label: "Help",
        },
        KeyHint {
            key: "ctrl+q",
            label: "Quit",
        },
    ];
    if !rows.is_empty() {
        let row = rows.remove(0);
        let closes = compact && theme().footer_closes && leader.is_none();
        render_design_lab_scoped_footer_row(
            frame,
            row,
            &global,
            false,
            if closes { "└" } else { "│" },
        );
    }
    let contextual = if compact {
        Vec::new()
    } else {
        design_lab_contextual_hints(scene)
    };
    let contextual_rows = ShortcutRow::pack(&contextual, area.width, 2);
    let row_count = contextual_rows.len().min(rows.len());
    for (index, (row, items)) in rows.into_iter().zip(contextual_rows).enumerate() {
        let closes = theme().footer_closes && leader.is_none() && index + 1 == row_count;
        render_design_lab_scoped_footer_row(
            frame,
            row,
            &items,
            false,
            if closes { "└" } else { "│" },
        );
    }
    if let Some(row) = leader {
        render_design_lab_scoped_footer_row(
            frame,
            row,
            &[
                KeyHint {
                    key: "ctrl+x w",
                    label: "Workspace",
                },
                KeyHint {
                    key: "tab",
                    label: "Focus",
                },
                KeyHint {
                    key: "ctrl+x r",
                    label: "Run",
                },
                KeyHint {
                    key: "ctrl+x l",
                    label: "Layout",
                },
                KeyHint {
                    key: "ctrl+x e",
                    label: "Context",
                },
                KeyHint {
                    key: "ctrl+x h",
                    label: "Help",
                },
            ],
            true,
            if theme().footer_closes { "└" } else { "│" },
        );
    }
}

#[cfg(feature = "tui-preview")]
fn design_lab_contextual_hints(scene: DesignLabScene) -> Vec<KeyHint<'static>> {
    if matches!(
        scene,
        DesignLabScene::OpsAgents
            | DesignLabScene::OpsMetricsDemo
            | DesignLabScene::ActivityTimeline
            | DesignLabScene::ActivityJournal
    ) {
        return vec![
            KeyHint {
                key: "ctrl+x v",
                label: "View",
            },
            KeyHint {
                key: "tab",
                label: "Focus",
            },
            KeyHint {
                key: "type",
                label: "Filter",
            },
            KeyHint {
                key: "ctrl+space",
                label: "Include",
            },
            KeyHint {
                key: "ctrl+r",
                label: "Refresh",
            },
        ];
    }
    if matches!(
        scene,
        DesignLabScene::TableDecoration
            | DesignLabScene::TableLong
            | DesignLabScene::TableDetails
            | DesignLabScene::TableColumns
    ) {
        return vec![
            KeyHint {
                key: "←/→",
                label: "Pan",
            },
            KeyHint {
                key: "ctrl+x c",
                label: "Columns",
            },
            KeyHint {
                key: "enter",
                label: "Details",
            },
            KeyHint {
                key: "tab",
                label: "Focus",
            },
        ];
    }
    let mut hints = vec![
        KeyHint {
            key: "ctrl+r",
            label: "Run",
        },
        KeyHint {
            key: "ctrl+x l",
            label: "Layout",
        },
        KeyHint {
            key: "ctrl+x e",
            label: "Context",
        },
    ];
    if matches!(
        scene,
        DesignLabScene::SplitFocus | DesignLabScene::SplitScrollable | DesignLabScene::ContentSplit
    ) {
        hints.push(KeyHint {
            key: "tab",
            label: "Focus",
        });
    }
    hints.extend([
        KeyHint {
            key: "enter",
            label: "Open",
        },
        KeyHint {
            key: "ctrl+r",
            label: "Refresh",
        },
    ]);
    hints
}

#[cfg(feature = "tui-preview")]
fn render_design_lab_footer_navigation(
    frame: &mut Frame<'_>,
    area: Rect,
    joined: bool,
    scene: DesignLabScene,
) {
    if joined {
        let (_, weights) = design_lab_top_panes(scene);
        let junctions = PaneLayout::horizontal(area, &weights).junctions();
        if matches!(
            scene,
            DesignLabScene::OpsAgents
                | DesignLabScene::OpsMetricsDemo
                | DesignLabScene::ActivityTimeline
                | DesignLabScene::ActivityJournal
        ) {
            WorkspaceSelector {
                items: &[WorkspaceItem {
                    key: "",
                    label: "Ops",
                    active: true,
                }],
                junctions: &junctions,
            }
            .render(frame, area, &theme());
            return;
        }
        WorkspaceSelector {
            items: &[
                WorkspaceItem {
                    key: "",
                    label: "Home",
                    active: true,
                },
                WorkspaceItem {
                    key: "",
                    label: "Dev",
                    active: false,
                },
                WorkspaceItem {
                    key: "",
                    label: "Ops",
                    active: false,
                },
            ],
            junctions: &junctions,
        }
        .render(frame, area, &theme());
        return;
    }
    let line = if joined {
        Line::from(vec![
            Span::styled("├─", design_lab_footer_rule_style()),
            Span::styled(
                "[ ",
                Style::default()
                    .fg(theme().text_secondary)
                    .add_modifier(Modifier::BOLD),
            ),
            design_lab_footer_shortcut_span("1"),
            Span::styled(
                " Overview ]",
                Style::default()
                    .fg(theme().text_secondary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("──", design_lab_footer_rule_style()),
            Span::styled("( ", Style::default().fg(theme().text_muted)),
            design_lab_footer_shortcut_span("2"),
            Span::styled(" Workbench )", Style::default().fg(theme().text_muted)),
            Span::styled("──", design_lab_footer_rule_style()),
            Span::styled("( ", Style::default().fg(theme().text_muted)),
            design_lab_footer_shortcut_span("3"),
            Span::styled(" Resources )", Style::default().fg(theme().text_muted)),
            Span::styled(
                "─".repeat(area.width as usize),
                design_lab_footer_rule_style(),
            ),
        ])
    } else {
        Line::from(vec![
            Span::styled(format!("{} ", theme().rail_glyph), footer_rail_style()),
            design_lab_footer_shortcut_span("1"),
            Span::styled(
                " Overview",
                Style::default()
                    .fg(theme().text_secondary)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            ),
            Span::raw("   "),
            design_lab_footer_shortcut_span("2"),
            Span::styled(" Workbench", Style::default().fg(theme().text_muted)),
            Span::raw("   "),
            design_lab_footer_shortcut_span("3"),
            Span::styled(" Resources", Style::default().fg(theme().text_muted)),
        ])
    };
    frame.render_widget(Paragraph::new(line).style(footer_style()), area);
    if joined {
        frame.buffer_mut()[(area.right().saturating_sub(1), area.y)]
            .set_symbol("┘")
            .set_style(design_lab_footer_rule_style());
    }
}

#[cfg(feature = "tui-preview")]
fn render_design_lab_scoped_footer_row(
    frame: &mut Frame<'_>,
    area: Rect,
    items: &[KeyHint<'_>],
    active: bool,
    left_glyph: &'static str,
) {
    ShortcutRow {
        items,
        active,
        left_glyph,
        fallback: items.iter().find(|item| item.key == "ctrl+p").copied(),
    }
    .render(frame, area, &theme());
}

#[cfg(feature = "tui-preview")]
fn design_lab_footer_rule_style() -> Style {
    Style::default()
        .fg(theme().footer_rule)
        .bg(theme().footer_background)
}

#[cfg(feature = "tui-preview")]
fn design_lab_footer_shortcut_span(key: &'static str) -> Span<'static> {
    key_span(key, &theme())
}

#[cfg(feature = "tui-preview")]
fn render_design_lab_search(frame: &mut Frame<'_>) {
    let area = centered_rect_fixed(68, 12, frame.area());
    let lines = vec![
        design_lab_search_input("dep", area.width.saturating_sub(4) as usize),
        palette_line(vec![]),
        design_lab_search_result("Deploy application", true, false),
        design_lab_search_result("Open deployment details", false, false),
        design_lab_search_result(
            "Delete deployment — unavailable: no remote context",
            false,
            true,
        ),
        palette_line(vec![]),
        design_lab_overlay_shortcuts(&[("enter", "Select"), ("up/down", "Move"), ("esc", "Close")]),
    ];
    render_design_lab_overlay(frame, area, Some("Commands"), lines);
}

#[cfg(feature = "tui-preview")]
fn design_lab_search_input(query: &'static str, width: usize) -> Line<'static> {
    SearchInput { query }.line(width, &theme())
}

#[cfg(feature = "tui-preview")]
fn design_lab_search_result(
    label: &'static str,
    selected: bool,
    unavailable: bool,
) -> Line<'static> {
    SelectableRow {
        label,
        selected,
        unavailable,
    }
    .line(58, &theme())
}

#[cfg(feature = "tui-preview")]
fn render_design_lab_decision(frame: &mut Frame<'_>) {
    let area = centered_rect_fixed(68, 11, frame.area());
    let lines = vec![
        palette_line(vec![Span::styled(
            "! Switching context will stop 2 running jobs.",
            Style::default().fg(theme().marker),
        )]),
        palette_line(vec![]),
        design_lab_decision_table_row("Job", "State", true),
        design_lab_decision_table_row("build #18", "running", false),
        design_lab_decision_table_row("agent stream", "connected", false),
        palette_line(vec![]),
        design_lab_overlay_shortcuts(&[("enter/y", "Confirm"), ("esc/n", "Cancel")]),
    ];
    render_design_lab_overlay(frame, area, Some("Stop running work?"), lines);
}

#[cfg(feature = "tui-preview")]
fn design_lab_decision_table_row(
    job: &'static str,
    state: &'static str,
    header: bool,
) -> Line<'static> {
    DecisionTableRow {
        cells: &[job, state],
        widths: &[24, 14],
        header,
        selectable: false,
        selected: false,
    }
    .line(&theme())
}

#[cfg(feature = "tui-preview")]
fn render_design_lab_error(frame: &mut Frame<'_>) {
    let area = centered_rect_fixed(64, 9, frame.area());
    let lines = vec![
        palette_line(vec![Span::styled(
            "× Could not connect to the selected environment.",
            Style::default().fg(theme().error),
        )]),
        palette_line(vec![Span::raw("Existing deployment state was preserved.")]),
        palette_line(vec![]),
        design_lab_overlay_shortcuts(&[("enter", "Details"), ("esc", "Close")]),
    ];
    render_design_lab_overlay(frame, area, Some("Deployment failed"), lines);
}

#[cfg(feature = "tui-preview")]
fn render_design_lab_nested_error(frame: &mut Frame<'_>) {
    let area = centered_rect_fixed(52, 7, frame.area());
    let lines = vec![
        palette_line(vec![Span::styled(
            "× The active job changed while confirming.",
            Style::default().fg(theme().error),
        )]),
        palette_line(vec![]),
        design_lab_overlay_shortcuts(&[("esc", "Return to confirmation")]),
    ];
    render_design_lab_overlay(frame, area, Some("Action unavailable"), lines);
}

#[cfg(feature = "tui-preview")]
fn render_design_lab_overlay(
    frame: &mut Frame<'_>,
    area: Rect,
    title: Option<&'static str>,
    mut lines: Vec<Line<'static>>,
) {
    if theme().frame_overlays {
        OverlayFrame { title }.render(frame, area, lines, &theme());
        return;
    }
    frame.render_widget(Clear, area);
    if let Some(title) = title {
        lines.insert(
            0,
            palette_line(vec![Span::styled(
                title,
                Style::default()
                    .fg(theme().accent)
                    .add_modifier(Modifier::BOLD),
            )]),
        );
    }
    frame.render_widget(
        Paragraph::new(lines)
            .style(command_status_bg_style())
            .wrap(Wrap { trim: false }),
        Rect {
            x: area.x.saturating_add(2),
            width: area.width.saturating_sub(2),
            ..area
        },
    );
    render_design_lab_rail(
        frame,
        area,
        Style::default().fg(theme().border_subtle),
        true,
    );
}

#[cfg(feature = "tui-preview")]
fn design_lab_overlay_shortcuts(items: &[(&'static str, &'static str)]) -> Line<'static> {
    let hints = items
        .iter()
        .map(|(key, label)| KeyHint { key, label })
        .collect::<Vec<_>>();
    shortcut_line(&hints, &theme(), Alignment::Center)
}

fn render_dev_workspace(frame: &mut Frame<'_>, area: Rect, app: &TuiApp) {
    frame.render_widget(Paragraph::new("").style(surface_style()), area);
    let chrome = PaneChrome { area };
    let input = LayoutInput {
        area,
        active_workspace: app.active_workspace,
        dev_primary: app.dev_primary,
        focused_dev_view: app.dev_focus,
        mode: app.mode,
        context_picker_rows: context_picker_layout_rows(&app.context_switcher),
        context_picker_selected: app.context_switcher.selected,
        context_picker_prefix_height: context_picker_prefix_height(&app.context_switcher),
        agent_details_visible: app.agents.detail_visible,
        layout: app.layout.clone(),
    };

    for (panel, panel_area) in layout::dev_view_areas(area, &input) {
        if panel_area.width == 0 || panel_area.height == 0 {
            continue;
        }
        let body = chrome.body_area(panel_area);
        let content = chrome.content_area(body);
        match panel {
            DevView::Repl => render_repl_view(frame, content, app),
            DevView::BuildDeploy => render_output_view(frame, content, app),
            DevView::Server => render_server_view(frame, content, app),
        }
        chrome.render_header(
            frame,
            panel_area,
            panel.title(),
            if panel == app.dev_focus {
                PaneFocus::Active
            } else {
                PaneFocus::Idle
            },
            &theme(),
        );
        chrome.render_body_boundary(frame, body, &theme());
    }
    if let Some((primary, secondary)) = layout::dev_split_regions(area, &input) {
        let mut dividers = vec![primary];
        if let Some(secondary) = secondary {
            dividers.push(secondary);
        }
        chrome.render_dividers(frame, &dividers, &theme());
    }
}

fn render_repl_view(frame: &mut Frame<'_>, area: Rect, app: &TuiApp) {
    frame.render_widget(Paragraph::new("").style(surface_style()), area);
    let [summary_area, terminal_area] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .areas(area);

    frame.render_widget(
        Paragraph::new(repl_status_line(&app.repl, app.mode)).style(surface_style()),
        summary_area,
    );

    let terminal_content = terminal_content_area(terminal_area, &app.repl.run.screen);
    let lines = if app.repl.run.status == ReplStatus::Stopped && app.repl.run.exit_code.is_none() {
        vec![prefixed_line(
            "Open Actions with ctrl+x a to start `golem repl`.",
        )]
    } else {
        repl_screen_lines(&app.repl.run, terminal_content.height as usize)
    };

    frame.render_widget(
        Paragraph::new(lines).style(surface_style()),
        terminal_content,
    );
    render_output_scrollbar(frame, terminal_area, &app.repl.run.screen);

    if app.mode == TuiMode::Repl
        && let Some(cursor) = app
            .repl
            .run
            .screen
            .cursor_position(Position::new(terminal_content.x, terminal_content.y))
        && cursor.x < terminal_content.right()
        && cursor.y < terminal_content.bottom()
    {
        frame.set_cursor_position(cursor);
    }
}

fn render_server_view(frame: &mut Frame<'_>, area: Rect, app: &TuiApp) {
    frame.render_widget(Paragraph::new("").style(surface_style()), area);
    let [summary_area, output_area] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .areas(area);

    frame.render_widget(
        Paragraph::new(server_status_line(&app.server, app.server.available))
            .style(surface_style()),
        summary_area,
    );

    let output_lines = if app.server.run.terminal.is_empty() {
        if app.server.available {
            vec![prefixed_line(
                "Server logs will appear here. Use ctrl+x a to start.",
            )]
        } else {
            vec![prefixed_line("Local server is unavailable.")]
        }
    } else {
        render_output_lines(&app.server.run.terminal, output_area.height as usize)
    };
    let output_content = terminal_content_area(output_area, &app.server.run.terminal);
    frame.render_widget(
        Paragraph::new(output_lines)
            .style(surface_style())
            .wrap(Wrap { trim: false }),
        output_content,
    );
    render_output_scrollbar(frame, output_area, &app.server.run.terminal);
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
    Style::default()
        .fg(theme().text_muted)
        .bg(theme().footer_background)
}

fn footer_rail_style() -> Style {
    Style::default()
        .fg(theme().text_faint)
        .bg(theme().footer_background)
}

fn prefixed_line(text: impl Into<String>) -> Line<'static> {
    Line::from(text.into())
}

fn render_help(frame: &mut Frame<'_>, app: &TuiApp) {
    let area = adaptive_data_popup_rect(frame.area(), 48, 12);
    let content = OverlayFrame {
        title: Some("Help"),
    }
    .render_shell(frame, area, &theme());
    if content.height == 0 {
        return;
    }
    let footer_height = content.height.min(1);
    let body = Rect {
        height: content.height.saturating_sub(footer_height),
        ..content
    };
    let footer = Rect {
        y: body.bottom(),
        height: footer_height,
        ..content
    };
    let mut lines = help_lines(app, body.width as usize);
    let overflow = lines.len() > body.height as usize;
    if overflow {
        lines = help_lines(app, body.width.saturating_sub(1) as usize);
    }
    app.help_content_height.set(lines.len());
    app.help_viewport_height.set(body.height as usize);
    let offset = app
        .help_scroll
        .min(lines.len().saturating_sub(body.height as usize));
    frame.render_widget(
        Paragraph::new(lines)
            .style(
                Style::default()
                    .fg(theme().text)
                    .bg(theme().overlay_background),
            )
            .scroll((offset.min(u16::MAX as usize) as u16, 0)),
        Rect {
            width: body.width.saturating_sub(u16::from(overflow)),
            ..body
        },
    );
    if footer.height > 0 {
        frame.render_widget(
            Paragraph::new(shortcut_line(
                &[
                    KeyHint {
                        key: "↑/↓ / pgup/pgdn",
                        label: "Scroll",
                    },
                    KeyHint {
                        key: "esc",
                        label: "Close",
                    },
                ],
                &theme(),
                Alignment::Center,
            ))
            .style(Style::default().bg(theme().overlay_background)),
            footer,
        );
    }
    render_overlay_scrollbar(frame, body, app.help_content_height.get(), offset);
}

fn help_lines(app: &TuiApp, width: usize) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(Span::styled(
            fit("Keyboard Shortcuts", width),
            Style::default().add_modifier(Modifier::BOLD),
        ))
        .alignment(Alignment::Left),
        Line::default(),
    ];

    push_action_section_for_app(
        &mut lines,
        app,
        width,
        "Global",
        &[
            TuiActionId::OpenPalette,
            TuiActionId::OpenContextPicker,
            TuiActionId::ShowHelp,
            TuiActionId::Quit,
        ],
    );
    push_section_break(&mut lines, width, "Leader");
    push_raw_help(&mut lines, width, "ctrl+x w", "Workspaces");
    push_raw_help(&mut lines, width, "ctrl+x v", "Views");
    push_raw_help(&mut lines, width, "ctrl+x l", "Layout mode");
    push_raw_help(&mut lines, width, "ctrl+x a", "Context actions");
    push_raw_help(&mut lines, width, "ctrl+x j", "Jobs");
    push_raw_help(&mut lines, width, "ctrl+x e", "Context");

    push_section_break(&mut lines, width, "Palette");
    push_raw_help(&mut lines, width, "type", "Filter commands");
    push_raw_help(&mut lines, width, "up / down", "Move selection");
    push_raw_help(&mut lines, width, "enter", "Execute selected action");
    push_raw_help(&mut lines, width, "esc / ctrl+c", "Close palette");

    if app.active_workspace == TuiWorkspace::Dev {
        push_section_break(&mut lines, width, "Dev panes");
        push_raw_help(&mut lines, width, "tab / shift+tab", "Focus visible pane");
        push_raw_help(
            &mut lines,
            width,
            "ctrl+x l",
            "Resize, flip, show, or hide panes",
        );
        push_raw_help(&mut lines, width, "pageup", "Enter terminal scrollback");
        push_raw_help(&mut lines, width, "mouse wheel", "Scroll terminal history");
        push_raw_help(
            &mut lines,
            width,
            "esc",
            "Return from scrollback to live input",
        );
    } else if app.ops_view == OpsView::Metrics {
        push_section_break(&mut lines, width, "Metrics");
        push_raw_help(&mut lines, width, "ctrl+x v o", "Return to Overview");
        lines.push(
            Line::from(Span::styled(
                fit(
                    "  Metrics provider is not configured; preview samples never appear here.",
                    width,
                ),
                Style::default().fg(theme().text_muted),
            ))
            .alignment(Alignment::Left),
        );
    } else if app.ops_agents_focused() {
        push_action_section_for_app(
            &mut lines,
            app,
            width,
            "Agents Overview",
            &[
                TuiActionId::RefreshAgents,
                TuiActionId::OpenAgentDatasetFilter,
                TuiActionId::IncludeAgentMatches,
                TuiActionId::ExcludeAgentMatches,
                TuiActionId::LoadMoreAgents,
                TuiActionId::ToggleAgentAutoRefresh,
                TuiActionId::ToggleAgentDetails,
                TuiActionId::CycleAgentMode,
            ],
        );
        push_raw_help(&mut lines, width, "type", "Find within loaded agents");
        push_raw_help(&mut lines, width, "ctrl+x f", "Change local find field");
        push_raw_help(
            &mut lines,
            width,
            "tab / shift+tab",
            "Focus list or details",
        );
        push_raw_help(
            &mut lines,
            width,
            "up / down",
            "Move or scroll focused pane",
        );
        push_raw_help(&mut lines, width, "pageup / pagedown", "Move by page");
        push_raw_help(&mut lines, width, "home / end", "Move to first / last row");
        push_raw_help(&mut lines, width, "left / right", "Pan visible columns");
        push_raw_help(
            &mut lines,
            width,
            "ctrl+space",
            "Include or exclude focused agent",
        );
        push_raw_help(&mut lines, width, "enter", "Open selected Agent");
        push_raw_help(
            &mut lines,
            width,
            "ctrl+x a",
            "Columns and collection actions",
        );
        push_raw_help(&mut lines, width, "ctrl+x l", "Resize or hide details");
    } else {
        push_section_break(&mut lines, width, "Ops subject");
        push_raw_help(&mut lines, width, "up / down", "Scroll Overview");
        push_raw_help(&mut lines, width, "pageup / pagedown", "Scroll by page");
        push_raw_help(&mut lines, width, "home / end", "First / last line");
        push_raw_help(&mut lines, width, "esc", "Return to parent subject");
        push_raw_help(&mut lines, width, "ctrl+r", "Refresh typed provider data");
    }

    lines.push(Line::default());
    lines.push(Line::from(fit("Press esc to close this help.", width)).alignment(Alignment::Left));
    lines
}

fn push_action_section_for_app(
    lines: &mut Vec<Line<'static>>,
    app: &TuiApp,
    width: usize,
    title: &'static str,
    ids: &[TuiActionId],
) {
    push_section_break(lines, width, title);
    for id in ids {
        push_action_help_for_app(lines, app, action(*id), width);
    }
}

fn push_section_break(lines: &mut Vec<Line<'static>>, width: usize, title: &'static str) {
    if lines.last().is_some_and(|line| !line.spans.is_empty()) {
        lines.push(Line::default());
    }
    lines.push(
        Line::from(Span::styled(
            fit(title, width),
            Style::default()
                .fg(theme().text)
                .add_modifier(Modifier::BOLD),
        ))
        .alignment(Alignment::Left),
    );
}

fn push_action_help_for_app(
    lines: &mut Vec<Line<'static>>,
    app: &TuiApp,
    action: &TuiAction,
    width: usize,
) {
    let shortcut = action.shortcut.unwrap_or("-");
    let suffix = match app.action_availability(action) {
        TuiActionAvailability::Available => String::new(),
        TuiActionAvailability::Unavailable(reason) => format!(" ({reason})"),
    };
    let label = format!("{}{suffix}", action.label);
    lines.push(
        HelpRow {
            key: shortcut,
            label: &label,
        }
        .line(width, &theme()),
    );
}

fn push_raw_help(
    lines: &mut Vec<Line<'static>>,
    width: usize,
    shortcut: &'static str,
    label: &'static str,
) {
    lines.push(
        HelpRow {
            key: shortcut,
            label,
        }
        .line(width, &theme()),
    );
}

fn render_leader_hint(frame: &mut Frame<'_>, app: &TuiApp) {
    let area = Rect {
        x: frame.area().x,
        y: frame.area().y + frame.area().height.saturating_sub(2),
        width: frame.area().width,
        height: 1,
    };
    let entries: &[(&str, &str)] = match app.leader.group {
        None => &[
            ("w", "Workspaces"),
            ("v", "Views"),
            ("l", "Layout"),
            ("a", "Actions"),
            ("j", "Jobs"),
            ("e", "Context"),
        ],
        Some(LeaderGroup::Workspaces) => {
            &[("n", "Next"), ("p", "Previous"), ("d", "Dev"), ("o", "Ops")]
        }
        Some(LeaderGroup::Views) if app.active_workspace == TuiWorkspace::Dev => {
            &[("b", "Build & Deploy"), ("s", "Server"), ("r", "REPL")]
        }
        Some(LeaderGroup::Views) => &[("o", "Overview"), ("a", "Activity"), ("m", "Metrics")],
    };
    let items = entries
        .iter()
        .map(|(key, label)| KeyHint { key, label })
        .collect::<Vec<_>>();
    ShortcutRow {
        items: &items,
        active: true,
        left_glyph: "│",
        fallback: None,
    }
    .render(frame, area, &theme());
}

fn render_output_view(frame: &mut Frame<'_>, area: Rect, app: &TuiApp) {
    frame.render_widget(Paragraph::new("").style(surface_style()), area);
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
        None => status_summary_line(
            StatusKind::Idle,
            "idle".to_string(),
            &[
                ("job", "none".to_string()),
                ("yes", flag_state(app.command_options.yes).to_string()),
                ("reset", flag_state(app.command_options.reset).to_string()),
                ("actions", "ctrl+x a".to_string()),
            ],
        ),
    };
    frame.render_widget(Paragraph::new(summary).style(surface_style()), summary_area);

    let output_lines = run
        .map(|run| render_output_lines(&run.terminal, output_area.height as usize))
        .unwrap_or_else(|| vec![prefixed_line("Output will appear here.")]);
    let output_content = run
        .map(|run| terminal_content_area(output_area, &run.terminal))
        .unwrap_or(output_area);
    frame.render_widget(
        Paragraph::new(output_lines)
            .style(surface_style())
            .wrap(Wrap { trim: false }),
        output_content,
    );
    if let Some(run) = run {
        render_output_scrollbar(frame, output_area, &run.terminal);
    }

    if has_input {
        let prompt = "stdin ";
        frame.render_widget(
            Paragraph::new(Line::from(prompt)).style(surface_style()),
            input_area,
        );
        frame.set_cursor_position((input_area.x + prompt.len() as u16, input_area.y));
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

fn render_output_scrollbar(frame: &mut Frame<'_>, area: Rect, output: &TerminalScreen) {
    let visible_height = area.height as usize;
    let total = output.total_lines();
    if output.max_scrollback() == 0 || area.width == 0 || visible_height == 0 {
        return;
    }

    let position = output_scrollbar_position(total, visible_height, output.scrollback());
    let mut state = ScrollbarState::new(total)
        .position(position)
        .viewport_content_length(visible_height);

    render_scrollbar(
        frame,
        PaneScrollbarSlot {
            area: Rect::new(area.right().saturating_sub(1), area.y, 1, area.height),
        },
        &mut state,
        &theme(),
    );
}

fn terminal_content_area(area: Rect, output: &TerminalScreen) -> Rect {
    Rect {
        width: area.width.saturating_sub(u16::from(
            output.max_scrollback() > 0 && area.width > 0 && area.height > 0,
        )),
        ..area
    }
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

fn command_status_line(run: &CommandRun) -> Line<'static> {
    let hint = if matches!(
        run.status,
        CommandStatus::Running | CommandStatus::Cancelling
    ) {
        "esc / ctrl+c cancel"
    } else {
        "ctrl+x a run again"
    };
    status_summary_line(
        command_status_kind(run.status),
        command_status_display(run),
        &[
            ("job", run.kind.title().to_string()),
            ("yes", flag_state(run.options.yes).to_string()),
            ("reset", flag_state(run.options.reset).to_string()),
            ("command", command_display(&run.args)),
            ("context", run.launch_context.clone()),
            ("action", hint.to_string()),
        ],
    )
}

fn server_status_line(server: &LocalServerService, local_server_available: bool) -> Line<'static> {
    let run = &server.run;
    let clean = run.clean;
    let ports = run.bound_ports.as_ref().map_or_else(
        || "ports: pending".to_string(),
        |ports| {
            format!(
                "ports: {}/{}/{}",
                ports.router_port, ports.custom_request_port, ports.mcp_port
            )
        },
    );
    status_summary_line(
        server_status_kind(run.status),
        server_status_display(run),
        &[
            ("session", "server".to_string()),
            ("clean", flag_state(clean).to_string()),
            ("ports", ports.trim_start_matches("ports: ").to_string()),
            ("command", command_display(&run.args)),
            ("context", run.launch_context.clone()),
            (
                "action",
                server_hint(run, local_server_available).to_string(),
            ),
        ],
    )
}

fn repl_status_line(repl: &ReplState, mode: TuiMode) -> Line<'static> {
    let run = &repl.run;
    status_summary_line(
        repl_status_kind(run.status),
        run.status.title().to_string(),
        &[
            ("session", "repl".to_string()),
            ("command", command_display(&run.args)),
            ("context", run.launch_context.clone()),
            ("action", repl_mode_hint(run, mode).to_string()),
        ],
    )
}

fn repl_status_kind(status: ReplStatus) -> StatusKind {
    match status {
        ReplStatus::Starting | ReplStatus::Running | ReplStatus::Stopping => StatusKind::Running,
        ReplStatus::Stopped => StatusKind::Idle,
        ReplStatus::Failed => StatusKind::Failed,
    }
}

fn repl_mode_hint(run: &ReplRun, mode: TuiMode) -> &'static str {
    match mode {
        TuiMode::Repl => "ctrl+x q leave  ctrl+x k stop",
        TuiMode::Leader => "ctrl+x ctrl+x send literal ctrl+x",
        _ if run.is_running() => "ctrl+x a focus / stop",
        _ => "ctrl+x a start",
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

fn server_status_kind(status: ServerStatus) -> StatusKind {
    match status {
        ServerStatus::Starting | ServerStatus::Running | ServerStatus::Stopping => {
            StatusKind::Running
        }
        ServerStatus::Stopped => StatusKind::Idle,
        ServerStatus::Failed => StatusKind::Failed,
    }
}

fn server_hint(run: &ServerRun, local_server_available: bool) -> &'static str {
    if !local_server_available {
        return "local server unavailable";
    }
    match run.status {
        ServerStatus::Starting | ServerStatus::Running => "ctrl+x a stop",
        ServerStatus::Stopping => "ctrl+x a force stop",
        ServerStatus::Stopped | ServerStatus::Failed => "ctrl+x a start / restart",
    }
}

fn command_status_kind(status: CommandStatus) -> StatusKind {
    match status {
        CommandStatus::Running | CommandStatus::Cancelling | CommandStatus::Succeeded => {
            StatusKind::Running
        }
        CommandStatus::Failed | CommandStatus::Killed => StatusKind::Failed,
    }
}

fn status_summary_line(
    kind: StatusKind,
    status: String,
    fields: &[(&'static str, String)],
) -> Line<'static> {
    let mut spans = StatusMarker {
        kind,
        label: &status,
    }
    .spans(&theme())
    .to_vec();
    for (label, value) in fields {
        spans.push(Span::styled(
            " · ",
            Style::default().fg(theme().border_subtle),
        ));
        spans.push(Span::styled(
            format!("{label} "),
            Style::default().fg(theme().text_muted),
        ));
        spans.push(Span::styled(
            value.clone(),
            Style::default().fg(theme().text_secondary),
        ));
    }
    Line::from(spans)
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

fn flag_state(enabled: bool) -> &'static str {
    if enabled { "on" } else { "off" }
}

fn render_output_lines(buffer: &TerminalScreen, height: usize) -> Vec<Line<'static>> {
    buffer
        .lines()
        .into_iter()
        .take(height)
        .map(output_line)
        .collect()
}

fn repl_screen_lines(run: &ReplRun, height: usize) -> Vec<Line<'static>> {
    let mut lines = run
        .screen
        .lines()
        .into_iter()
        .take(height)
        .map(output_line)
        .collect::<Vec<_>>();

    if let Some(error) = &run.last_error
        && !lines.is_empty()
    {
        lines[0] = Line::from(Span::styled(
            error.clone(),
            Style::default().fg(theme().error),
        ));
    }

    lines
}

fn repl_screen_size_for_terminal(cols: u16, rows: u16) -> (u16, u16) {
    let screen_rows = rows.saturating_sub(5).max(1);
    let screen_cols = cols.saturating_sub(2).max(1);
    (screen_rows, screen_cols)
}

fn pty_size_for_region(area: Rect) -> (u16, u16) {
    (
        area.width.saturating_sub(2).max(1),
        area.height.saturating_sub(1).max(1),
    )
}

fn output_line(line: Line<'static>) -> Line<'static> {
    Line::from(
        line.spans
            .into_iter()
            .map(span_with_surface_bg)
            .collect::<Vec<_>>(),
    )
}

fn span_with_surface_bg(mut span: Span<'static>) -> Span<'static> {
    span.style = span.style.bg(theme().surface);
    span
}

fn render_context_picker(frame: &mut Frame<'_>, app: &TuiApp) {
    let selected = app
        .context_switcher
        .selected
        .min(app.context_switcher.row_count().saturating_sub(1));
    let rows = context_picker_rows(&app.context_switcher);
    let area = adaptive_data_popup_rect(frame.area(), 48, 10);
    let content = OverlayFrame::content_area(area);
    let (title, subtitle) = match app.context_switcher.mode {
        ContextPickerStep::Targets => (
            "Switch Context",
            format!("Current: {}", app.context.short_label()),
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
        ),
    };
    let mut lines = vec![
        Notice {
            kind: NoticeKind::Info,
            message: &subtitle,
        }
        .line(&theme()),
        Line::default(),
    ];

    if app.context_switcher.switch_running {
        lines.push(
            Notice {
                kind: NoticeKind::Loading,
                message: "Switching context",
            }
            .line(&theme()),
        );
        lines.push(Line::default());
    }
    if let Some(error) = &app.context_switcher.last_error {
        lines.push(
            Notice {
                kind: NoticeKind::Error,
                message: error,
            }
            .line(&theme()),
        );
        lines.push(Line::default());
    }

    let prefix_height = context_picker_prefix_height(&app.context_switcher);
    debug_assert_eq!(prefix_height, lines.len());
    let row_capacity = (content.height as usize)
        .saturating_sub(prefix_height)
        .saturating_sub(2)
        .max(1);
    let selected_line = rows
        .iter()
        .position(|row| {
            matches!(
                row,
                ContextPickerRenderRow::Item {
                    selectable_index,
                    ..
                } if *selectable_index == selected
            )
        })
        .unwrap_or_default();
    let mut start = selected_line.saturating_sub(row_capacity.saturating_sub(1));
    if start > 0
        && matches!(
            rows.get(start.saturating_sub(1)),
            Some(ContextPickerRenderRow::Section(_))
        )
        && selected_line.saturating_sub(start).saturating_add(2) <= row_capacity
    {
        start = start.saturating_sub(1);
    }
    let overflow = rows.len() > row_capacity;
    let label_width = context_picker_label_width(&rows);
    let content_width = content.width.saturating_sub(u16::from(overflow)) as usize;
    lines.extend(
        rows.iter()
            .skip(start)
            .take(row_capacity)
            .map(|row| context_picker_render_line(row, selected, label_width, content_width)),
    );

    lines.push(Line::default());
    let footer_items = match app.context_switcher.mode {
        ContextPickerStep::Targets => [
            KeyHint {
                key: "enter",
                label: "Select",
            },
            KeyHint {
                key: "↑/↓",
                label: "Navigate",
            },
            KeyHint {
                key: "esc",
                label: "Close",
            },
        ],
        ContextPickerStep::AppEnvironments => [
            KeyHint {
                key: "enter",
                label: "Switch",
            },
            KeyHint {
                key: "↑/↓",
                label: "Navigate",
            },
            KeyHint {
                key: "esc",
                label: "Servers",
            },
        ],
    };
    let footer = shortcut_line(&footer_items, &theme(), Alignment::Center);
    let lines = overlay_lines_with_footer(lines, footer, content.height);

    OverlayFrame { title: Some(title) }.render(frame, area, lines, &theme());
    if overflow {
        render_overlay_scrollbar(
            frame,
            Rect {
                y: content.y.saturating_add(prefix_height as u16),
                height: row_capacity.min(u16::MAX as usize) as u16,
                ..content
            },
            rows.len(),
            selected_line,
        );
    }
    if app.context_switcher.environment_list_running {
        render_context_environment_loading(frame, app);
    }
}

fn render_context_switch_confirm(frame: &mut Frame<'_>, app: &TuiApp) {
    let area = centered_rect_fixed(72, 11, frame.area());
    let message = app
        .context_switch_dev_blocker_message()
        .unwrap_or_else(|| "Switching context will stop running dev jobs.".to_string());
    let (status_kind, status) = if app.context_switcher.waiting_for_dev_stop {
        (NoticeKind::Loading, "Waiting for dev jobs to stop...")
    } else {
        (
            NoticeKind::Info,
            "Press enter to stop them gracefully, or esc to keep the current context.",
        )
    };
    let lines = vec![
        Notice {
            kind: NoticeKind::Warning,
            message: &message,
        }
        .line(&theme()),
        Line::default(),
        Notice {
            kind: status_kind,
            message: status,
        }
        .line(&theme()),
        Line::default(),
        shortcut_line(
            &[
                KeyHint {
                    key: "enter",
                    label: "Confirm",
                },
                KeyHint {
                    key: "esc",
                    label: "Cancel",
                },
            ],
            &theme(),
            Alignment::Center,
        ),
    ];
    OverlayFrame {
        title: Some("Switch context?"),
    }
    .render(frame, area, lines, &theme());
}

fn render_context_environment_loading(frame: &mut Frame<'_>, app: &TuiApp) {
    let area = centered_rect_fixed(52, 9, frame.area());
    let spinner = spinner_symbol(app.context_switcher.environment_list_spinner_frame);
    let message = format!("{spinner} Loading app environments");
    let lines = vec![
        Notice {
            kind: NoticeKind::Loading,
            message: &message,
        }
        .line(&theme()),
        Line::default(),
        shortcut_line(
            &[
                KeyHint {
                    key: "esc",
                    label: "Cancel",
                },
                KeyHint {
                    key: "ctrl+q",
                    label: "Quit",
                },
            ],
            &theme(),
            Alignment::Center,
        ),
    ];
    OverlayFrame {
        title: Some("Loading"),
    }
    .render(frame, area, lines, &theme());
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

fn context_picker_rows(context_switcher: &ContextSwitcherState) -> Vec<ContextPickerRenderRow> {
    match context_switcher.mode {
        ContextPickerStep::Targets => {
            context_switcher
                .targets
                .iter()
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

fn context_picker_layout_rows(context_switcher: &ContextSwitcherState) -> Vec<Option<usize>> {
    context_picker_rows(context_switcher)
        .into_iter()
        .map(|row| match row {
            ContextPickerRenderRow::Section(_) => None,
            ContextPickerRenderRow::Item {
                selectable_index, ..
            } => Some(selectable_index),
        })
        .collect()
}

fn context_picker_prefix_height(context_switcher: &ContextSwitcherState) -> usize {
    2 + usize::from(context_switcher.switch_running) * 2
        + usize::from(context_switcher.last_error.is_some()) * 2
}

fn context_picker_render_line(
    row: &ContextPickerRenderRow,
    selected: usize,
    label_width: usize,
    content_width: usize,
) -> Line<'static> {
    match row {
        ContextPickerRenderRow::Section(label) => SectionHeading {
            title: label,
            detail: None,
        }
        .line(&theme()),
        ContextPickerRenderRow::Item {
            selectable_index,
            label,
            marker,
            detail,
            unavailable,
        } => {
            let is_selected = *selectable_index == selected;
            let marker_width = marker
                .as_ref()
                .map(|marker| marker.chars().count() + 1)
                .unwrap_or_default();
            let fixed_width = 2 + label_width + 2;
            let detail_width = content_width
                .saturating_sub(fixed_width + marker_width + CONTEXT_PICKER_RIGHT_PADDING);
            let detail_text = if marker.is_some() {
                pad_or_ellipsis(detail, detail_width)
            } else {
                ellipsis_text(detail, detail_width)
            };
            let badge = marker
                .as_ref()
                .map(|marker| format!(" {marker}"))
                .unwrap_or_default();
            let text = format!(
                "{}  {}{}",
                pad_or_ellipsis(label, label_width),
                detail_text,
                badge
            );
            SelectableRow {
                label: &text,
                selected: is_selected,
                unavailable: *unavailable,
            }
            .line(content_width, &theme())
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

fn render_command_form(frame: &mut Frame<'_>, app: &TuiApp) {
    let Some(form) = app.command_form.as_ref() else {
        return;
    };
    let area = adaptive_data_popup_rect(frame.area(), 58, 16);
    let content = OverlayFrame::content_area(area);
    let mut fields = command_form_rows(form);
    let mut lines = vec![
        Notice {
            kind: if form.kind == CommandKind::Clean
                || form.values.post_deploy == PostDeployPolicy::Reset
                || form.values.post_deploy == PostDeployPolicy::Redeploy
            {
                NoticeKind::Warning
            } else {
                NoticeKind::Info
            },
            message: match form.kind {
                CommandKind::Plan => {
                    "Runs the real nested CLI with deploy --plan; no changes are applied."
                }
                CommandKind::Build => "Runs the real nested CLI for the full application build.",
                CommandKind::Deploy => {
                    "Runs the real nested CLI for the selected application environment."
                }
                CommandKind::Clean => {
                    "Removes all application build artifacts; source files are retained."
                }
            },
        }
        .line(&theme()),
        Line::default(),
    ];
    for (index, (label, value)) in fields.drain(..).enumerate() {
        let text = format!("{label:<18} {value}");
        lines.push(
            SelectableRow {
                label: &text,
                selected: index == form.selected,
                unavailable: false,
            }
            .line(content.width as usize, &theme()),
        );
    }
    lines.push(Line::default());
    let footer = shortcut_line(
        &[
            KeyHint {
                key: "↑/↓",
                label: "Field",
            },
            KeyHint {
                key: "space / enter",
                label: "Change",
            },
            KeyHint {
                key: "esc",
                label: "Cancel",
            },
        ],
        &theme(),
        Alignment::Center,
    );
    let title = format!("Review {}", form.kind.title());
    OverlayFrame {
        title: Some(&title),
    }
    .render(
        frame,
        area,
        overlay_lines_with_footer(lines, footer, content.height),
        &theme(),
    );
}

fn command_form_rows(form: &CommandReviewForm) -> Vec<(String, String)> {
    let toggle = |enabled: bool| {
        if enabled {
            "[x]".to_string()
        } else {
            "[ ]".to_string()
        }
    };
    match form.kind {
        CommandKind::Build => ["check", "build", "add-metadata", "gen-bridge"]
            .into_iter()
            .enumerate()
            .map(|(index, step)| {
                (
                    format!("step {step}"),
                    toggle(form.values.build_steps[index]),
                )
            })
            .chain([
                ("skip check".to_string(), toggle(form.values.skip_check)),
                ("force build".to_string(), toggle(form.values.force_build)),
                ("--yes".to_string(), toggle(form.values.yes)),
                ("Run".to_string(), "nested CLI PTY".to_string()),
            ])
            .collect(),
        CommandKind::Plan | CommandKind::Deploy => vec![
            ("full diff".to_string(), toggle(form.values.full_diff)),
            ("force build".to_string(), toggle(form.values.force_build)),
            (
                "rollback version".to_string(),
                if form.values.rollback_version.is_empty() {
                    "-".to_string()
                } else {
                    form.values.rollback_version.clone()
                },
            ),
            (
                "rollback revision".to_string(),
                if form.values.rollback_revision.is_empty() {
                    "-".to_string()
                } else {
                    form.values.rollback_revision.clone()
                },
            ),
            (
                "post deploy".to_string(),
                form.values.post_deploy.label().to_string(),
            ),
            ("--yes".to_string(), toggle(form.values.yes)),
            ("Run".to_string(), "nested CLI PTY".to_string()),
        ],
        CommandKind::Clean => vec![
            ("--yes".to_string(), toggle(form.values.yes)),
            ("Run".to_string(), "nested CLI PTY".to_string()),
        ],
    }
}

fn render_session_form(frame: &mut Frame<'_>, app: &TuiApp) {
    let Some(form) = app.session_form.as_ref() else {
        return;
    };
    let area = adaptive_data_popup_rect(frame.area(), 64, 18);
    let content = OverlayFrame::content_area(area);
    let (title, notice, error, fields) = match form {
        SessionReviewForm::Server {
            values,
            restart,
            error,
            ..
        } => (
            if *restart {
                "Review Server restart"
            } else {
                "Review Server start"
            },
            "Running is established only after the process stays alive and rewrites a parseable ports file.",
            error.as_deref(),
            server_form_rows(values),
        ),
        SessionReviewForm::Repl {
            values, restart, ..
        } => (
            if *restart {
                "Review REPL restart"
            } else {
                "Review REPL start"
            },
            "Starts the real interactive nested CLI in a persistent PTY.",
            None,
            repl_form_rows(values),
        ),
    };
    let mut lines = vec![
        Notice {
            kind: NoticeKind::Info,
            message: notice,
        }
        .line(&theme()),
        Line::default(),
    ];
    if let Some(error) = error {
        lines.push(
            Notice {
                kind: NoticeKind::Error,
                message: error,
            }
            .line(&theme()),
        );
        lines.push(Line::default());
    }
    for (index, (label, value)) in fields.into_iter().enumerate() {
        let text = format!("{label:<22} {value}");
        lines.push(
            SelectableRow {
                label: &text,
                selected: index == form.selected(),
                unavailable: false,
            }
            .line(content.width as usize, &theme()),
        );
    }
    lines.push(Line::default());
    let footer = shortcut_line(
        &[
            KeyHint {
                key: "↑/↓",
                label: "Field",
            },
            KeyHint {
                key: "space / enter",
                label: "Change",
            },
            KeyHint {
                key: "esc",
                label: "Cancel",
            },
        ],
        &theme(),
        Alignment::Center,
    );
    OverlayFrame { title: Some(title) }.render(
        frame,
        area,
        overlay_lines_with_footer(lines, footer, content.height),
        &theme(),
    );
}

fn server_form_rows(values: &ServerFormValues) -> Vec<(String, String)> {
    let shown = |value: &str| {
        if value.is_empty() {
            "<default>".to_string()
        } else {
            value.to_string()
        }
    };
    vec![
        (
            "system memory".to_string(),
            shown(&values.system_memory_override),
        ),
        ("router address".to_string(), shown(&values.router_address)),
        ("router port".to_string(), shown(&values.router_port)),
        (
            "custom request port".to_string(),
            shown(&values.custom_request_port),
        ),
        ("MCP port".to_string(), shown(&values.mcp_port)),
        ("ports file".to_string(), shown(&values.ports_file)),
        ("data directory".to_string(), shown(&values.data_dir)),
        (
            "agent filesystem root".to_string(),
            shown(&values.agent_filesystem_root),
        ),
        (
            "clean".to_string(),
            if values.clean { "[x]" } else { "[ ]" }.to_string(),
        ),
        (
            "--yes".to_string(),
            if values.yes { "[x]" } else { "[ ]" }.to_string(),
        ),
        ("Run".to_string(), "nested CLI PTY".to_string()),
    ]
}

fn repl_form_rows(values: &ReplFormValues) -> Vec<(String, String)> {
    vec![
        ("language".to_string(), values.language.clone()),
        (
            "stream logs".to_string(),
            if values.stream_logs { "[x]" } else { "[ ]" }.to_string(),
        ),
        (
            "automatic imports".to_string(),
            if values.auto_imports { "[x]" } else { "[ ]" }.to_string(),
        ),
        (
            "post deploy".to_string(),
            values.post_deploy.label().to_string(),
        ),
        (
            "--yes".to_string(),
            if values.yes { "[x]" } else { "[ ]" }.to_string(),
        ),
        ("Run".to_string(), "nested CLI PTY".to_string()),
    ]
}

fn render_jobs(frame: &mut Frame<'_>, app: &TuiApp) {
    let area = adaptive_data_popup_rect(frame.area(), 52, 12);
    let content = OverlayFrame::content_area(area);
    let capacity = content.height.saturating_sub(3).max(1) as usize;
    let mut jobs = app.command_run.iter().collect::<Vec<_>>();
    jobs.extend(app.completed_jobs.iter());
    let selected = app.selected_job.min(jobs.len().saturating_sub(1));
    let start = selected.saturating_sub(capacity.saturating_sub(1));
    let mut lines = Vec::new();
    if jobs.is_empty() {
        lines.push(
            Notice {
                kind: NoticeKind::Empty,
                message: "No finite Jobs have run in this session",
            }
            .line(&theme()),
        );
    } else {
        for (offset, run) in jobs.iter().skip(start).take(capacity).enumerate() {
            let elapsed = run.elapsed().as_secs_f32();
            let label = format!(
                "#{:<3} {:<8} {:<11} {:>6.1}s  {}",
                run.id,
                run.kind.title(),
                run.status.title(),
                elapsed,
                run.launch_context
            );
            lines.push(
                SelectableRow {
                    label: &label,
                    selected: start + offset == selected,
                    unavailable: false,
                }
                .line(content.width as usize, &theme()),
            );
        }
    }
    lines.push(Line::default());
    let footer = shortcut_line(
        &[
            KeyHint {
                key: "↑/↓",
                label: "Navigate",
            },
            KeyHint {
                key: "enter",
                label: "Open output",
            },
            KeyHint {
                key: "esc",
                label: "Close",
            },
        ],
        &theme(),
        Alignment::Center,
    );
    OverlayFrame {
        title: Some("Jobs · latest 20 completed"),
    }
    .render(
        frame,
        area,
        overlay_lines_with_footer(lines, footer, content.height),
        &theme(),
    );
}

fn render_quit_blocked(frame: &mut Frame<'_>, app: &TuiApp) {
    let area = centered_rect_fixed(64, 9, frame.area());
    let mut active = Vec::new();
    if app.command_is_running() {
        active.push("finite Job");
    }
    if app.server.run.is_running() {
        active.push("Server");
    }
    if app.repl.is_running() {
        active.push("REPL");
    }
    let message = format!(
        "Quit is blocked while TUI-owned work is active: {}.",
        active.join(", ")
    );
    let lines = vec![
        Notice {
            kind: NoticeKind::Warning,
            message: &message,
        }
        .line(&theme()),
        Line::default(),
        Notice {
            kind: NoticeKind::Info,
            message: "Stop the active work from its view, then quit again.",
        }
        .line(&theme()),
        Line::default(),
        shortcut_line(
            &[KeyHint {
                key: "enter / esc",
                label: "Return",
            }],
            &theme(),
            Alignment::Center,
        ),
    ];
    OverlayFrame {
        title: Some("Active work prevents quit"),
    }
    .render(frame, area, lines, &theme());
}

fn render_palette(frame: &mut Frame<'_>, app: &TuiApp) {
    let actions = app.filtered_actions();
    let area = adaptive_data_popup_rect(frame.area(), 36, 10);
    let content = OverlayFrame::content_area(area);
    let action_capacity = content.height.saturating_sub(5).max(1) as usize;
    let overflow = actions.len() > action_capacity;
    let content_width = content.width.saturating_sub(u16::from(overflow)) as usize;
    let selected = app.palette.selected.min(actions.len().saturating_sub(1));
    let visible_count = action_capacity.min(actions.len());
    let start = selected.saturating_sub(visible_count.saturating_sub(1));
    let visible_actions = actions
        .iter()
        .skip(start)
        .take(visible_count)
        .copied()
        .collect::<Vec<_>>();
    let scope_line = Line::from(vec![
        Span::styled("Scope  ", Style::default().fg(theme().text_muted)),
        Span::styled(
            if app.palette.scope == PaletteScope::All {
                "[All]"
            } else {
                " All "
            },
            if app.palette.scope == PaletteScope::All {
                Style::default()
                    .fg(theme().accent_hover)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme().text_muted)
            },
        ),
        Span::raw("  "),
        Span::styled(
            if app.palette.scope == PaletteScope::Context {
                "[Context]"
            } else {
                " Context "
            },
            if app.palette.scope == PaletteScope::Context {
                Style::default()
                    .fg(theme().accent_hover)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme().text_muted)
            },
        ),
    ]);
    let mut lines = vec![
        scope_line,
        SearchInput {
            query: &app.palette.query,
        }
        .line(content_width, &theme()),
        Line::default(),
    ];

    if actions.is_empty() {
        lines.push(
            Notice {
                kind: NoticeKind::Empty,
                message: "No matching commands",
            }
            .line(&theme()),
        );
    } else {
        for (index, action) in visible_actions.iter().enumerate() {
            let availability = app.action_availability(action);
            let reason = match availability {
                TuiActionAvailability::Available => String::new(),
                TuiActionAvailability::Unavailable(reason) => {
                    format!(" — unavailable: {reason}")
                }
            };
            let review = if action.review() == TuiActionReview::ReviewForm {
                " · review"
            } else {
                ""
            };
            let description = format!(
                "{}{reason} · {} · target: {}{review}",
                action.description,
                action.risk().label(),
                app.action_target_summary(action)
            );
            lines.push(
                CommandRow {
                    label: action.label,
                    shortcut: action.shortcut,
                    description: &description,
                    selected: start + index == selected,
                    unavailable: availability.is_unavailable(),
                }
                .line(content_width, &theme()),
            );
        }
    }
    lines.push(Line::default());
    let footer = shortcut_line(
        &[
            KeyHint {
                key: "↑/↓",
                label: "Navigate",
            },
            KeyHint {
                key: "enter",
                label: "Run",
            },
            KeyHint {
                key: "ctrl+p / ctrl+x a",
                label: "Scope",
            },
            KeyHint {
                key: "esc",
                label: "Close",
            },
        ],
        &theme(),
        Alignment::Center,
    );
    let lines = overlay_lines_with_footer(lines, footer, content.height);
    OverlayFrame {
        title: Some("Commands"),
    }
    .render(frame, area, lines, &theme());
    if overflow {
        render_overlay_scrollbar(
            frame,
            Rect {
                y: content.y.saturating_add(3),
                height: content.height.saturating_sub(5),
                ..content
            },
            actions.len(),
            selected,
        );
    }
}

fn palette_line(mut spans: Vec<Span<'static>>) -> Line<'static> {
    Line::from(std::mem::take(&mut spans))
}

fn overlay_lines_with_footer(
    mut body: Vec<Line<'static>>,
    footer: Line<'static>,
    height: u16,
) -> Vec<Line<'static>> {
    if height == 0 {
        return Vec::new();
    }
    let body_height = height.saturating_sub(1) as usize;
    body.truncate(body_height);
    body.resize(body_height, Line::default());
    body.push(footer);
    body
}

fn render_overlay_scrollbar(
    frame: &mut Frame<'_>,
    area: Rect,
    content_length: usize,
    position: usize,
) {
    if area.width == 0 || area.height == 0 || content_length <= area.height as usize {
        return;
    }
    let mut state = ScrollbarState::new(content_length)
        .position(position)
        .viewport_content_length(area.height as usize);
    render_scrollbar(
        frame,
        PaneScrollbarSlot {
            area: Rect {
                x: area.right().saturating_sub(1),
                width: 1,
                ..area
            },
        },
        &mut state,
        &theme(),
    );
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum TuiActionId {
    Plan,
    Build,
    Deploy,
    Clean,
    ToggleServer,
    RestartServer,
    CleanRestartServer,
    FlipPanePlacement,
    ToggleServerSubview,
    RefreshAgents,
    OpenAgentDatasetFilter,
    OpenAgentColumns,
    IncludeAgentMatches,
    ExcludeAgentMatches,
    LoadMoreAgents,
    ToggleAgentAutoRefresh,
    CycleAgentMode,
    ToggleAgentDetails,
    CycleOpsView,
    CycleAgentFindScope,
    SelectDev,
    SelectOps,
    SelectOpsAgents,
    SelectOpsDeployment,
    SelectOpsEnvironment,
    SelectOpsServer,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TuiActionRisk {
    ReadOnly,
    RoutineWrite,
    Disruptive,
    Destructive,
}

impl TuiActionRisk {
    fn label(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::RoutineWrite => "routine-write",
            Self::Disruptive => "disruptive",
            Self::Destructive => "destructive",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TuiActionReview {
    None,
    ReviewForm,
}

impl TuiActionReview {
    fn label(self) -> &'static str {
        match self {
            Self::None => "no-review",
            Self::ReviewForm => "review-form",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TuiActionTarget {
    Interface,
    ApplicationEnvironment,
    LocalServer,
    LoadedAgents,
}

impl TuiActionTarget {
    fn label(self) -> &'static str {
        match self {
            Self::Interface => "interface",
            Self::ApplicationEnvironment => "application-environment",
            Self::LocalServer => "local-server",
            Self::LoadedAgents => "loaded-agents",
        }
    }
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

impl TuiAction {
    fn leader_path(self) -> Option<&'static str> {
        match self.id {
            TuiActionId::SelectDev | TuiActionId::SelectOps => Some("ctrl+x w"),
            TuiActionId::CycleOpsView => Some("ctrl+x v"),
            TuiActionId::FlipPanePlacement | TuiActionId::ToggleServerSubview => Some("ctrl+x l"),
            TuiActionId::OpenContextPicker => Some("ctrl+x e"),
            TuiActionId::OpenPalette | TuiActionId::ShowHelp | TuiActionId::Quit => None,
            _ => Some("ctrl+x a"),
        }
    }

    fn risk(self) -> TuiActionRisk {
        match self.id {
            TuiActionId::Clean | TuiActionId::CleanRestartServer => TuiActionRisk::Destructive,
            TuiActionId::Deploy | TuiActionId::RestartServer | TuiActionId::StopRepl => {
                TuiActionRisk::Disruptive
            }
            TuiActionId::Build
            | TuiActionId::ToggleServer
            | TuiActionId::StartOrFocusRepl
            | TuiActionId::RestartRepl => TuiActionRisk::RoutineWrite,
            _ => TuiActionRisk::ReadOnly,
        }
    }

    fn review(self) -> TuiActionReview {
        match self.id {
            TuiActionId::Plan
            | TuiActionId::Build
            | TuiActionId::Deploy
            | TuiActionId::Clean
            | TuiActionId::ToggleServer
            | TuiActionId::RestartServer
            | TuiActionId::CleanRestartServer
            | TuiActionId::StartOrFocusRepl
            | TuiActionId::RestartRepl => TuiActionReview::ReviewForm,
            _ => TuiActionReview::None,
        }
    }

    fn target(self) -> TuiActionTarget {
        match self.id {
            TuiActionId::Plan
            | TuiActionId::Build
            | TuiActionId::Deploy
            | TuiActionId::Clean
            | TuiActionId::StartOrFocusRepl
            | TuiActionId::RestartRepl => TuiActionTarget::ApplicationEnvironment,
            TuiActionId::ToggleServer
            | TuiActionId::RestartServer
            | TuiActionId::CleanRestartServer => TuiActionTarget::LocalServer,
            TuiActionId::RefreshAgents
            | TuiActionId::OpenAgentDatasetFilter
            | TuiActionId::OpenAgentColumns
            | TuiActionId::IncludeAgentMatches
            | TuiActionId::ExcludeAgentMatches
            | TuiActionId::LoadMoreAgents
            | TuiActionId::ToggleAgentAutoRefresh
            | TuiActionId::CycleAgentMode
            | TuiActionId::ToggleAgentDetails
            | TuiActionId::CycleAgentFindScope => TuiActionTarget::LoadedAgents,
            _ => TuiActionTarget::Interface,
        }
    }
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
    Plan,
    Build,
    Deploy,
    Clean,
    ToggleServer,
    RestartServer,
    CleanRestartServer,
    FlipPanePlacement,
    ToggleServerSubview,
    RefreshAgents,
    OpenAgentDatasetFilter,
    OpenAgentColumns,
    IncludeAgentMatches,
    ExcludeAgentMatches,
    LoadMoreAgents,
    ToggleAgentAutoRefresh,
    CycleAgentMode,
    ToggleAgentDetails,
    CycleOpsView,
    CycleAgentFindScope,
    SelectOpsRoot(OpsRootSubject),
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

const ACTIONS: [TuiAction; 35] = [
    TuiAction {
        id: TuiActionId::Plan,
        label: "Plan",
        description: "Review a golem deploy --plan command",
        shortcut: None,
        category: TuiActionCategory::Dev,
        scope: TuiActionScope::Global,
        execution_kind: TuiActionExecutionKind::NestedCli,
        palette_visible: true,
        kind: TuiActionKind::Plan,
    },
    TuiAction {
        id: TuiActionId::Build,
        label: "Build",
        description: "Run golem build",
        shortcut: None,
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
        shortcut: None,
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
        shortcut: None,
        category: TuiActionCategory::Dev,
        scope: TuiActionScope::Global,
        execution_kind: TuiActionExecutionKind::NestedCli,
        palette_visible: true,
        kind: TuiActionKind::Clean,
    },
    TuiAction {
        id: TuiActionId::ToggleServer,
        label: "Start/Stop Server",
        description: "Focus Dev Server and toggle it",
        shortcut: None,
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
        shortcut: None,
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
        shortcut: None,
        category: TuiActionCategory::Dev,
        scope: TuiActionScope::Leader,
        execution_kind: TuiActionExecutionKind::NestedCli,
        palette_visible: true,
        kind: TuiActionKind::CleanRestartServer,
    },
    TuiAction {
        id: TuiActionId::FlipPanePlacement,
        label: "Flip Pane Placement",
        description: "Place secondary panes on the right or bottom",
        shortcut: None,
        category: TuiActionCategory::Settings,
        scope: TuiActionScope::Leader,
        execution_kind: TuiActionExecutionKind::Internal,
        palette_visible: true,
        kind: TuiActionKind::FlipPanePlacement,
    },
    TuiAction {
        id: TuiActionId::ToggleServerSubview,
        label: "Toggle Server Subview",
        description: "Show or hide Server beside the primary Dev view",
        shortcut: None,
        category: TuiActionCategory::Dev,
        scope: TuiActionScope::Leader,
        execution_kind: TuiActionExecutionKind::Internal,
        palette_visible: true,
        kind: TuiActionKind::ToggleServerSubview,
    },
    TuiAction {
        id: TuiActionId::RefreshAgents,
        label: "Refresh Agents",
        description: "Refresh the Ops agent list",
        shortcut: Some("ctrl+r"),
        category: TuiActionCategory::Ops,
        scope: TuiActionScope::Agents,
        execution_kind: TuiActionExecutionKind::Direct,
        palette_visible: true,
        kind: TuiActionKind::RefreshAgents,
    },
    TuiAction {
        id: TuiActionId::OpenAgentDatasetFilter,
        label: "Filter Agent Dataset",
        description: "Choose a server-side component or agent-type filter",
        shortcut: Some("ctrl+f"),
        category: TuiActionCategory::Ops,
        scope: TuiActionScope::Agents,
        execution_kind: TuiActionExecutionKind::Internal,
        palette_visible: true,
        kind: TuiActionKind::OpenAgentDatasetFilter,
    },
    TuiAction {
        id: TuiActionId::OpenAgentColumns,
        label: "Configure Agent Columns",
        description: "Choose visible Agent collection columns",
        shortcut: None,
        category: TuiActionCategory::Ops,
        scope: TuiActionScope::Agents,
        execution_kind: TuiActionExecutionKind::Internal,
        palette_visible: true,
        kind: TuiActionKind::OpenAgentColumns,
    },
    TuiAction {
        id: TuiActionId::IncludeAgentMatches,
        label: "Include Loaded Matches",
        description: "Include every loaded agent matching the local find",
        shortcut: Some("ctrl+a"),
        category: TuiActionCategory::Ops,
        scope: TuiActionScope::Agents,
        execution_kind: TuiActionExecutionKind::Internal,
        palette_visible: true,
        kind: TuiActionKind::IncludeAgentMatches,
    },
    TuiAction {
        id: TuiActionId::ExcludeAgentMatches,
        label: "Exclude Loaded Matches",
        description: "Exclude every loaded agent matching the local find",
        shortcut: Some("ctrl+n"),
        category: TuiActionCategory::Ops,
        scope: TuiActionScope::Agents,
        execution_kind: TuiActionExecutionKind::Internal,
        palette_visible: true,
        kind: TuiActionKind::ExcludeAgentMatches,
    },
    TuiAction {
        id: TuiActionId::LoadMoreAgents,
        label: "Load Next 200 Agents",
        description: "Fetch one additional batch of up to 200 agents",
        shortcut: Some("ctrl+l"),
        category: TuiActionCategory::Ops,
        scope: TuiActionScope::Agents,
        execution_kind: TuiActionExecutionKind::Direct,
        palette_visible: true,
        kind: TuiActionKind::LoadMoreAgents,
    },
    TuiAction {
        id: TuiActionId::ToggleAgentAutoRefresh,
        label: "Toggle Agent Auto Refresh",
        description: "Toggle automatic agent refresh",
        shortcut: None,
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
        shortcut: None,
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
        shortcut: None,
        category: TuiActionCategory::Ops,
        scope: TuiActionScope::Agents,
        execution_kind: TuiActionExecutionKind::Internal,
        palette_visible: true,
        kind: TuiActionKind::ToggleAgentDetails,
    },
    TuiAction {
        id: TuiActionId::CycleOpsView,
        label: "Cycle Ops View",
        description: "Cycle Overview, Activity, and Metrics for the current subject",
        shortcut: None,
        category: TuiActionCategory::Navigation,
        scope: TuiActionScope::Leader,
        execution_kind: TuiActionExecutionKind::ViewNavigation,
        palette_visible: true,
        kind: TuiActionKind::CycleOpsView,
    },
    TuiAction {
        id: TuiActionId::CycleAgentFindScope,
        label: "Cycle Agent Find Field",
        description: "Search loaded agents by AgentID, component, or agent type",
        shortcut: None,
        category: TuiActionCategory::Ops,
        scope: TuiActionScope::Agents,
        execution_kind: TuiActionExecutionKind::Internal,
        palette_visible: true,
        kind: TuiActionKind::CycleAgentFindScope,
    },
    TuiAction {
        id: TuiActionId::SelectDev,
        label: "Go to Dev",
        description: "Switch to Dev workspace",
        shortcut: None,
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
        shortcut: None,
        category: TuiActionCategory::Navigation,
        scope: TuiActionScope::Global,
        execution_kind: TuiActionExecutionKind::ViewNavigation,
        palette_visible: true,
        kind: TuiActionKind::SelectView(TuiWorkspace::Ops),
    },
    TuiAction {
        id: TuiActionId::SelectOpsAgents,
        label: "Open Agents",
        description: "Open the Ops Agents collection",
        shortcut: None,
        category: TuiActionCategory::Ops,
        scope: TuiActionScope::Global,
        execution_kind: TuiActionExecutionKind::ViewNavigation,
        palette_visible: true,
        kind: TuiActionKind::SelectOpsRoot(OpsRootSubject::Agents),
    },
    TuiAction {
        id: TuiActionId::SelectOpsDeployment,
        label: "Open Deployment",
        description: "Open the current Deployment overview",
        shortcut: None,
        category: TuiActionCategory::Ops,
        scope: TuiActionScope::Global,
        execution_kind: TuiActionExecutionKind::ViewNavigation,
        palette_visible: true,
        kind: TuiActionKind::SelectOpsRoot(OpsRootSubject::Deployment),
    },
    TuiAction {
        id: TuiActionId::SelectOpsEnvironment,
        label: "Open Environment",
        description: "Open the selected Environment overview",
        shortcut: None,
        category: TuiActionCategory::Ops,
        scope: TuiActionScope::Global,
        execution_kind: TuiActionExecutionKind::ViewNavigation,
        palette_visible: true,
        kind: TuiActionKind::SelectOpsRoot(OpsRootSubject::Environment),
    },
    TuiAction {
        id: TuiActionId::SelectOpsServer,
        label: "Open Server",
        description: "Open the selected Server overview",
        shortcut: None,
        category: TuiActionCategory::Ops,
        scope: TuiActionScope::Global,
        execution_kind: TuiActionExecutionKind::ViewNavigation,
        palette_visible: true,
        kind: TuiActionKind::SelectOpsRoot(OpsRootSubject::Server),
    },
    TuiAction {
        id: TuiActionId::StartOrFocusRepl,
        label: "Start or Focus REPL",
        description: "Run golem repl and send input there",
        shortcut: None,
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
        shortcut: None,
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
        shortcut: None,
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
        shortcut: None,
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
        shortcut: None,
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
        shortcut: Some("ctrl+h"),
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
        shortcut: Some("ctrl+q"),
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

#[cfg(all(feature = "tui-preview", test))]
fn preview_production_app() -> TuiApp {
    let mut app = TuiApp {
        should_quit: false,
        active_workspace: TuiWorkspace::Ops,
        ops_subject: OpsSubject::Root(OpsRootSubject::Agents),
        ops_view: OpsView::Overview,
        dev_primary: DevView::Repl,
        dev_focus: DevView::Repl,
        mode: TuiMode::Normal,
        leader: LeaderState::default(),
        help_scroll: 0,
        help_content_height: Cell::new(0),
        help_viewport_height: Cell::new(0),
        palette: CommandPalette::default(),
        command_options: CommandOptions::default(),
        command_form: None,
        command_form_memory: HashMap::new(),
        session_form: None,
        server_form_memory: HashMap::new(),
        repl_form_memory: HashMap::new(),
        command_run: None,
        completed_jobs: VecDeque::new(),
        selected_job: 0,
        server: LocalServerService::default(),
        pending_server_restart: None,
        repl: ReplState::default(),
        pending_repl_restart: None,
        agents: AgentsState::default(),
        ops_environment: OpsEnvironmentState::default(),
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
        preferences_path: PathBuf::from("/tmp/golem-tui-preview-preferences.json"),
        preferences_error: None,
    };
    app.agents.agents = vec![
        AgentListItem {
            agent_id: "checkout(cart-17)".into(),
            component: Some("checkout".into()),
            agent_type: Some("CheckoutAgent".into()),
            revision: "7".into(),
            pending: "2".into(),
            created_at: "2026-09-23T12:00:00Z".into(),
            status: AgentStatus::Running,
            last_error_kind: None,
            raw: serde_json::json!({"name": "checkout(cart-17)"}),
        },
        AgentListItem {
            agent_id: "orders(order-04)".into(),
            component: Some("orders".into()),
            agent_type: Some("OrderAgent".into()),
            revision: "4".into(),
            pending: "0".into(),
            created_at: "2026-09-23T11:30:00Z".into(),
            status: AgentStatus::Idle,
            last_error_kind: None,
            raw: serde_json::json!({"name": "orders(order-04)"}),
        },
    ];
    app.agents.included.insert(app.agents.agents[0].identity());
    app
}

fn palette_actions(scope: PaletteScope, app: &TuiApp) -> Vec<TuiAction> {
    ACTIONS
        .iter()
        .copied()
        .filter(|action| action.palette_visible)
        .filter(|action| scope == PaletteScope::All || action_relevant_to_context(action, app))
        .collect()
}

fn action_relevant_to_context(action: &TuiAction, app: &TuiApp) -> bool {
    match app.active_workspace {
        TuiWorkspace::Dev => matches!(
            action.category,
            TuiActionCategory::Dev | TuiActionCategory::Repl | TuiActionCategory::Settings
        ),
        TuiWorkspace::Ops => {
            action.category == TuiActionCategory::Ops
                || matches!(action.id, TuiActionId::CycleOpsView)
        }
    }
}

fn filtered_actions(query: &str, scope: PaletteScope, app: &TuiApp) -> Vec<TuiAction> {
    let query = query.trim();
    if query.is_empty() {
        return palette_actions(scope, app);
    }

    let matcher = SkimMatcherV2::default();
    let mut matches = palette_actions(scope, app)
        .into_iter()
        .filter_map(|action| {
            let haystack = format!(
                "{} {} {} {} {} {} {} {} {}",
                action.label,
                action.description,
                action.category.label(),
                action.scope.label(),
                action.execution_kind.label(),
                action.risk().label(),
                action.review().label(),
                action.target().label(),
                action.leader_path().unwrap_or("")
            );
            matcher
                .fuzzy_match(&haystack, query)
                .map(|score| (score, action))
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
                .is_some_and(|line| line.starts_with('┌')),
            "{frame}"
        );
        assert!(frame.contains("GOLEM"), "{frame}");
        assert!(
            frame
                .lines()
                .next()
                .is_some_and(|line| line.contains("app sample-app · env local · server local")),
            "{frame}"
        );
        assert!(frame.contains("Agents · Overview"), "{frame}");
        assert!(!frame.contains(" Build "), "{frame}");
        assert!(!frame.contains(" Deploy "), "{frame}");
    }

    #[test]
    fn renders_active_tab() {
        let app = test_app();
        let frame = render_app_text(&app);

        assert!(frame.contains("[ Ops ]"), "{frame}");
        assert!(!frame.contains("Home"), "{frame}");
        assert!(frame.contains("( Dev )"), "{frame}");
    }

    #[test]
    fn dev_and_ops_use_the_same_context_only_header() {
        let ops = test_app();
        let ops_buffer = render_app_buffer_at(&ops, 120, 30);
        let ops_text = render_buffer_text(&ops_buffer);
        let ops_header = ops_text.lines().next().unwrap_or_default().to_string();

        let mut dev = test_app();
        dev.active_workspace = TuiWorkspace::Dev;
        dev.dev_primary = DevView::BuildDeploy;
        dev.dev_focus = DevView::BuildDeploy;
        dev.layout.dev_visible = [false, false, false];
        dev.command_run = Some(CommandRun::new(
            1,
            CommandKind::Build,
            vec!["build".to_string()],
            CommandOptions::default(),
            "sample-app:local".to_string(),
        ));
        dev.server.run.status = ServerStatus::Running;
        dev.repl.run.status = ReplStatus::Running;
        let dev_buffer = render_app_buffer_at(&dev, 120, 30);
        let dev_text = render_buffer_text(&dev_buffer);
        let dev_header = dev_text.lines().next().unwrap_or_default().to_string();

        assert_eq!(dev_header, ops_header);
        assert!(ops_header.starts_with("┌ GOLEM · app sample-app · env local · server local"));
        for excluded in [
            "[Dev]",
            "[Ops]",
            "job",
            "repl",
            "Build & Deploy",
            "Overview",
        ] {
            assert!(!ops_header.contains(excluded), "{ops_header}");
        }
        assert_eq!(ops_buffer[(2, 0)].fg, theme().accent);
        assert_eq!(ops_buffer[(2, 0)].bg, theme().surface);
    }

    #[test]
    fn compact_header_uses_the_locked_context_path() {
        let app = test_app();
        let frame = render_app_text_at(&app, 32, 12);
        let header = frame.lines().next().unwrap_or_default();

        assert!(
            header.starts_with("┌ GOLEM · sample-app/local/local"),
            "{frame}"
        );
        assert!(!header.contains("Ops"), "{frame}");
    }

    #[test]
    fn dev_panes_use_shared_titles_boundaries_and_status_colors() {
        let mut app = test_app();
        app.active_workspace = TuiWorkspace::Dev;
        app.dev_primary = DevView::BuildDeploy;
        app.dev_focus = DevView::BuildDeploy;
        app.layout.dev_visible = [false, true, false];
        app.command_run = Some(CommandRun::new(
            1,
            CommandKind::Build,
            vec!["build".to_string()],
            CommandOptions::default(),
            "sample-app:local".to_string(),
        ));

        let buffer = render_app_buffer_at(&app, 120, 30);
        let text = render_buffer_text(&buffer);
        let build_body = snapshot_region(&app, RegionKind::DevViewBody(DevView::BuildDeploy));
        let divider = snapshot_region(&app, RegionKind::DevPrimarySplit);

        assert!(text.contains("[ Build & Deploy ]"), "{text}");
        assert!(text.contains("( Server )"), "{text}");
        assert!(!text.contains("tab focus"), "{text}");
        assert_eq!(buffer[(2, 1)].fg, theme().accent_hover);
        assert_eq!(buffer[(build_body.x, build_body.y)].symbol(), "│");
        assert_eq!(
            buffer[(build_body.x, build_body.y)].fg,
            theme().border_subtle
        );
        assert_eq!(buffer[(build_body.x, build_body.y)].bg, theme().surface);
        assert_eq!(buffer[(build_body.x + 1, build_body.y)].symbol(), "●");
        assert_eq!(buffer[(build_body.x + 1, build_body.y)].fg, theme().success);
        assert_eq!(buffer[(build_body.x + 1, build_body.y)].bg, theme().surface);
        assert_eq!(buffer[(divider.x, divider.y + 1)].symbol(), "│");
        assert_eq!(buffer[(divider.x, divider.y + 1)].fg, theme().border_subtle);
        assert_eq!(buffer[(divider.x, divider.y + 1)].bg, theme().surface);
    }

    #[test]
    fn lifecycle_statuses_follow_shared_semantic_colors() {
        for status in [
            CommandStatus::Running,
            CommandStatus::Cancelling,
            CommandStatus::Succeeded,
        ] {
            assert_eq!(command_status_kind(status), StatusKind::Running);
        }
        for status in [
            ServerStatus::Starting,
            ServerStatus::Running,
            ServerStatus::Stopping,
        ] {
            assert_eq!(server_status_kind(status), StatusKind::Running);
        }
        for status in [
            ReplStatus::Starting,
            ReplStatus::Running,
            ReplStatus::Stopping,
        ] {
            assert_eq!(repl_status_kind(status), StatusKind::Running);
        }
        assert_eq!(server_status_kind(ServerStatus::Stopped), StatusKind::Idle);
        assert_eq!(repl_status_kind(ReplStatus::Failed), StatusKind::Failed);
        assert_eq!(
            command_status_kind(CommandStatus::Killed),
            StatusKind::Failed
        );
    }

    #[test]
    fn dev_shell_renders_all_recursive_layouts_at_supported_extremes() {
        for placement in [layout::PanePlacement::Right, layout::PanePlacement::Bottom] {
            for (width, height) in [(160, 40), (100, 16), (60, 24), (1, 1)] {
                let mut app = test_app();
                app.active_workspace = TuiWorkspace::Dev;
                app.dev_primary = DevView::BuildDeploy;
                app.dev_focus = DevView::BuildDeploy;
                app.layout.dev_visible = [false, true, true];
                app.layout.dev_placement = placement;

                let _ = render_app_text_at(&app, width, height);
            }
        }
    }

    #[test]
    fn recursive_pane_junctions_match_right_and_bottom_layouts() {
        for (placement, expected) in [
            (layout::PanePlacement::Right, "├"),
            (layout::PanePlacement::Bottom, "┬"),
        ] {
            let mut app = test_app();
            app.active_workspace = TuiWorkspace::Dev;
            app.dev_primary = DevView::BuildDeploy;
            app.dev_focus = DevView::BuildDeploy;
            app.layout.dev_visible = [false, true, true];
            app.layout.dev_placement = placement;

            let buffer = render_app_buffer_at(&app, 120, 30);
            let primary = snapshot_region(&app, RegionKind::DevPrimarySplit);
            let secondary = snapshot_region(&app, RegionKind::DevSecondarySplit);
            let (x, y) = match placement {
                layout::PanePlacement::Right => (primary.x, secondary.y),
                layout::PanePlacement::Bottom => (secondary.x, primary.y),
            };

            assert_eq!(buffer[(x, y)].symbol(), expected);
            assert_eq!(buffer[(x, y)].fg, theme().border_subtle);
            assert_eq!(buffer[(x, y)].bg, theme().surface);
        }
    }

    #[test]
    fn dev_terminal_scrollbars_use_the_shared_trailing_slot() {
        let mut app = test_app();
        app.active_workspace = TuiWorkspace::Dev;
        app.dev_primary = DevView::BuildDeploy;
        app.dev_focus = DevView::BuildDeploy;
        app.layout.dev_visible = [false, false, false];
        app.command_run = Some(CommandRun::new(
            1,
            CommandKind::Build,
            vec!["build".to_string()],
            CommandOptions::default(),
            "sample-app:local".to_string(),
        ));
        for index in 0..60 {
            app.append_command_output(format!("line {index}\r\n").as_bytes());
        }

        let buffer = render_app_buffer_at(&app, 80, 18);
        let body = snapshot_region(&app, RegionKind::DevViewBody(DevView::BuildDeploy));
        let scrollbar_x = body.right().saturating_sub(1);

        assert_eq!(buffer[(scrollbar_x, body.y + 1)].bg, theme().surface);
        assert!(
            [theme().text_muted, theme().text_faint]
                .contains(&buffer[(scrollbar_x, body.y + 1)].fg)
        );
        assert_ne!(buffer[(scrollbar_x, body.y + 1)].symbol(), "0");
    }

    #[test]
    fn legacy_dev_runtime_state_does_not_leak_into_ops_navigation() {
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
        assert!(!frame.contains("golem build"), "{frame}");
        assert!(!frame.contains("REPL"), "{frame}");
        assert!(frame.contains("[ Ops ]"), "{frame}");
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
        assert_eq!(
            buffer.cell((0, 1)).expect("missing title cell").symbol(),
            "├"
        );
        for y in 2..21 {
            let symbol = buffer.cell((0, y)).expect("missing cell").symbol();
            assert_eq!(symbol, "│", "missing rail at row {y}");
        }
    }

    #[test]
    fn leader_hint_renders_navigation_groups() {
        let mut app = test_app();

        app.handle_key(modified_key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        let frame = render_app_text(&app);

        assert_eq!(app.mode, TuiMode::Leader);
        assert!(frame.contains("Workspaces"), "{frame}");
        assert!(frame.contains("Views"), "{frame}");
        assert!(frame.contains("Layout"), "{frame}");
        assert!(frame.contains("Context"), "{frame}");
        assert!(frame.contains("Actions"), "{frame}");
        assert!(frame.contains("Jobs"), "{frame}");
    }

    #[test]
    fn leader_shortcut_switches_ops_views() {
        let mut app = test_app();

        app.handle_key(modified_key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        app.handle_key(key(KeyCode::Char('v')));
        app.handle_key(key(KeyCode::Char('m')));
        let frame = render_app_text(&app);
        assert_eq!(app.ops_view, OpsView::Metrics);
        assert!(frame.contains("Agents · Metrics"), "{frame}");

        app.handle_key(modified_key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        app.handle_key(key(KeyCode::Char('v')));
        app.handle_key(key(KeyCode::Char('o')));
        let frame = render_app_text(&app);
        assert_eq!(app.ops_view, OpsView::Overview);
        assert!(frame.contains("Agents · Overview"), "{frame}");
    }

    #[test]
    fn alt_number_is_not_a_workspace_shortcut() {
        let mut app = test_app();

        app.handle_key(modified_key(KeyCode::Char('1'), KeyModifiers::ALT));

        assert_eq!(app.active_workspace, TuiWorkspace::Ops);
        assert_eq!(app.mode, TuiMode::Normal);
    }

    #[test]
    fn bare_workspace_number_starts_agent_find() {
        let mut app = test_app();
        app.handle_key(key(KeyCode::Char('2')));

        assert_eq!(app.active_workspace, TuiWorkspace::Ops);
        assert_eq!(app.dev_focus, DevView::Repl);
        assert_eq!(app.mode, TuiMode::AgentFilter);
        assert_eq!(app.agents.query, "2");
    }

    #[test]
    fn opens_palette() {
        let mut app = test_app();

        app.handle_key(modified_key(KeyCode::Char('p'), KeyModifiers::CONTROL));
        let frame = render_app_text(&app);

        assert!(frame.contains("Commands"), "{frame}");
        assert!(frame.contains("Refresh Agents"), "{frame}");
    }

    #[test]
    fn adaptive_data_popups_use_available_space_and_keep_margins() {
        assert_eq!(
            adaptive_data_popup_rect(Rect::new(0, 0, 100, 40), 36, 10),
            Rect::new(7, 3, 85, 34)
        );
        assert_eq!(
            adaptive_data_popup_rect(Rect::new(0, 0, 40, 12), 48, 10),
            Rect::new(1, 1, 38, 10)
        );
        assert_eq!(
            adaptive_data_popup_rect(Rect::new(0, 0, 1, 1), 48, 12),
            Rect::new(0, 0, 1, 1)
        );

        assert_eq!(
            centered_rect_fixed(52, 9, Rect::new(0, 0, 100, 40)),
            Rect::new(24, 15, 52, 9)
        );
    }

    #[test]
    fn visual_preferences_round_trip_without_operational_state() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("tui/preferences.json");
        let mut app = test_app();
        app.layout.dev_placement = layout::PanePlacement::Bottom;
        app.layout.dev_primary_ratio = 63;
        app.layout.dev_secondary_ratio = 41;
        app.layout.ops_details_placement = layout::PanePlacement::Bottom;
        app.layout.ops_details_ratio = 57;
        app.agents
            .table
            .set_column_visible(&AGENT_COLUMNS, "pending", true);
        let expected = TuiPreferences::from_state(&app.layout, &app.agents.table);

        write_tui_preferences(&path, &expected).unwrap();
        let (loaded, error) = load_tui_preferences(&path);

        assert_eq!(loaded, Some(expected));
        assert!(error.is_none());
    }

    #[test]
    fn unsupported_preferences_version_is_not_applied() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("preferences.json");
        fs::write(
            &path,
            br#"{"version":999,"devPlacement":"right","devPrimaryRatio":50,"devSecondaryRatio":50,"opsDetailsPlacement":"right","opsDetailsRatio":50,"agentColumns":[]}"#,
        )
        .unwrap();

        let (loaded, error) = load_tui_preferences(&path);

        assert!(loaded.is_none());
        assert!(error.is_some_and(|error| error.contains("unsupported")));
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
    fn action_metadata_captures_path_risk_review_and_target() {
        let clean = *action(TuiActionId::Clean);
        assert_eq!(clean.leader_path(), Some("ctrl+x a"));
        assert_eq!(clean.risk(), TuiActionRisk::Destructive);
        assert_eq!(clean.review(), TuiActionReview::ReviewForm);
        assert_eq!(clean.target(), TuiActionTarget::ApplicationEnvironment);

        let refresh = *action(TuiActionId::RefreshAgents);
        assert_eq!(refresh.risk(), TuiActionRisk::ReadOnly);
        assert_eq!(refresh.review(), TuiActionReview::None);
        assert_eq!(refresh.target(), TuiActionTarget::LoadedAgents);
    }

    #[test]
    fn remote_dev_mutations_are_discoverable_but_unavailable() {
        let mut app = test_app();
        app.context.uses_local_server = false;

        assert_eq!(
            app.action_availability(action(TuiActionId::Build)),
            TuiActionAvailability::Unavailable(
                "remote Dev mutations are not enabled in this slice"
            )
        );
        assert!(
            filtered_actions("build", PaletteScope::All, &app)
                .iter()
                .any(|action| action.id == TuiActionId::Build)
        );
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
    fn action_shortcuts_never_use_bare_printable_keys() {
        for action in ACTIONS {
            if let Some(shortcut) = action.shortcut {
                assert!(
                    shortcut.contains("ctrl+") || shortcut.contains("alt+"),
                    "shortcut must include a modifier: {shortcut}"
                );
            }
        }
    }

    #[test]
    fn refresh_agents_action_metadata_is_direct() {
        let app = test_app();
        assert_eq!(
            action(TuiActionId::RefreshAgents).execution_kind,
            TuiActionExecutionKind::Direct
        );
        assert!(
            filtered_actions("nested-cli", PaletteScope::All, &app)
                .iter()
                .all(|action| action.id != TuiActionId::RefreshAgents)
        );
        assert!(
            filtered_actions("direct", PaletteScope::All, &app)
                .iter()
                .any(|action| action.id == TuiActionId::RefreshAgents)
        );
    }

    #[test]
    fn palette_uses_visible_actions_and_category_search() {
        let app = test_app();
        assert!(
            filtered_actions("", PaletteScope::All, &app)
                .iter()
                .all(|action| action.palette_visible)
        );
        assert!(
            filtered_actions("", PaletteScope::All, &app)
                .iter()
                .all(|action| action.id != TuiActionId::OpenPalette)
        );

        let ops_actions = filtered_actions("ops", PaletteScope::All, &app);
        assert!(
            ops_actions
                .iter()
                .any(|action| action.id == TuiActionId::RefreshAgents),
            "{ops_actions:?}"
        );
    }

    #[test]
    fn footer_exposes_only_the_shared_shell_shortcuts() {
        let app = test_app();
        let footer = render_app_text_at(&app, 120, 30);

        assert!(footer.contains("ctrl+x Menu"), "{footer}");
        assert!(footer.contains("ctrl+p All actions"), "{footer}");
        assert!(footer.contains("ctrl+h Help"), "{footer}");
        assert!(!footer.contains("ctrl+x b"), "{footer}");
    }

    #[test]
    fn resized_production_footer_shows_palette_shortcut_once() {
        let app = test_app();

        for width in [48, 72, 100, 140] {
            let frame = render_app_text_at(&app, width, 24);
            assert_eq!(
                frame.matches("ctrl+p").count(),
                1,
                "duplicate palette shortcut at width {width}:\n{frame}"
            );
        }
    }

    #[test]
    fn leader_hints_use_registered_actions() {
        let mut app = test_app();
        let driver = TuiTestDriver::new(120, 32);

        app.handle_key(modified_key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        driver.assert_visible(&app, "Context");
        driver.assert_visible(&app, "Actions");

        app.handle_key(key(KeyCode::Char('a')));
        driver.assert_visible(&app, "Toggle Agent Auto Refresh");
        driver.assert_visible(&app, "Cycle Agent Mode");
        driver.assert_visible(&app, "Configure Agent Columns");
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
        assert!(frame.contains("▌ sample-app/local"), "{frame}");
        assert!(frame.contains("current"), "{frame}");
        assert!(!frame.contains("[manifest"), "{frame}");
        assert!(frame.contains("sample-app/prod"), "{frame}");
        assert!(frame.contains("Profile prod"), "{frame}");
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
        assert!(frame.contains("▌ sample-app/prod"), "{frame}");
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
        assert!(frame.contains("esc Cancel"), "{frame}");
        assert!(frame.contains("ctrl+q Quit"), "{frame}");
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
    fn context_environment_loading_ctrl_q_quits() {
        let mut app = test_app();
        app.open_context_picker();
        app.context_switcher.environment_list_running = true;

        app.handle_key(modified_key(KeyCode::Char('q'), KeyModifiers::CONTROL));

        assert!(app.should_quit);
    }

    #[test]
    fn context_picker_rows_include_targets_and_app_environments() {
        let mut state = ContextSwitcherState::new(vec![
            test_current_context_target(),
            test_server_context_target(
                "server:profile:prod",
                "Profile production with long descriptive name",
                "https://very-long-production-worker-service.internal.example.com:9443",
            ),
        ]);
        let rows = context_picker_rows(&state);
        assert!(rows.iter().any(|row| matches!(row,
            ContextPickerRenderRow::Item { label, .. } if label.contains("production")
        )));

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
        let rows = context_picker_rows(&state);
        assert!(rows.iter().any(|row| matches!(row,
            ContextPickerRenderRow::Item { label, .. } if label == "large-application/prod"
        )));
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
    fn non_local_context_does_not_expose_server_shortcut() {
        let mut app = test_app();
        app.context.uses_local_server = false;
        app.context.dev_eligible = false;

        assert_eq!(
            app.action_availability(action(TuiActionId::ToggleServer)),
            TuiActionAvailability::Unavailable("requires an available local server target")
        );

        app.handle_key(key(KeyCode::Char('s')));

        assert_eq!(app.server.run.status, ServerStatus::Stopped);
        assert_eq!(app.active_workspace, TuiWorkspace::Ops);
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
            TuiActionAvailability::Unavailable("requires a manifest-backed application")
        );

        app.handle_key(key(KeyCode::Char('b')));

        assert_eq!(app.active_workspace, TuiWorkspace::Ops);
        assert!(app.command_run.is_none());
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
    fn agent_filter_defaults_to_case_insensitive_agent_id_substrings() {
        let mut app = test_app();
        app.agents.agents = sample_agents();

        assert_eq!(app.agents.query_scope, AgentQueryScope::AgentId);
        app.mode = TuiMode::AgentFilter;
        for character in "CART".chars() {
            app.handle_key(key(KeyCode::Char(character)));
        }
        assert_eq!(
            app.filtered_agents()
                .iter()
                .map(|agent| agent.agent_id.as_str())
                .collect::<Vec<_>>(),
            vec!["cart-1", "cart-2"]
        );

        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.agents.selected, 1);
        app.handle_key(key(KeyCode::Up));
        assert_eq!(app.agents.selected, 0);
    }

    #[test]
    fn agent_filter_is_literal_scoped_and_preserves_loaded_order() {
        let mut app = test_app();
        app.agents.agents = sample_agents();

        app.agents.query = "ct1".to_string();
        assert!(app.filtered_agents().is_empty());

        app.agents.query = "agent".to_string();
        assert!(app.filtered_agents().is_empty());
        app.agents.query_scope = AgentQueryScope::AgentType;
        assert_eq!(
            app.filtered_agents()
                .iter()
                .map(|agent| agent.agent_id.as_str())
                .collect::<Vec<_>>(),
            vec!["cart-1", "cart-2", "order-1"]
        );

        app.agents.query_scope = AgentQueryScope::Component;
        app.agents.query = "cart".to_string();
        assert_eq!(
            app.filtered_agents()
                .iter()
                .map(|agent| agent.agent_id.as_str())
                .collect::<Vec<_>>(),
            vec!["cart-1", "cart-2"]
        );
    }

    #[test]
    fn agent_find_scopes_loaded_rows_and_bulk_selection_preserves_hidden_agents() {
        let mut app = test_app();
        app.agents.agents = sample_agents();
        app.agents.included = app
            .agents
            .agents
            .iter()
            .map(AgentListItem::identity)
            .collect();
        app.agents.query = "cart".to_string();
        app.agents.query_scope = AgentQueryScope::Component;

        assert_eq!(app.filtered_agents().len(), 2);
        app.exclude_filtered_agents();

        assert_eq!(app.agents.included.len(), 1);
        assert!(app.agents.included.contains(&AgentIdentity {
            component: "orders".to_string(),
            agent_id: "order-1".to_string(),
        }));

        app.include_filtered_agents();
        assert_eq!(app.agents.included.len(), 3);
    }

    #[test]
    fn agent_dataset_picker_uses_loaded_component_and_type_choices() {
        let mut app = test_app();
        app.agents.agents = sample_agents();

        app.handle_key(modified_key(KeyCode::Char('f'), KeyModifiers::CONTROL));
        let frame = render_app_text_at(&app, 100, 30);

        assert_eq!(app.mode, TuiMode::AgentDatasetFilter);
        assert!(frame.contains("Agent dataset"), "{frame}");
        assert!(
            frame.contains("Server-side filter; applied before loading agents"),
            "{frame}"
        );
        assert!(frame.contains("cart"), "{frame}");
        assert!(frame.contains("CartAgent"), "{frame}");
        assert!(frame.contains('│'), "{frame}");
        assert!(!frame.contains("cartCartAgent"), "{frame}");
        assert!(frame.contains("↑/↓ Navigate"), "{frame}");
    }

    #[test]
    fn agent_dataset_picker_literal_searches_components_and_agent_types() {
        let mut app = test_app();
        app.agents.agents = sample_agents();
        app.handle_key(modified_key(KeyCode::Char('f'), KeyModifiers::CONTROL));

        for character in "oRdErAgEnT".chars() {
            app.handle_key(key(KeyCode::Char(character)));
        }
        let filter = app.agents.dataset_filter.as_ref().expect("dataset filter");
        assert_eq!(filter.filtered_indices().len(), 1);
        assert!(matches!(
            filter.selected_choice(),
            Some(AgentDatasetScope::AgentType { agent_type, .. }) if agent_type == "OrderAgent"
        ));
        let frame = render_app_text_at(&app, 100, 30);
        assert!(frame.contains("1/5 choices"), "{frame}");
        assert!(frame.contains("OrderAgent"), "{frame}");

        app.handle_key(modified_key(KeyCode::Char('u'), KeyModifiers::CONTROL));
        for character in "odrar".chars() {
            app.handle_key(key(KeyCode::Char(character)));
        }
        assert!(
            app.agents
                .dataset_filter
                .as_ref()
                .expect("dataset filter")
                .filtered_indices()
                .is_empty()
        );
        let frame = render_app_text_at(&app, 100, 30);
        assert!(
            frame.contains("No component or agent type matches"),
            "{frame}"
        );

        app.handle_key(modified_key(KeyCode::Char('u'), KeyModifiers::CONTROL));
        for character in "OrderAgent".chars() {
            app.handle_key(key(KeyCode::Char(character)));
        }
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(
            app.agents.dataset_scope,
            AgentDatasetScope::AgentType {
                component: "orders".to_string(),
                agent_type: "OrderAgent".to_string(),
            }
        );
    }

    #[test]
    fn agent_columns_offer_live_operational_fields_and_navigation_hint() {
        let mut app = test_app();
        app.agents.agents = sample_agents();

        app.execute_action(TuiActionKind::OpenAgentColumns, None);
        let frame = render_app_text_at(&app, 100, 30);

        assert_eq!(app.mode, TuiMode::AgentColumns);
        assert!(frame.contains("Revision"), "{frame}");
        assert!(frame.contains("Pending"), "{frame}");
        assert!(frame.contains("Created at"), "{frame}");
        assert!(frame.contains("↑/↓ Navigate"), "{frame}");
    }

    #[test]
    fn agent_table_panning_uses_the_rendered_list_width() {
        let mut app = test_app();
        app.agents.agents = sample_agents();

        render_app_text_at(&app, 50, 24);
        let narrow_width = app.agents.table_viewport_width.get();
        assert!(narrow_width > 0);
        assert_ne!(narrow_width, 72);
        app.handle_key(key(KeyCode::Right));
        assert_eq!(app.agents.table.horizontal_offset, 1);

        render_app_text_at(&app, 200, 24);
        assert!(app.agents.table_viewport_width.get() > narrow_width);
        app.handle_key(key(KeyCode::Right));
        assert_eq!(app.agents.table.horizontal_offset, 0);
    }

    #[test]
    fn resetting_agent_dataset_invalidates_an_in_flight_refresh() {
        let mut app = test_app();
        let (tx, _rx) = test_event_channel();
        app.agents.refresh_generation = 4;
        app.agents.refresh_context_id = Some(TuiContextId::new(1));
        app.agents.refresh_running = true;

        app.reset_agent_dataset();
        assert_eq!(app.agents.refresh_generation, 5);
        assert!(!app.agents.refresh_running);

        app.handle_event(
            TuiEvent::AgentRefreshFinished {
                generation: 4,
                append: false,
                result: agent_refresh_success(
                    1,
                    sample_agents_metadata_response(vec![sample_agent_metadata_view(
                        "old-dataset",
                        "OldAgent(\"stale\")",
                        AgentStatus::Running,
                    )]),
                ),
            },
            &tx,
        );

        assert!(app.agents.agents.is_empty());
    }

    #[test]
    fn details_focus_scroll_and_split_resize_are_keyboard_accessible() {
        let mut app = test_app();
        app.agents.agents = sample_agents();
        app.agents.agents[0].raw = serde_json::json!({
            "values": (0..100).collect::<Vec<_>>()
        });
        render_app_text_at(&app, 120, 30);

        app.handle_key(key(KeyCode::Tab));
        app.handle_key(key(KeyCode::PageDown));
        assert_eq!(app.agents.focus, AgentOverviewFocus::Details);
        assert!(app.agents.details_scroll > 0);

        let old_ratio = app.layout.ops_details_ratio;
        app.handle_key(modified_key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        app.handle_key(key(KeyCode::Char('l')));
        app.handle_key(key(KeyCode::Right));
        assert!(app.layout.ops_details_ratio > old_ratio);
    }

    #[test]
    fn details_focus_and_resize_shortcuts_are_visible_and_work_from_find() {
        let mut app = test_app();
        app.agents.agents = sample_agents();
        render_app_text_at(&app, 120, 30);

        app.handle_key(key(KeyCode::Char('c')));
        assert_eq!(app.mode, TuiMode::AgentFilter);

        let old_ratio = app.layout.ops_details_ratio;
        app.handle_key(modified_key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        app.handle_key(key(KeyCode::Char('l')));
        app.handle_key(key(KeyCode::Right));
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.mode, TuiMode::AgentFilter);
        assert!(app.layout.ops_details_ratio > old_ratio);

        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.mode, TuiMode::Normal);
        assert_eq!(app.agents.focus, AgentOverviewFocus::Details);
        assert!(app.agents.detail_visible);

        let frame = render_app_text_at(&app, 120, 30);
        assert!(frame.contains("tab / shift+tab Focus pane"), "{frame}");
        assert!(frame.contains("ctrl+x l Layout"), "{frame}");

        app.handle_key(key(KeyCode::BackTab));
        assert_eq!(app.agents.focus, AgentOverviewFocus::List);
    }

    #[test]
    fn control_arrows_are_not_consumed_for_pane_focus() {
        let mut app = test_app();
        app.agents.agents = sample_agents();
        render_app_text_at(&app, 120, 30);

        app.handle_key(modified_key(KeyCode::Right, KeyModifiers::CONTROL));

        assert_eq!(app.agents.focus, AgentOverviewFocus::List);
        assert_eq!(app.mode, TuiMode::Normal);
    }

    #[test]
    fn leader_shortcuts_remain_available_while_find_is_editing() {
        let mut app = test_app();
        app.agents.agents = sample_agents();
        app.handle_key(key(KeyCode::Char('c')));
        assert_eq!(app.mode, TuiMode::AgentFilter);

        app.handle_key(modified_key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        app.handle_key(key(KeyCode::Char('a')));

        assert_eq!(app.mode, TuiMode::Palette);
        assert_eq!(app.palette.scope, PaletteScope::Context);
        assert_eq!(app.agents.query, "c");
    }

    #[test]
    fn modified_agent_shortcuts_remain_available_while_find_is_editing() {
        let mut app = test_app();
        app.agents.agents = sample_agents();

        app.handle_key(key(KeyCode::Char('c')));
        assert_eq!(app.mode, TuiMode::AgentFilter);
        assert_eq!(app.agents.query, "c");

        let selected = app.filtered_agents()[app.agents.selected].identity();
        app.handle_key(modified_key(KeyCode::Char(' '), KeyModifiers::CONTROL));
        assert!(app.agents.included.contains(&selected));
        assert_eq!(app.mode, TuiMode::AgentFilter);

        app.handle_key(modified_key(KeyCode::Char('p'), KeyModifiers::CONTROL));
        assert_eq!(app.mode, TuiMode::Palette);
    }

    #[test]
    fn agent_details_show_agent_metadata_before_agent_type_metadata() {
        let agents = sample_agents();
        let mut agent_types = BTreeMap::new();
        agent_types.insert(
            ("cart".to_string(), "CartAgent".to_string()),
            serde_json::json!({"type": "CartAgent"}),
        );

        let text = build_ops_agent_details_lines(agents.first(), 0, &agent_types, None, 80)
            .iter()
            .map(|line| {
                line.spans.iter().fold(String::new(), |mut text, span| {
                    text.push_str(&span.content);
                    text
                })
            })
            .collect::<Vec<_>>()
            .join("\n");

        let agent_metadata = text.find("Agent metadata").expect("agent metadata heading");
        let agent_type_metadata = text
            .find("Agent type metadata")
            .expect("agent type metadata heading");
        assert!(agent_metadata < agent_type_metadata, "{text}");
    }

    #[test]
    fn agent_id_list_and_detail_spans_share_semantic_highlighting() {
        let id = r#"Cart("ann", 42, true)"#;
        let cell = agent_id_table_cell(id);
        let detail = agent_id_line_spans(id);

        assert_eq!(
            cell.spans
                .iter()
                .map(|span| span.text.as_str())
                .collect::<String>(),
            id
        );
        assert!(
            cell.spans
                .iter()
                .any(|span| { span.text == r#""ann""# && span.tone == Some(CellTone::Success) })
        );
        assert!(
            cell.spans
                .iter()
                .any(|span| span.text == "42" && span.tone == Some(CellTone::Info))
        );
        assert!(
            cell.spans
                .iter()
                .any(|span| span.text == "true" && span.tone == Some(CellTone::Warning))
        );
        assert_eq!(
            detail.iter().fold(String::new(), |mut text, span| {
                text.push_str(&span.content);
                text
            }),
            id
        );
        assert!(
            detail
                .iter()
                .any(|span| span.style.fg == Some(theme().success))
        );
        assert!(
            detail
                .iter()
                .any(|span| span.style.fg == Some(theme().info))
        );
        assert!(
            detail
                .iter()
                .any(|span| span.style.fg == Some(theme().marker))
        );
    }

    #[test]
    fn agent_find_exposes_only_explicit_scopes() {
        let mut app = test_app();
        app.agents.agents = sample_agents();
        app.mode = TuiMode::AgentFilter;

        for (scope, label) in [
            (AgentQueryScope::AgentId, "AgentID"),
            (AgentQueryScope::Component, "Component"),
            (AgentQueryScope::AgentType, "Agent type"),
        ] {
            app.agents.query_scope = scope;
            let frame = render_app_text_at(&app, 120, 30);
            assert!(frame.contains(label), "missing scope {label}:\n{frame}");
            assert!(!frame.contains("All fields"), "{frame}");
        }
    }

    #[test]
    fn production_shell_exposes_dev_and_ops_workspaces() {
        let mut app = test_app();
        app.agents.agents = sample_agents();

        let frame = render_app_text_at(&app, 120, 30);

        assert!(frame.contains("[ Ops ]"), "{frame}");
        assert!(frame.contains("Agents · Overview"), "{frame}");
        assert!(
            frame.contains("app sample-app · env local · server local"),
            "{frame}"
        );
        assert!(frame.contains("( Dev )"), "{frame}");
        assert!(!frame.contains("Home"), "{frame}");
    }

    #[test]
    fn production_metrics_view_is_truthfully_unavailable() {
        let mut app = test_app();

        app.handle_key(modified_key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        app.handle_key(key(KeyCode::Char('v')));
        app.handle_key(key(KeyCode::Char('m')));
        let frame = render_app_text_at(&app, 120, 28);

        assert_eq!(app.ops_view, OpsView::Metrics);
        assert!(frame.contains("Agents · Metrics"), "{frame}");
        assert!(frame.contains("No production Metrics provider"), "{frame}");
        assert!(!frame.contains("golem.agent.invocations"), "{frame}");
    }

    #[test]
    fn unavailable_metrics_view_does_not_invent_focusable_series() {
        let mut app = test_app();
        app.ops_view = OpsView::Metrics;
        render_app_text_at(&app, 120, 16);

        app.handle_key(key(KeyCode::Down));

        assert_eq!(app.ops_view, OpsView::Metrics);
        let frame = render_app_text_at(&app, 120, 16);
        assert!(!frame.contains("golem.agent.invocations"), "{frame}");
    }

    #[test]
    fn explicit_agent_selection_survives_filter_changes() {
        let mut app = test_app();
        app.agents.agents = sample_agents();

        app.handle_key(modified_key(KeyCode::Char(' '), KeyModifiers::CONTROL));
        assert!(app.agents.included.contains(&AgentIdentity {
            component: "cart".to_string(),
            agent_id: "cart-1".to_string(),
        }));

        app.mode = TuiMode::AgentFilter;
        for character in "order".chars() {
            app.handle_key(key(KeyCode::Char(character)));
        }
        app.handle_key(key(KeyCode::Enter));
        app.handle_key(modified_key(KeyCode::Char(' '), KeyModifiers::CONTROL));

        assert_eq!(app.agents.included.len(), 2);
        assert!(app.agents.included.contains(&AgentIdentity {
            component: "orders".to_string(),
            agent_id: "order-1".to_string(),
        }));
    }

    #[test]
    fn column_chooser_applies_only_on_confirmation() {
        let mut app = test_app();

        app.execute_action(TuiActionKind::OpenAgentColumns, None);
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Char(' ')));
        app.handle_key(key(KeyCode::Esc));
        assert!(app.agents.table.column_visible("type"));

        app.execute_action(TuiActionKind::OpenAgentColumns, None);
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Char(' ')));
        app.handle_key(key(KeyCode::Enter));
        assert!(!app.agents.table.column_visible("type"));
    }

    #[test]
    fn failed_refresh_keeps_stale_agents_visible() {
        let mut app = test_app();
        app.agents.agents = sample_agents();
        app.agents.last_error = Some("server unavailable".to_string());

        let frame = render_app_text_at(&app, 120, 30);

        assert!(
            frame.contains("showing the last successful result"),
            "{frame}"
        );
        assert!(frame.contains("cart-1"), "{frame}");
    }

    #[test]
    fn ops_shell_renders_at_supported_extremes() {
        let app = test_app();

        for (width, height) in [(160, 40), (100, 8), (32, 24), (12, 4), (1, 1)] {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                render_app_text_at(&app, width, height)
            }));
            assert!(result.is_ok(), "Ops shell panicked at {width}x{height}");
        }
    }

    #[test]
    fn agent_mode_cycles_and_preserves_filter() {
        let mut app = test_app();
        app.active_workspace = TuiWorkspace::Ops;
        app.agents.query = "cart".to_string();
        app.agents.selected = 3;

        app.execute_action(TuiActionKind::CycleAgentMode, None);

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
        app.handle_key(key(KeyCode::Char('l')));
        app.handle_key(key(KeyCode::Char('d')));
        app.handle_key(key(KeyCode::Enter));
        let frame = render_app_text(&app);
        assert!(!frame.contains("( Details )"), "{frame}");
    }

    #[test]
    fn enter_opens_agent_subject_and_escape_unwinds() {
        let mut app = test_app();
        app.active_workspace = TuiWorkspace::Ops;
        app.agents.agents = sample_agents();

        app.handle_key(key(KeyCode::Enter));
        assert!(matches!(app.ops_subject, OpsSubject::Agent(_)));

        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.ops_subject, OpsSubject::Root(OpsRootSubject::Agents));
    }

    #[test]
    fn narrow_agent_view_keeps_keyboard_focus_on_the_visible_list() {
        let mut app = test_app();
        app.active_workspace = TuiWorkspace::Ops;
        app.agents.agents = sample_agents();
        app.agents.focus = AgentOverviewFocus::Details;

        render_app_text_at(&app, 60, 24);

        assert!(!app.agent_details_open());
        assert!(app.agent_list_focused());
    }

    #[test]
    fn typed_agent_metadata_maps_to_agent_list_item() {
        let response = sample_agents_metadata_response(vec![sample_agent_metadata_view(
            "cart",
            "CartAgent(\"cart-1\")",
            golem_common::model::AgentStatus::Running,
        )]);

        let items = agent_items_from_metadata_response(response);

        assert_eq!(items[0].agent_id, "CartAgent(\"cart-1\")");
        assert_eq!(items[0].component.as_deref(), Some("cart"));
        assert_eq!(items[0].agent_type.as_deref(), Some("CartAgent"));
        assert_eq!(items[0].revision, "1");
        assert_eq!(items[0].pending, "0");
        assert_eq!(items[0].created_at, "2024-01-01T00:00:00.000Z");
        assert_eq!(items[0].status, AgentStatus::Running);
        assert_eq!(items[0].raw["agentId"], "CartAgent(\"cart-1\")");
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
                append: false,
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
        assert_eq!(app.agents.agents[0].agent_id, "cart-1");
    }

    #[test]
    fn agent_refresh_append_deduplicates_rows_and_advances_page_depth() {
        let mut app = test_app();
        let (tx, _rx) = test_event_channel();
        app.agents.agents = vec![agent_item_from_metadata(sample_agent_metadata_view(
            "cart",
            "cart-1",
            AgentStatus::Idle,
        ))];
        app.agents.refresh_generation = 2;
        app.agents.refresh_context_id = Some(TuiContextId::new(1));
        app.agents.refresh_running = true;

        app.handle_event(
            TuiEvent::AgentRefreshFinished {
                generation: 2,
                append: true,
                result: agent_refresh_success(
                    1,
                    sample_agents_metadata_response(vec![
                        sample_agent_metadata_view("cart", "cart-1", AgentStatus::Idle),
                        sample_agent_metadata_view("cart", "cart-2", AgentStatus::Running),
                    ]),
                ),
            },
            &tx,
        );

        assert_eq!(app.agents.agents.len(), 2);
        assert_eq!(app.agents.paging.loaded_depth(), 2);
        assert!(!app.agents.has_more());
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
                append: false,
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
    fn stale_ops_environment_refresh_is_ignored() {
        let mut app = test_app();
        app.ops_environment.generation = 2;
        app.ops_environment.context_id = Some(TuiContextId::new(7));
        app.ops_environment.loading = true;
        app.ops_environment.snapshot = Some(sample_ops_environment_snapshot("current"));

        app.finish_ops_environment_refresh(
            1,
            TuiContextTaskResult::new(
                TuiContextId::new(7),
                Ok(sample_ops_environment_snapshot("stale")),
                Vec::new(),
            ),
        );

        assert!(app.ops_environment.loading);
        assert_eq!(
            app.ops_environment
                .snapshot
                .as_ref()
                .expect("retained snapshot")
                .environment_revision,
            "current"
        );
    }

    #[test]
    fn failed_ops_environment_refresh_keeps_last_successful_snapshot() {
        let mut app = test_app();
        app.ops_environment.generation = 3;
        app.ops_environment.context_id = Some(TuiContextId::new(7));
        app.ops_environment.loading = true;
        app.ops_environment.snapshot = Some(sample_ops_environment_snapshot("current"));

        app.finish_ops_environment_refresh(
            3,
            TuiContextTaskResult::new(
                TuiContextId::new(7),
                Err("\u{1b}[31mserver unavailable\u{1b}[0m".to_string()),
                vec!["\u{1b}[33mcaptured detail\u{1b}[0m".to_string()],
            ),
        );

        assert!(!app.ops_environment.loading);
        assert_eq!(
            app.ops_environment
                .snapshot
                .as_ref()
                .expect("retained snapshot")
                .environment_revision,
            "current"
        );
        let error = app.ops_environment.error.as_deref().expect("refresh error");
        assert!(error.contains("server unavailable"), "{error}");
        assert!(error.contains("captured detail"), "{error}");
        assert!(!error.contains('\u{1b}'), "{error:?}");
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
                append: false,
                result: TuiContextTaskResult::new(
                    TuiContextId::new(1),
                    Err("\u{1b}[31mrefresh failed\u{1b}[0m".to_string()),
                    vec!["\u{1b}[33mcaptured log\u{1b}[0m".to_string()],
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

        assert!(frame.contains("› agent"), "{frame}");
        assert!(frame.contains("Refresh Agents"), "{frame}");
    }

    #[test]
    fn palette_scope_switch_preserves_query() {
        let mut app = test_app();
        app.open_palette(PaletteScope::All);
        for character in "agent".chars() {
            app.handle_palette_key(key(KeyCode::Char(character)), None);
        }

        app.handle_palette_key(
            modified_key(KeyCode::Char('x'), KeyModifiers::CONTROL),
            None,
        );
        app.handle_leader_key(key(KeyCode::Char('a')), None);

        assert_eq!(app.mode, TuiMode::Palette);
        assert_eq!(app.palette.scope, PaletteScope::Context);
        assert_eq!(app.palette.query, "agent");

        app.handle_palette_key(
            modified_key(KeyCode::Char('p'), KeyModifiers::CONTROL),
            None,
        );
        assert_eq!(app.palette.scope, PaletteScope::All);
        assert_eq!(app.palette.query, "agent");
    }

    #[test]
    fn executes_palette_view_action() {
        let mut app = test_app();

        app.handle_key(modified_key(KeyCode::Char('p'), KeyModifiers::CONTROL));
        for character in "cycle ops view".chars() {
            app.handle_key(key(KeyCode::Char(character)));
        }
        app.handle_key(key(KeyCode::Enter));

        let frame = render_app_text(&app);
        assert_eq!(app.ops_view, OpsView::Activity);
        assert!(frame.contains("Agents · Activity"), "{frame}");
        assert!(frame.contains("not implemented yet"), "{frame}");
        assert!(!frame.contains("[ Commands ]"), "{frame}");
    }

    #[test]
    fn opens_help() {
        let mut app = test_app();

        app.handle_key(modified_key(KeyCode::Char('h'), KeyModifiers::CONTROL));
        let frame = render_app_text(&app);

        assert!(frame.contains("Keyboard Shortcuts"), "{frame}");
        assert!(frame.contains("ctrl+p"), "{frame}");
    }

    #[test]
    fn help_scrolls_with_keyboard_and_mouse_while_its_footer_stays_visible() {
        let mut app = test_app();
        app.open_help();
        let frame = render_app_text_at(&app, 80, 16);
        assert!(app.help_content_height.get() > app.help_viewport_height.get());
        assert!(frame.contains("esc Close"), "{frame}");

        app.handle_key(key(KeyCode::PageDown));
        let after_key = app.help_scroll;
        assert!(after_key > 0);
        app.handle_mouse(mouse(MouseEventKind::ScrollDown, 40, 8), None);
        assert!(app.help_scroll >= after_key);

        let frame = render_app_text_at(&app, 80, 16);
        assert!(frame.contains("esc Close"), "{frame}");
    }

    #[test]
    fn global_help_explains_the_shell_and_current_ops_actions() {
        let mut app = test_app();
        let mut driver = TuiTestDriver::new(120, 48);

        driver.key(
            &mut app,
            modified_key(KeyCode::Char('h'), KeyModifiers::CONTROL),
        );

        driver.assert_visible(&app, action(TuiActionId::RefreshAgents).label);
        driver.assert_visible(&app, "Workspaces");
        driver.assert_visible(&app, action(TuiActionId::ShowHelp).label);
        driver.key(&mut app, key(KeyCode::End));
        driver.assert_visible(&app, "Columns and collection actions");
    }

    #[test]
    fn agent_help_is_reachable_and_contextual() {
        let mut app = test_app();
        let mut driver = TuiTestDriver::new(120, 48);

        driver.key(
            &mut app,
            modified_key(KeyCode::Char('h'), KeyModifiers::CONTROL),
        );

        assert_eq!(app.mode, TuiMode::Help);
        driver.assert_visible(&app, "Agents Overview");
        driver.key(&mut app, key(KeyCode::End));
        driver.assert_visible(&app, "left / right");
        driver.assert_visible(&app, "Include or exclude focused agent");
    }

    #[test]
    fn legacy_repl_key_does_not_change_help_context() {
        let mut app = test_app();
        let mut driver = TuiTestDriver::new(120, 48);

        driver.key(&mut app, key(KeyCode::Char('r')));
        assert_eq!(app.mode, TuiMode::AgentFilter);
        assert_eq!(app.agents.query, "r");
        driver.key(&mut app, key(KeyCode::Esc));
        driver.key(
            &mut app,
            modified_key(KeyCode::Char('h'), KeyModifiers::CONTROL),
        );

        assert_eq!(app.mode, TuiMode::Help);
        driver.assert_visible(&app, "Agents Overview");
    }

    #[test]
    fn legacy_build_key_does_not_change_help_context() {
        let mut app = test_app();
        let mut driver = TuiTestDriver::new(120, 48);

        driver.key(&mut app, key(KeyCode::Char('b')));
        assert_eq!(app.mode, TuiMode::AgentFilter);
        assert_eq!(app.agents.query, "b");
        driver.key(&mut app, key(KeyCode::Esc));
        driver.key(
            &mut app,
            modified_key(KeyCode::Char('h'), KeyModifiers::CONTROL),
        );

        assert_eq!(app.mode, TuiMode::Help);
        driver.assert_visible(&app, "Agents Overview");
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
        let frame = render_app_text_at(&app, 160, 30);
        assert!(frame.contains("context executor unavailable"), "{frame}");

        app.handle_key(key(KeyCode::Enter));

        assert_eq!(app.mode, TuiMode::Palette);
        assert!(!app.agents.refresh_running);
        assert!(app.agents.last_error.is_none());
    }

    #[test]
    fn closes_help_with_escape() {
        let mut app = test_app();

        app.handle_key(modified_key(KeyCode::Char('h'), KeyModifiers::CONTROL));
        app.handle_key(key(KeyCode::Esc));
        let frame = render_app_text(&app);

        assert!(!frame.contains("Keyboard Shortcuts"), "{frame}");
        assert!(frame.contains("[ Ops ]"), "{frame}");
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
        assert!(!frame.contains("[ Commands ]"), "{frame}");
    }

    #[test]
    fn legacy_command_option_shortcuts_are_not_exposed() {
        let mut app = test_app();

        app.handle_key(modified_key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        app.handle_key(key(KeyCode::Char('y')));
        app.handle_key(modified_key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        app.handle_key(key(KeyCode::Char('r')));

        assert!(!app.command_options.yes);
        assert!(!app.command_options.reset);
    }

    #[test]
    fn production_footer_omits_legacy_command_flags() {
        let app = test_app();

        let frame = render_app_text(&app);
        assert!(!frame.contains("yes:"), "{frame}");
        assert!(!frame.contains("reset:"), "{frame}");
    }

    #[test]
    fn bare_build_key_starts_agent_find_in_ops_first_shell() {
        let mut app = test_app();

        app.handle_key(key(KeyCode::Char('b')));

        assert!(app.command_run.is_none());
        assert_eq!(app.active_workspace, TuiWorkspace::Ops);
        assert_eq!(app.mode, TuiMode::AgentFilter);
        assert_eq!(app.agents.query, "b");
    }

    #[test]
    fn bare_clean_key_starts_agent_find_in_ops_first_shell() {
        let mut app = test_app();

        app.handle_key(key(KeyCode::Char('c')));

        assert!(app.command_run.is_none());
        assert_eq!(app.active_workspace, TuiWorkspace::Ops);
        assert_eq!(app.mode, TuiMode::AgentFilter);
        assert_eq!(app.agents.query, "c");
    }

    #[test]
    fn clean_palette_action_opens_a_review_form() {
        let mut app = test_app();

        app.handle_key(modified_key(KeyCode::Char('p'), KeyModifiers::CONTROL));
        for character in "clean".chars() {
            app.handle_key(key(KeyCode::Char(character)));
        }
        app.handle_key(key(KeyCode::Enter));

        assert!(app.command_run.is_none());
        assert_eq!(app.mode, TuiMode::CommandForm);
        assert!(matches!(
            app.command_form,
            Some(CommandReviewForm {
                kind: CommandKind::Clean,
                ..
            })
        ));
    }

    #[test]
    fn production_shell_omits_legacy_command_status() {
        let mut app = test_app();

        app.command_run = Some(CommandRun::new(
            1,
            CommandKind::Build,
            vec!["build".to_string()],
            CommandOptions::default(),
            "sample-app:local".to_string(),
        ));
        let frame = render_app_text(&app);

        assert!(!frame.contains("golem build"), "{frame}");
    }

    #[test]
    fn output_input_row_is_hidden_when_idle() {
        let mut app = test_app();
        app.active_workspace = TuiWorkspace::Dev;

        let frame = render_app_text(&app);
        assert!(!frame.contains("stdin"), "{frame}");
    }

    #[test]
    fn production_shell_omits_legacy_command_input_row() {
        let mut app = test_app();

        app.handle_key(key(KeyCode::Char('b')));
        let frame = render_app_text(&app);
        assert!(!frame.contains("stdin"), "{frame}");

        app.handle_key(modified_key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        app.handle_key(key(KeyCode::Char('y')));
        let frame = render_app_text(&app);
        assert!(!frame.contains("stdin"), "{frame}");
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
    fn legacy_build_key_does_not_take_terminal_cursor() {
        let mut app = test_app();

        app.handle_key(key(KeyCode::Char('b')));
        let (_, cursor) = render_app_text_and_cursor(&app);

        assert_eq!((cursor.x, cursor.y), (0, 0));
    }

    #[test]
    fn nested_terminal_autofollows_and_scrolls() {
        let mut output = TerminalScreen::new(3, 40);

        for index in 0..20 {
            output.feed(format!("line {index}\r\n").as_bytes());
        }
        assert_eq!(output.scrollback(), 0);
        assert!(output.plain_text().contains("line 19"));

        output.scroll_up(5);
        assert_eq!(output.scrollback(), 5);

        output.scroll_bottom();
        assert_eq!(output.scrollback(), 0);
    }

    #[test]
    fn nested_terminal_top_scroll_fills_viewport() {
        let mut output = TerminalScreen::new(5, 40);

        for index in 0..20 {
            output.feed(format!("line {index}\r\n").as_bytes());
        }
        output.scroll_top();

        let lines = output.lines();
        let text = lines.iter().map(ToString::to_string).collect::<String>();
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
    fn output_scroll_helpers_remain_available_for_future_dev_views() {
        let mut app = test_app();
        app.active_workspace = TuiWorkspace::Dev;
        app.dev_focus = DevView::BuildDeploy;
        app.command_run = Some(CommandRun::new(
            1,
            CommandKind::Build,
            vec!["build".to_string()],
            CommandOptions::default(),
            "sample-app:local".to_string(),
        ));
        app.set_command_status(CommandStatus::Succeeded);
        for index in 0..40 {
            app.append_command_output(format!("line {index}\r\n").as_bytes());
        }

        app.scroll_output_up();
        assert_eq!(
            app.command_run
                .as_ref()
                .map(|run| run.terminal.scrollback()),
            Some(10)
        );

        app.scroll_output_up_by(3);
        assert_eq!(
            app.command_run
                .as_ref()
                .map(|run| run.terminal.scrollback()),
            Some(13)
        );

        app.scroll_output_down_by(3);
        assert_eq!(
            app.command_run
                .as_ref()
                .map(|run| run.terminal.scrollback()),
            Some(10)
        );
    }

    #[test]
    fn command_page_up_enters_explicit_scrollback_and_escape_returns_live() {
        let mut app = test_app();
        app.active_workspace = TuiWorkspace::Dev;
        app.dev_primary = DevView::BuildDeploy;
        app.dev_focus = DevView::BuildDeploy;
        app.command_run = Some(CommandRun::new(
            1,
            CommandKind::Build,
            vec!["build".to_string()],
            CommandOptions::default(),
            "sample-app:local".to_string(),
        ));
        for index in 0..30 {
            app.append_command_output(format!("line {index}\r\n").as_bytes());
        }
        app.mode = TuiMode::CommandInteraction;

        app.handle_command_interaction_key(key(KeyCode::PageUp));
        assert_eq!(app.mode, TuiMode::TerminalScrollback);
        assert!(app.command_run.as_ref().unwrap().terminal.scrollback() > 0);

        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.mode, TuiMode::CommandInteraction);
        assert_eq!(app.command_run.as_ref().unwrap().terminal.scrollback(), 0);
    }

    #[test]
    fn esc_cancels_then_force_kills_running_command() {
        let mut app = test_app();

        app.start_command(CommandKind::Build, None);
        app.handle_command_interaction_key(key(KeyCode::Esc));
        app.command_run.as_mut().unwrap().stop_requested_at =
            Some(Instant::now() - FORCE_STOP_DELAY);
        assert_eq!(
            app.command_run.as_ref().map(|run| run.status),
            Some(CommandStatus::Cancelling)
        );

        app.handle_command_interaction_key(key(KeyCode::Esc));
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

        app.start_command(CommandKind::Build, None);
        assert_eq!(
            app.command_run.as_ref().map(|run| run.spinner_frame),
            Some(0)
        );

        app.handle_event(TuiEvent::SpinnerTick(1), &tx);
        assert_eq!(
            app.command_run.as_ref().map(|run| run.spinner_frame),
            Some(1)
        );
        assert_eq!(app.active_workspace, TuiWorkspace::Dev);
    }

    #[test]
    fn agent_refresh_spinner_is_generation_scoped_and_visible() {
        let mut app = test_app();
        let (tx, _rx) = test_event_channel();
        app.agents.refresh_running = true;
        app.agents.refresh_generation = 7;

        app.handle_event(TuiEvent::AgentRefreshSpinnerTick { generation: 6 }, &tx);
        assert_eq!(app.agents.refresh_spinner_frame, 0);
        app.handle_event(TuiEvent::AgentRefreshSpinnerTick { generation: 7 }, &tx);
        assert_eq!(app.agents.refresh_spinner_frame, 1);

        let frame = render_app_text_at(&app, 120, 30);
        assert!(frame.contains("[\\ refreshing]"), "{frame}");
        assert!(frame.contains("[\\ Loading] Refreshing agents"), "{frame}");
    }

    #[test]
    fn full_tick_channel_is_not_treated_as_closed() {
        let (tx, _rx) = mpsc::channel(1);
        tx.try_send(TuiEvent::SpinnerTick(1)).expect("first tick");

        assert!(!event_channel_closed(tx.try_send(TuiEvent::SpinnerTick(2))));
    }

    #[test]
    fn dev_server_is_a_normal_view_not_a_global_drawer() {
        let mut app = test_app();

        app.open_dev_workspace(DevView::Server);
        let frame = render_app_text(&app);

        assert_eq!(app.active_workspace, TuiWorkspace::Dev);
        assert!(frame.contains("( Ops )"), "{frame}");
        assert!(frame.contains("Server"), "{frame}");
        assert_eq!(
            frame.matches("○ stopped · session server").count(),
            1,
            "{frame}"
        );
    }

    #[test]
    fn server_form_clean_is_applied_to_the_reviewed_command() {
        let app = test_app();
        let mut values = ServerFormValues::new(&app.context.config_dir);
        values.clean = true;
        let args = server_args_from_values(&values);

        assert_eq!(
            args,
            vec![
                "server",
                "run",
                "--ports-file",
                "/tmp/golem-config/tui-server-ports.json",
                "--clean"
            ]
        );
    }

    #[test]
    fn reviewed_dev_commands_build_exact_nested_cli_arguments() {
        let app = test_app();
        let build = CommandFormValues {
            build_steps: [true, false, true, false],
            skip_check: true,
            force_build: true,
            yes: true,
            ..CommandFormValues::default()
        };
        assert_eq!(
            app.command_args(CommandKind::Build, &build),
            vec![
                "build",
                "--step",
                "check",
                "--step",
                "add-metadata",
                "--force-build",
                "--yes",
            ]
        );

        let plan = CommandFormValues {
            force_build: true,
            full_diff: true,
            rollback_version: " 7 ".to_string(),
            rollback_revision: "ignored".to_string(),
            post_deploy: PostDeployPolicy::Reset,
            yes: true,
            ..CommandFormValues::default()
        };
        assert_eq!(
            app.command_args(CommandKind::Plan, &plan),
            vec![
                "deploy",
                "--plan",
                "--force-build",
                "--full-diff",
                "--version",
                "7",
                "--reset",
                "--yes",
            ]
        );

        let deploy = CommandFormValues {
            rollback_revision: " 11 ".to_string(),
            post_deploy: PostDeployPolicy::UpdateManual,
            ..CommandFormValues::default()
        };
        assert_eq!(
            app.command_args(CommandKind::Deploy, &deploy),
            vec!["deploy", "--revision", "11", "--update-agents", "manual",]
        );
        assert_eq!(
            app.command_args(
                CommandKind::Clean,
                &CommandFormValues {
                    yes: true,
                    ..CommandFormValues::default()
                }
            ),
            vec!["clean", "--yes"]
        );
    }

    #[test]
    fn reviewed_server_and_repl_build_exact_nested_cli_arguments() {
        let server = ServerFormValues {
            system_memory_override: " 2GiB ".to_string(),
            router_address: " 127.0.0.2 ".to_string(),
            router_port: "9010".to_string(),
            custom_request_port: "9011".to_string(),
            mcp_port: "9012".to_string(),
            ports_file: " ./ports.json ".to_string(),
            data_dir: " ./data ".to_string(),
            agent_filesystem_root: " ./agents ".to_string(),
            clean: true,
            yes: true,
        };
        assert_eq!(
            server_args_from_values(&server),
            vec![
                "server",
                "run",
                "--system-memory-override",
                "2GiB",
                "--router-addr",
                "127.0.0.2",
                "--router-port",
                "9010",
                "--custom-request-port",
                "9011",
                "--mcp-port",
                "9012",
                "--ports-file",
                "./ports.json",
                "--data-dir",
                "./data",
                "--agent-filesystem-root",
                "./agents",
                "--clean",
                "--yes",
            ]
        );

        let repl = ReplFormValues {
            language: " javascript ".to_string(),
            stream_logs: false,
            auto_imports: false,
            post_deploy: PostDeployPolicy::Redeploy,
            yes: true,
        };
        assert_eq!(
            repl_args_from_values(&repl),
            vec![
                "repl",
                "--language",
                "javascript",
                "--disable-stream",
                "--disable-auto-imports",
                "--redeploy-agents",
                "--yes",
            ]
        );
    }

    #[test]
    fn reviewed_form_memory_is_context_scoped_and_yes_is_one_shot() {
        let mut app = test_app();
        app.command_form = Some(CommandReviewForm::new(
            CommandKind::Deploy,
            CommandFormValues {
                full_diff: true,
                post_deploy: PostDeployPolicy::Reset,
                yes: true,
                ..CommandFormValues::default()
            },
        ));
        if let Some(form) = app.command_form.as_mut() {
            form.values.yes = true;
        }
        app.launch_command_form(None);
        app.open_command_form(CommandKind::Deploy);
        let remembered = &app.command_form.as_ref().expect("command form").values;
        assert!(remembered.full_diff);
        assert_eq!(remembered.post_deploy, PostDeployPolicy::Reset);
        assert!(!remembered.yes);

        app.context.environment = "other".to_string();
        app.open_command_form(CommandKind::Deploy);
        let other_context = &app
            .command_form
            .as_ref()
            .expect("other context form")
            .values;
        assert!(!other_context.full_diff);
        assert_eq!(other_context.post_deploy, PostDeployPolicy::None);
        assert!(!other_context.yes);
    }

    #[test]
    fn reviewed_session_forms_remember_values_but_not_yes() {
        let mut server_app = test_app();
        let mut server_values = ServerFormValues::new(&server_app.context.config_dir);
        server_values.data_dir = "./data".to_string();
        server_values.clean = true;
        server_values.yes = true;
        server_app.session_form = Some(SessionReviewForm::Server {
            values: server_values,
            selected: 10,
            editing: false,
            restart: false,
            error: None,
        });
        server_app.launch_session_form(None);
        server_app.open_server_form(false, false);
        let SessionReviewForm::Server { values, .. } = server_app
            .session_form
            .as_ref()
            .expect("remembered server form")
        else {
            panic!("expected server form");
        };
        assert_eq!(values.data_dir, "./data");
        assert!(values.clean);
        assert!(!values.yes);

        let mut repl_app = test_app();
        repl_app.session_form = Some(SessionReviewForm::Repl {
            values: ReplFormValues {
                language: "javascript".to_string(),
                stream_logs: false,
                auto_imports: false,
                post_deploy: PostDeployPolicy::Reset,
                yes: true,
            },
            selected: 5,
            restart: false,
        });
        repl_app.launch_session_form(None);
        repl_app.open_repl_form(false);
        let SessionReviewForm::Repl { values, .. } = repl_app
            .session_form
            .as_ref()
            .expect("remembered repl form")
        else {
            panic!("expected repl form");
        };
        assert_eq!(values.language, "javascript");
        assert!(!values.stream_logs);
        assert!(!values.auto_imports);
        assert_eq!(values.post_deploy, PostDeployPolicy::Reset);
        assert!(!values.yes);
    }

    #[test]
    fn server_start_and_restart_always_open_review_forms() {
        let mut app = test_app();
        app.request_server_toggle();
        assert_eq!(app.server.run.status, ServerStatus::Stopped);
        assert_eq!(app.mode, TuiMode::SessionForm);
        assert!(matches!(
            app.session_form,
            Some(SessionReviewForm::Server { restart: false, .. })
        ));

        app.server.run.status = ServerStatus::Running;
        app.open_server_form(true, true);
        assert_eq!(app.server.run.status, ServerStatus::Running);
        assert!(matches!(
            app.session_form,
            Some(SessionReviewForm::Server {
                restart: true,
                values: ServerFormValues { clean: true, .. },
                ..
            })
        ));
    }

    #[test]
    fn server_shortcut_is_not_exposed_in_ops_first_shell() {
        let mut app = test_app();

        app.handle_key(key(KeyCode::Char('s')));
        assert_eq!(app.active_workspace, TuiWorkspace::Ops);
        assert_eq!(app.server.run.status, ServerStatus::Stopped);

        app.server.run.status = ServerStatus::Running;
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.server.run.status, ServerStatus::Running);
    }

    #[test]
    fn server_palette_action_switches_to_server_and_opens_review() {
        let mut app = test_app();

        app.execute_action(TuiActionKind::ToggleServer, None);

        assert_eq!(app.active_workspace, TuiWorkspace::Dev);
        assert_eq!(app.dev_focus, DevView::Server);
        assert_eq!(app.server.run.status, ServerStatus::Stopped);
        assert_eq!(app.mode, TuiMode::SessionForm);
    }

    #[test]
    fn server_logs_are_separate_from_command_output_buffers() {
        let mut app = test_app();

        app.start_command(CommandKind::Build, None);
        app.append_command_output(b"command log\n");
        app.server.run.terminal.feed(b"server log\n");

        let command = app.command_run.as_ref().unwrap().terminal.plain_text();
        let server = app.server.run.terminal.plain_text();
        assert!(command.contains("command log"));
        assert!(!command.contains("server log"));
        assert!(server.contains("server log"));
        assert!(!server.contains("command log"));
    }

    #[test]
    fn server_mouse_scrolls_logs() {
        let mut app = test_app();
        app.open_dev_workspace(DevView::Server);
        for index in 0..40 {
            app.server
                .run
                .terminal
                .feed(format!("server line {index}\r\n").as_bytes());
        }
        render_app_text(&app);

        app.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::ScrollUp,
                column: 10,
                row: 10,
                modifiers: KeyModifiers::empty(),
            },
            None,
        );

        assert_eq!(app.server.run.terminal.scrollback(), 3);
        assert_eq!(app.mode, TuiMode::TerminalScrollback);
    }

    #[test]
    fn production_layout_omits_legacy_dev_panel_regions() {
        let app = test_app();
        render_app_text_at(&app, 120, 32);
        assert!(
            app.layout_snapshot
                .borrow()
                .as_ref()
                .unwrap()
                .region(RegionKind::DevViewBody(DevView::BuildDeploy))
                .is_none()
        );
    }

    #[test]
    fn production_layout_exposes_both_workspace_selectors() {
        let app = test_app();
        render_app_text_at(&app, 120, 32);
        let snapshot = app.layout_snapshot.borrow();
        let snapshot = snapshot.as_ref().unwrap();
        assert!(
            snapshot
                .region(RegionKind::Workspace(TuiWorkspace::Ops))
                .is_some()
        );
        assert!(
            snapshot
                .region(RegionKind::Workspace(TuiWorkspace::Dev))
                .is_some()
        );
    }

    #[test]
    fn production_layout_does_not_expose_dev_panel_hit_targets() {
        let app = test_app();
        render_app_text_at(&app, 120, 32);
        assert!(
            app.layout_snapshot
                .borrow()
                .as_ref()
                .unwrap()
                .region(RegionKind::DevViewBody(DevView::BuildDeploy))
                .is_none()
        );
    }

    #[test]
    fn mouse_click_agent_row_selects_agent() {
        let mut app = test_app();
        app.active_workspace = TuiWorkspace::Ops;
        app.agents.agents = sample_agents();
        render_app_text_at(&app, 120, 32);
        let list = snapshot_region(&app, RegionKind::OpsList);

        app.handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), list.x + 4, list.y),
            None,
        );
        assert_eq!(app.agents.selected, 0);

        app.handle_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                list.x + 4,
                list.y + 3,
            ),
            None,
        );

        assert_eq!(app.agents.selected, 1);
    }

    #[test]
    fn mouse_click_keeps_the_visible_agent_viewport_anchored() {
        let mut app = test_app();
        let template = sample_agents()[0].clone();
        app.agents.agents = (0..40)
            .map(|index| {
                let mut agent = template.clone();
                agent.agent_id = format!("cart-{index:02}");
                agent
            })
            .collect();
        render_app_text_at(&app, 120, 16);
        app.scroll_agent_list(14, false);
        render_app_text_at(&app, 120, 16);
        let first = app.agents.table_first_visible_row.get();
        assert!(first > 0);
        let list = snapshot_region(&app, RegionKind::OpsList);

        app.handle_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                list.x + 4,
                list.y + 3,
            ),
            None,
        );
        render_app_text_at(&app, 120, 16);

        assert_eq!(app.agents.selected, first + 1);
        assert_eq!(app.agents.table_first_visible_row.get(), first);
    }

    #[test]
    fn mouse_wheel_moves_the_agent_list_selection() {
        let mut app = test_app();
        app.active_workspace = TuiWorkspace::Ops;
        app.agents.agents = sample_agents();
        render_app_text_at(&app, 120, 32);
        let list = snapshot_region(&app, RegionKind::OpsList);

        app.handle_mouse(
            mouse(MouseEventKind::ScrollDown, list.x + 4, list.y + 4),
            None,
        );
        assert_eq!(app.agents.selected, 2);
        assert_eq!(app.agents.focus, AgentOverviewFocus::List);

        app.handle_mouse(
            mouse(MouseEventKind::ScrollUp, list.x + 4, list.y + 4),
            None,
        );
        assert_eq!(app.agents.selected, 0);
    }

    #[test]
    fn continuation_is_requested_explicitly_not_by_navigation() {
        let mut app = test_app();
        let (tx, _rx) = test_event_channel();
        app.active_workspace = TuiWorkspace::Ops;
        app.agents.agents = sample_agents();
        app.agents.paging.finish_request(
            false,
            BTreeMap::from([("cart".to_string(), Some("0/42".parse().unwrap()))]),
        );
        render_app_text_at(&app, 120, 32);
        let list = snapshot_region(&app, RegionKind::OpsList);

        app.handle_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                list.x + 4,
                list.y + 4,
            ),
            Some(&tx),
        );

        assert_eq!(app.agents.selected, 2);
        assert!(app.agents.last_error.is_none());

        app.handle_key_with_events(key(KeyCode::End), Some(&tx));
        assert!(app.agents.last_error.is_none());
        app.handle_mouse(
            mouse(MouseEventKind::ScrollDown, list.x + 4, list.y + 4),
            Some(&tx),
        );
        assert!(app.agents.last_error.is_none());

        app.handle_key_with_events(
            modified_key(KeyCode::Char('l'), KeyModifiers::CONTROL),
            Some(&tx),
        );
        assert_eq!(
            app.agents.last_error.as_deref(),
            Some("TUI context executor is not available")
        );
    }

    #[test]
    fn production_layout_does_not_expose_legacy_nested_inspect_panes() {
        let app = test_app();
        render_app_text_at(&app, 120, 32);
        assert!(app.ops_agents_focused());
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
    fn production_layout_omits_dev_resize_split() {
        let app = test_app();
        render_app_text_at(&app, 120, 32);
        assert!(
            app.layout_snapshot
                .borrow()
                .as_ref()
                .unwrap()
                .region(RegionKind::DevPrimarySplit)
                .is_none()
        );
    }

    #[test]
    fn production_layout_exposes_ops_details_resize_split() {
        let mut app = test_app();
        app.agents.detail_visible = true;
        render_app_text_at(&app, 120, 32);
        assert!(
            app.layout_snapshot
                .borrow()
                .as_ref()
                .unwrap()
                .region(RegionKind::OpsDetailsSplit)
                .is_some()
        );
    }

    #[test]
    fn bare_repl_key_starts_agent_find_in_ops_first_shell() {
        let mut app = test_app();

        app.handle_key(key(KeyCode::Char('r')));

        assert_eq!(app.active_workspace, TuiWorkspace::Ops);
        assert_eq!(app.dev_focus, DevView::Repl);
        assert_eq!(app.mode, TuiMode::AgentFilter);
        assert_eq!(app.agents.query, "r");
        assert_eq!(app.repl.run.status, ReplStatus::Stopped);

        let frame = render_app_text(&app);
        assert!(!frame.contains("golem repl"), "{frame}");
    }

    #[test]
    fn inactive_repl_output_is_not_rendered_in_ops_shell() {
        let mut app = test_app();
        let (tx, _rx) = test_event_channel();

        app.handle_key(key(KeyCode::Char('r')));
        app.handle_event(TuiEvent::ReplOutput(b"hello\x1b[2DXY".to_vec()), &tx);

        let frame = render_app_text(&app);
        assert!(!frame.contains("helXY"), "{frame}");
    }

    #[test]
    fn inactive_repl_does_not_take_terminal_cursor() {
        let mut app = test_app();
        let (tx, _rx) = test_event_channel();

        app.handle_key(key(KeyCode::Char('r')));
        app.handle_event(TuiEvent::ReplOutput(b"abc".to_vec()), &tx);
        let (_, cursor) = render_app_text_and_cursor(&app);

        assert_eq!((cursor.x, cursor.y), (0, 0));
    }

    #[test]
    fn repl_leader_actions_are_not_exposed() {
        let mut app = test_app();

        app.handle_key(modified_key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        assert_eq!(app.mode, TuiMode::Leader);

        app.handle_key(key(KeyCode::Char('q')));
        assert_eq!(app.mode, TuiMode::Normal);
        assert_eq!(app.repl.run.status, ReplStatus::Stopped);

        app.handle_key(modified_key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        app.handle_key(key(KeyCode::Char('k')));
        assert_eq!(app.mode, TuiMode::Normal);
        assert_eq!(app.repl.run.status, ReplStatus::Stopped);
    }

    fn test_app() -> TuiApp {
        TuiApp {
            should_quit: false,
            active_workspace: TuiWorkspace::Ops,
            ops_subject: OpsSubject::Root(OpsRootSubject::Agents),
            ops_view: OpsView::Overview,
            dev_primary: DevView::Repl,
            dev_focus: DevView::Repl,
            mode: TuiMode::Normal,
            leader: LeaderState::default(),
            help_scroll: 0,
            help_content_height: Cell::new(0),
            help_viewport_height: Cell::new(0),
            palette: CommandPalette::default(),
            command_options: CommandOptions::default(),
            command_form: None,
            command_form_memory: HashMap::new(),
            session_form: None,
            server_form_memory: HashMap::new(),
            repl_form_memory: HashMap::new(),
            command_run: None,
            completed_jobs: VecDeque::new(),
            selected_job: 0,
            server: LocalServerService::default(),
            pending_server_restart: None,
            repl: ReplState::default(),
            pending_repl_restart: None,
            agents: AgentsState::default(),
            ops_environment: OpsEnvironmentState::default(),
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
            preferences_path: PathBuf::from("/tmp/golem-tui-test-preferences.json"),
            preferences_error: None,
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
        Config::add_profile(
            ProfileName("prod".to_string()),
            Profile {
                custom_url: Some(Url::parse("http://localhost:9882").expect("profile url")),
                custom_worker_url: None,
                allow_insecure: true,
                config: Default::default(),
                auth: AuthenticationConfig::static_builtin_local(),
            },
            false,
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
    ) -> TuiContextTaskResult<AgentRefreshPayload> {
        TuiContextTaskResult::new(
            TuiContextId::new(context_id),
            Ok(AgentRefreshPayload {
                agents: response,
                cursor: AgentListPageCursor::default(),
                agent_types: None,
            }),
            Vec::new(),
        )
    }

    fn sample_ops_environment_snapshot(environment_revision: &str) -> OpsEnvironmentSnapshot {
        OpsEnvironmentSnapshot {
            account_id: "account-1".to_string(),
            application_id: "application-1".to_string(),
            application_name: "sample-app".to_string(),
            environment_id: "environment-1".to_string(),
            environment_name: "local".to_string(),
            environment_revision: environment_revision.to_string(),
            diff_model_version: 1,
            compatibility_check: true,
            tool_compatibility_mode: "None".to_string(),
            version_check: true,
            security_overrides: false,
            deployment: None,
        }
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
            agent_id: crate::model::agent::RawAgentId(agent_name.to_string()),
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
            last_error_kind: None,
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
                tool_compatibility_mode: Default::default(),
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
                agent_id: "cart-1".to_string(),
                component: Some("cart".to_string()),
                agent_type: Some("CartAgent".to_string()),
                revision: "1".to_string(),
                pending: "0".to_string(),
                created_at: "2024-01-01T00:00:00Z".to_string(),
                status: AgentStatus::Running,
                last_error_kind: None,
                raw: serde_json::json!({"name":"cart-1"}),
            },
            AgentListItem {
                agent_id: "cart-2".to_string(),
                component: Some("cart".to_string()),
                agent_type: Some("CartAgent".to_string()),
                revision: "1".to_string(),
                pending: "0".to_string(),
                created_at: "2024-01-01T00:00:00Z".to_string(),
                status: AgentStatus::Idle,
                last_error_kind: None,
                raw: serde_json::json!({"name":"cart-2"}),
            },
            AgentListItem {
                agent_id: "order-1".to_string(),
                component: Some("orders".to_string()),
                agent_type: Some("OrderAgent".to_string()),
                revision: "1".to_string(),
                pending: "0".to_string(),
                created_at: "2024-01-01T00:00:00Z".to_string(),
                status: AgentStatus::Running,
                last_error_kind: None,
                raw: serde_json::json!({"name":"order-1"}),
            },
        ]
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
        render_buffer_text(&render_app_buffer_at(app, width, height))
    }

    fn render_app_buffer_at(app: &TuiApp, width: u16, height: u16) -> Buffer {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| render(frame, app)).unwrap();

        terminal.backend().buffer().clone()
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
