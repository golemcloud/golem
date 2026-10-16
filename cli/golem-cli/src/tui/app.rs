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

#[cfg(feature = "tui-preview")]
mod preview_fixture;

#[cfg(feature = "tui-preview")]
pub(super) use preview_fixture::render_preview_buffer;
#[cfg(all(feature = "tui-preview", test))]
pub(super) use preview_fixture::render_production_preview_buffer;

use crate::agent_id_display::{
    AgentIdHighlightKind, format_agent_id_for_terminal, highlight_agent_id,
};
use crate::app::context::ApplicationContext;
use crate::auth::AuthPresenter;
use crate::command::GolemCliGlobalFlags;
use crate::command_handler::{AgentStreamItem, Handlers, SshCommandHandler};
use crate::config::Config;
use crate::context::Context;
use crate::log::Output;
use crate::model::agent::oplog::OplogEntryGroupKey;
use crate::model::agent::{
    AgentListMode, AgentListPageCursor, AgentListRequest, AgentMetadataView,
    AgentsMetadataResponseView, RawAgentId,
};
use crate::model::app::ApplicationSourceMode;
use crate::model::app_raw::{BuiltinServer, Server};
use crate::model::environment::{EnvironmentReference, EnvironmentResolveMode};
use crate::model::masking::Masked;
use crate::tui::TuiEvent;
use crate::tui::context_executor::{TuiContextExecutor, TuiContextId, TuiContextTaskResult};
use crate::tui::input::encode_key_for_pty;
use crate::tui::interaction::{InteractionMode, ModeStack};
use crate::tui::layout::{
    self, DragTarget, LayoutInput, RegionKind, ScrollbarTarget, TuiLayoutState, UiGeometry,
    adaptive_data_popup_rect, content_popup_rect,
};
use crate::tui::navigation::{DevView, Workspace};
use crate::tui::nested_cli::{
    CommandExit, NestedCliEvent, NestedCliRuntime, NestedCliSpec, NestedCliTarget, spawn_nested_cli,
};
use crate::tui::ops::activity::{
    self, ActivityAgent, ActivityAgentRequest, ActivityFocus, ActivityInputKind,
    ActivityLoadPosition, ActivityOrder, ActivityRefreshPayload, ActivityRepresentation,
    ActivityState,
};
use crate::tui::ops::deployments::{self, DeploymentChangeKind, DeploymentsState, RECENT_LIMIT};
use crate::tui::ops::metrics::{
    self, MAX_EXPLORE_SERIES, MetricsFocus, MetricsLoad, MetricsMode, MetricsQuery, MetricsRange,
    MetricsRefreshPayload, MetricsState,
};
use crate::tui::ops::overview::{
    self, OverviewIssue, OverviewIssueTarget, OverviewItem, OverviewObservability, OverviewState,
};
use crate::tui::ops::resources::{self, ResourceScope, ResourcesRefreshPayload, ResourcesState};
use crate::tui::ops::ssh::{SshLineKind, SshState, SshStatus};
use crate::tui::ops::stream::{self, StreamState, StreamStatus};
use crate::tui::ops::{
    AgentIdentity, DeploymentCompareFocus, OplogEntryIdentity, OplogRoute, OpsApp, OpsDestination,
    OpsRoute,
};
use crate::tui::terminal::TerminalGuard;
use crate::tui::terminal_screen::TerminalScreen;
use crate::tui::visual::active_style;
use crate::tui::visual::{TuiVisualStyle, with_style};
use crate::tui::widgets::{
    ActionButton, CellPolicy, CellTone, CollectionQueryBar, ColumnChooserState, CommandMarker,
    CommandRow, ContextHeader, ContextPair, ControlChoice, ControlGroup, ControlStrip,
    CursorCollectionState, DecisionTableRow, FieldRow, FooterStatusCluster, FooterStatusItem,
    HelpRow, JsonDocument, KeyHint, NavigationRow, Notice, NoticeKind, OverlayFrame, PaneChrome,
    PaneEnding, PaneFocus, PaneHeader, PaneHeaderAction, PaneHeaderActions, PaneHeaderStatus,
    PaneLayout, PaneScrollbarSlot, PaneStatusTone, PaneTable, PaneTableCell, PaneTableCellSpan,
    PaneTableColumn, PaneTableState, ReviewFieldRow, ScrollableDocument, SearchInput,
    SectionHeading, SelectableRow, ShortcutRow, StatusKind, StatusMarker, WorkspaceItem,
    WorkspaceSelector, fit, pane_table_virtual_width, render_scrollbar, responsive_table_columns,
    shortcut_line, wrap_styled_line,
};
use crossterm::event::{
    Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use futures_util::StreamExt;
use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;
use golem_client::model::{EnvironmentWithDetails, OAuth2WebflowData, ScanCursor};
use golem_common::base_model::tool::ToolName;
use golem_common::model::agent::DeployedRegisteredAgentType;
use golem_common::model::oplog::OplogErrorKind;
use golem_common::model::{AgentStatus, LogLevel};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, ScrollbarState, Wrap};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::cell::Cell;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::io::{Write, stdout};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Instant, SystemTime};
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{
    mpsc::{self, Receiver, Sender},
    oneshot,
};
use tokio::time::{Duration, sleep};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const TUI_EVENT_CHANNEL_CAPACITY: usize = 1024;
const TUI_OUTPUT_CHANNEL_CAPACITY: usize = 64;
const TUI_TERMINAL_CHANNEL_CAPACITY: usize = 128;
const CONTEXT_PICKER_RIGHT_PADDING: usize = 4;
const AGENT_PAGE_SIZE: u64 = 200;
const COMPLETED_JOB_LIMIT: usize = 20;
const FORCE_STOP_DELAY: Duration = Duration::from_secs(5);

fn force_stop_available(requested_at: Option<Instant>) -> bool {
    requested_at.is_some_and(|requested| requested.elapsed() >= FORCE_STOP_DELAY)
}

#[derive(Clone)]
struct TuiEventSender {
    control: Sender<TuiEvent>,
    output: Sender<TuiEvent>,
}

impl TuiEventSender {
    fn new(control: Sender<TuiEvent>, output: Sender<TuiEvent>) -> Self {
        Self { control, output }
    }

    async fn send(
        &self,
        event: TuiEvent,
    ) -> Result<(), tokio::sync::mpsc::error::SendError<TuiEvent>> {
        self.control.send(event).await
    }

    fn try_send(&self, event: TuiEvent) -> Result<(), TrySendError<TuiEvent>> {
        self.control.try_send(event)
    }

    fn control(&self) -> Sender<TuiEvent> {
        self.control.clone()
    }

    fn output(&self) -> Sender<TuiEvent> {
        self.output.clone()
    }

    #[cfg(test)]
    fn single(sender: Sender<TuiEvent>) -> Self {
        Self::new(sender.clone(), sender)
    }
}

pub async fn run(ctx: Arc<Context>, global_flags: GolemCliGlobalFlags) -> anyhow::Result<()> {
    let context_executor = Arc::new(TuiContextExecutor::new(ctx.clone()));
    let mut app = TuiApp::from_launch(TuiLaunchConfig::new(ctx.clone(), global_flags));
    app.context_executor = Some(context_executor);
    let mut terminal = TerminalGuard::enter()?;
    let (control_tx, mut control_rx) = mpsc::channel::<TuiEvent>(TUI_EVENT_CHANNEL_CAPACITY);
    let (output_tx, mut output_rx) = mpsc::channel::<TuiEvent>(TUI_OUTPUT_CHANNEL_CAPACITY);
    let (terminal_tx, mut terminal_rx) = mpsc::channel::<Event>(TUI_TERMINAL_CHANNEL_CAPACITY);
    let event_tx = TuiEventSender::new(control_tx, output_tx);
    spawn_terminal_event_reader(terminal_tx);
    app.refresh_agents(Some(&event_tx));
    spawn_metrics_auto_refresh(event_tx.clone());

    terminal.draw(|frame| render(frame, &app))?;
    app.sync_nested_session_sizes();

    while !app.should_quit {
        let Some(event) = next_tui_event(&mut terminal_rx, &mut control_rx, &mut output_rx).await
        else {
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
                let redraw = terminal
                    .resume()
                    .and_then(|_| terminal.draw(|frame| render(frame, &app)));
                if let Err(error) = redraw {
                    terminal.restore_for_exit();
                    return Err(error);
                }
                app.sync_nested_session_sizes();
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
                    app.sync_nested_session_sizes();
                }
            }
        }
    }

    app.save_preferences();
    app.cleanup_running_command();
    Ok(())
}

async fn next_tui_event(
    terminal_rx: &mut Receiver<Event>,
    control_rx: &mut Receiver<TuiEvent>,
    output_rx: &mut Receiver<TuiEvent>,
) -> Option<TuiEvent> {
    tokio::select! {
        biased;
        event = terminal_rx.recv() => event.map(TuiEvent::Terminal),
        event = control_rx.recv() => event,
        event = output_rx.recv() => event,
    }
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

fn spawn_terminal_event_reader(event_tx: Sender<Event>) {
    tokio::spawn(async move {
        let mut reader = EventStream::new();
        while let Some(event) = reader.next().await {
            match event {
                Ok(event) => {
                    if event_tx.send(event).await.is_err() {
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

fn spawn_metrics_auto_refresh(event_tx: TuiEventSender) {
    tokio::spawn(async move {
        loop {
            sleep(metrics::REFRESH_INTERVAL).await;
            if event_channel_closed(event_tx.try_send(TuiEvent::MetricsRefreshTick)) {
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

enum MetricOplogTarget {
    Exact {
        agent: AgentIdentity,
        index: u64,
    },
    Range {
        agent: AgentIdentity,
        from_unix_nanos: i64,
        to_unix_nanos: i64,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MetricsRangeFormField {
    Apply,
    Preset,
    From,
    To,
}

impl MetricsRangeFormField {
    const ALL: [Self; 4] = [Self::Apply, Self::Preset, Self::From, Self::To];
}

#[derive(Clone, Debug)]
struct MetricsRangeForm {
    selected: usize,
    preset: MetricsRange,
    custom: bool,
    from: TextEditState,
    to: TextEditState,
    editing: bool,
    editing_original: Option<String>,
    error: Option<String>,
}

impl MetricsRangeForm {
    fn new(
        range: MetricsRange,
        loaded_range: Option<crate::model::local_observability::LocalMetricTimeRange>,
    ) -> Self {
        let exact_range = match range {
            MetricsRange::Custom(range) => range,
            _ => loaded_range.unwrap_or_else(|| metrics::current_range(range)),
        };
        Self {
            selected: 0,
            preset: if range.is_custom() {
                MetricsRange::Minutes15
            } else {
                range
            },
            custom: range.is_custom(),
            from: TextEditState::new(&format_metrics_range_input(exact_range.from_unix_nanos)),
            to: TextEditState::new(&format_metrics_range_input(exact_range.to_unix_nanos)),
            editing: false,
            editing_original: None,
            error: None,
        }
    }

    fn selected_field(&self) -> MetricsRangeFormField {
        MetricsRangeFormField::ALL[self.selected.min(MetricsRangeFormField::ALL.len() - 1)]
    }

    fn move_focus(&mut self, forward: bool) {
        self.selected = if forward {
            (self.selected + 1) % MetricsRangeFormField::ALL.len()
        } else {
            (self.selected + MetricsRangeFormField::ALL.len() - 1)
                % MetricsRangeFormField::ALL.len()
        };
        self.error = None;
    }

    fn editor(&self) -> Option<&TextEditState> {
        match self.selected_field() {
            MetricsRangeFormField::From => Some(&self.from),
            MetricsRangeFormField::To => Some(&self.to),
            _ => None,
        }
    }

    fn editor_mut(&mut self) -> Option<&mut TextEditState> {
        match self.selected_field() {
            MetricsRangeFormField::From => Some(&mut self.from),
            MetricsRangeFormField::To => Some(&mut self.to),
            _ => None,
        }
    }

    fn cycle_preset(&mut self, forward: bool) {
        self.preset = if forward {
            self.preset.next()
        } else {
            self.preset.previous()
        };
        self.custom = false;
        self.error = None;
    }

    fn begin_edit(&mut self) {
        let Some(editor) = self.editor() else {
            return;
        };
        self.editing_original = Some(editor.draft.clone());
        self.editing = true;
        self.custom = true;
        self.error = None;
    }

    fn finish_edit(&mut self) {
        self.editing = false;
        self.editing_original = None;
    }

    fn cancel_edit(&mut self) {
        let original = self.editing_original.take();
        if let Some(original) = original
            && let Some(editor) = self.editor_mut()
        {
            *editor = TextEditState::new(&original);
        }
        self.editing = false;
    }
}

struct TuiApp {
    should_quit: bool,
    active_workspace: Workspace,
    ops: OpsApp,
    dev_primary: DevView,
    dev_focus: DevView,
    mode: ModeStack,
    leader: LeaderState,
    quit_choice: QuitChoice,
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
    overview: OverviewState,
    deployments: DeploymentsState,
    metrics: MetricsState,
    metrics_range_form: Option<MetricsRangeForm>,
    activity: ActivityState,
    resources: ResourcesState,
    stream: StreamState,
    ssh: SshState,
    next_command_id: u64,
    selected_target: SelectedTarget,
    context_switcher: ContextSwitcherState,
    context_executor: Option<Arc<TuiContextExecutor>>,
    auth_prompt: Option<AuthPromptState>,
    layout: TuiLayoutState,
    scrollbar_dragging: Option<ScrollbarTarget>,
    geometry: UiGeometry,
    preferences_path: PathBuf,
    preferences_error: Option<String>,
}

impl TuiApp {
    fn from_launch(launch: TuiLaunchConfig) -> Self {
        let selected_target =
            SelectedTarget::from_context(launch.initial_context.as_ref(), &launch.base_flags);
        let active_workspace =
            if selected_target.context.dev_eligible && selected_target.context.uses_local_server {
                Workspace::Dev
            } else {
                Workspace::Ops
            };
        let server = LocalServerService::from_launch(&launch);
        let preferences_path = tui_preferences_path(&selected_target.context.config_dir);
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
            ops: OpsApp::default(),
            dev_primary: DevView::BuildDeploy,
            dev_focus: DevView::BuildDeploy,
            mode: ModeStack::default(),
            leader: LeaderState::default(),
            quit_choice: QuitChoice::default(),
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
            overview: OverviewState::default(),
            deployments: DeploymentsState::default(),
            metrics: MetricsState::default(),
            metrics_range_form: None,
            activity: ActivityState::default(),
            resources: ResourcesState::default(),
            stream: StreamState::default(),
            ssh: SshState::default(),
            next_command_id: 1,
            selected_target,
            context_switcher: ContextSwitcherState::new(targets),
            context_executor: None,
            auth_prompt: None,
            layout,
            scrollbar_dragging: None,
            geometry: UiGeometry::default(),
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
            TuiEvent::NestedCli { target, event } => {
                self.handle_nested_cli_event(target, event, event_tx)
            }
            TuiEvent::SpinnerTick(command_id) => self.handle_spinner_tick(command_id),
            TuiEvent::ServerSpinnerTick(server_id) => self.handle_server_spinner_tick(server_id),
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
            TuiEvent::OverviewObservabilityFinished { generation, result } => {
                self.finish_overview_observability(generation, result)
            }
            TuiEvent::DeploymentsRefreshFinished { generation, result } => {
                let (context_id, result, logs) = result.into_parts();
                self.deployments.finish(
                    generation,
                    context_id,
                    result.map_err(|error| agent_refresh_error(error, logs)),
                );
            }
            TuiEvent::MetricsRefreshTick => {
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Metrics
                    && self.metrics.live
                {
                    self.refresh_metrics_live(Some(event_tx));
                }
            }
            TuiEvent::MetricsRefreshFinished { generation, result } => {
                self.finish_metrics_refresh(generation, result)
            }
            TuiEvent::ActivityRefreshFinished { generation, result } => {
                self.finish_activity_refresh(generation, result)
            }
            TuiEvent::ResourcesRefreshFinished { generation, result } => {
                self.finish_resources_refresh(generation, result)
            }
            TuiEvent::StreamMessage {
                generation,
                message,
            } => {
                let visible = self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Stream;
                self.stream.apply_message(generation, message, visible);
            }
            TuiEvent::StreamSessionFinished { generation, result } => {
                let (context_id, result, logs) = result.into_parts();
                self.stream.finish(
                    generation,
                    context_id,
                    result.map_err(|error| agent_refresh_error(error, logs)),
                );
            }
            TuiEvent::SshConnected { generation, result } => {
                self.finish_ssh_connect(generation, result)
            }
            TuiEvent::SshCommandFinished { generation, result } => {
                self.finish_ssh_command(generation, result)
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

    fn handle_nested_cli_event(
        &mut self,
        target: NestedCliTarget,
        event: NestedCliEvent,
        event_tx: &TuiEventSender,
    ) {
        match (target, event) {
            (NestedCliTarget::Command, NestedCliEvent::Output(bytes)) => {
                self.append_command_output(&bytes)
            }
            (NestedCliTarget::Command, NestedCliEvent::OutputClosed(Some(error))) => {
                self.append_local_command_line(format!("output closed: {error}"));
            }
            (NestedCliTarget::Command, NestedCliEvent::OutputClosed(None)) => {}
            (NestedCliTarget::Command, NestedCliEvent::Exited(exit)) => {
                self.finish_command(exit, event_tx)
            }
            (NestedCliTarget::Server, NestedCliEvent::Output(bytes)) => {
                self.server.run.terminal.feed(&bytes)
            }
            (NestedCliTarget::Server, NestedCliEvent::OutputClosed(Some(error))) => {
                self.server
                    .run
                    .terminal
                    .feed_local_line(format!("server output closed: {error}"));
            }
            (NestedCliTarget::Server, NestedCliEvent::OutputClosed(None)) => {}
            (NestedCliTarget::Server, NestedCliEvent::Exited(exit)) => {
                self.finish_server(exit, event_tx)
            }
            (NestedCliTarget::Repl, NestedCliEvent::Output(bytes)) => {
                self.append_repl_output(&bytes)
            }
            (NestedCliTarget::Repl, NestedCliEvent::OutputClosed(error)) => {
                self.handle_repl_output_closed(error)
            }
            (NestedCliTarget::Repl, NestedCliEvent::Exited(exit)) => {
                self.finish_repl(exit, event_tx)
            }
        }
    }

    #[cfg(test)]
    fn handle_key(&mut self, key: KeyEvent) {
        self.handle_key_with_events(key, None);
    }

    fn handle_key_with_events(&mut self, key: KeyEvent, event_tx: Option<&TuiEventSender>) {
        match self.mode.current() {
            InteractionMode::Normal => self.handle_global_key(key, event_tx),
            InteractionMode::Leader => self.handle_leader_key(key, event_tx),
            InteractionMode::Layout => self.handle_layout_key(key),
            InteractionMode::Jobs => self.handle_jobs_key(key),
            InteractionMode::QuitBlocked => self.handle_quit_blocked_key(key),
            InteractionMode::Palette => self.handle_palette_key(key, event_tx),
            InteractionMode::CommandForm => self.handle_command_form_key(key, event_tx),
            InteractionMode::SessionForm => self.handle_session_form_key(key, event_tx),
            InteractionMode::ContextPicker => self.handle_context_picker_key(key, event_tx),
            InteractionMode::ContextSwitchConfirm => {
                self.handle_context_switch_confirm_key(key, event_tx)
            }
            InteractionMode::Help => self.handle_help_key(key),
            InteractionMode::AgentFilter => self.handle_agent_filter_key(key, event_tx),
            InteractionMode::AgentDatasetFilter => {
                self.handle_agent_dataset_filter_key(key, event_tx)
            }
            InteractionMode::AgentColumns => self.handle_agent_columns_key(key),
            InteractionMode::OplogFilter => self.handle_oplog_filter_key(key, event_tx),
            InteractionMode::MetricsRange => self.handle_metrics_range_key(key, event_tx),
            InteractionMode::TerminalInput => self.handle_terminal_input_key(key),
            InteractionMode::TerminalScrollback => self.handle_terminal_scrollback_key(key),
            InteractionMode::SshInput => self.handle_ssh_input_key(key, event_tx),
        }
    }

    fn handle_global_key(&mut self, key: KeyEvent, event_tx: Option<&TuiEventSender>) {
        if self.handle_modified_shortcut(key, event_tx) {
            return;
        }
        match key.code {
            KeyCode::Up
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Overview
                    && matches!(self.ops.current_route(), OpsRoute::Root) =>
            {
                self.overview.move_item(false)
            }
            KeyCode::Down
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Overview
                    && matches!(self.ops.current_route(), OpsRoute::Root) =>
            {
                self.overview.move_item(true)
            }
            KeyCode::Enter
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Overview
                    && matches!(self.ops.current_route(), OpsRoute::Root) =>
            {
                self.open_selected_overview_item(event_tx)
            }
            KeyCode::Up
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Overview
                    && matches!(self.ops.current_route(), OpsRoute::OverviewIssues) =>
            {
                self.overview
                    .move_issue(false, self.overview_issues().len())
            }
            KeyCode::Down
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Overview
                    && matches!(self.ops.current_route(), OpsRoute::OverviewIssues) =>
            {
                self.overview.move_issue(true, self.overview_issues().len())
            }
            KeyCode::Enter
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Overview
                    && matches!(self.ops.current_route(), OpsRoute::OverviewIssues) =>
            {
                self.open_selected_overview_issue(event_tx)
            }
            KeyCode::Up
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Deployments
                    && matches!(
                        self.ops.current_route(),
                        OpsRoute::Root | OpsRoute::DeploymentHistory
                    ) =>
            {
                self.move_deployment_selection(false)
            }
            KeyCode::Down
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Deployments
                    && matches!(
                        self.ops.current_route(),
                        OpsRoute::Root | OpsRoute::DeploymentHistory
                    ) =>
            {
                self.move_deployment_selection(true)
            }
            KeyCode::Enter
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Deployments
                    && matches!(
                        self.ops.current_route(),
                        OpsRoute::Root | OpsRoute::DeploymentHistory
                    ) =>
            {
                self.open_selected_deployment(event_tx)
            }
            KeyCode::Enter
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Deployments
                    && matches!(self.ops.current_route(), OpsRoute::DeploymentRevision(_)) =>
            {
                self.compare_deployment_with_predecessor(event_tx)
            }
            KeyCode::Up
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Deployments
                    && matches!(self.ops.current_route(), OpsRoute::DeploymentCompare(_)) =>
            {
                self.move_deployment_change_selection(false)
            }
            KeyCode::Down
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Deployments
                    && matches!(self.ops.current_route(), OpsRoute::DeploymentCompare(_)) =>
            {
                self.move_deployment_change_selection(true)
            }
            KeyCode::Left
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Deployments
                    && matches!(self.ops.current_route(), OpsRoute::DeploymentCompare(_)) =>
            {
                self.move_deployment_compare_revision(false, event_tx)
            }
            KeyCode::Right
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Deployments
                    && matches!(self.ops.current_route(), OpsRoute::DeploymentCompare(_)) =>
            {
                self.move_deployment_compare_revision(true, event_tx)
            }
            KeyCode::Tab | KeyCode::BackTab
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Deployments
                    && matches!(self.ops.current_route(), OpsRoute::DeploymentCompare(_)) =>
            {
                self.toggle_deployment_compare_focus()
            }
            KeyCode::Up
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Deployments
                    && matches!(self.ops.current_route(), OpsRoute::DeploymentRevision(_)) =>
            {
                self.deployments.scroll_up(1)
            }
            KeyCode::Down
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Deployments
                    && matches!(self.ops.current_route(), OpsRoute::DeploymentRevision(_)) =>
            {
                self.deployments.scroll_down(1)
            }
            KeyCode::PageUp
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Deployments
                    && matches!(self.ops.current_route(), OpsRoute::DeploymentRevision(_)) =>
            {
                self.deployments.scroll_up(10)
            }
            KeyCode::PageDown
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Deployments
                    && matches!(self.ops.current_route(), OpsRoute::DeploymentRevision(_)) =>
            {
                self.deployments.scroll_down(10)
            }
            KeyCode::Home
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Deployments
                    && matches!(self.ops.current_route(), OpsRoute::DeploymentRevision(_)) =>
            {
                self.deployments.scroll = 0
            }
            KeyCode::End
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Deployments
                    && matches!(self.ops.current_route(), OpsRoute::DeploymentRevision(_)) =>
            {
                self.deployments.scroll_to_end()
            }
            KeyCode::Up
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Stream =>
            {
                self.stream.scroll_up(1)
            }
            KeyCode::Down
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Stream =>
            {
                self.stream.scroll_down(1)
            }
            KeyCode::PageUp
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Stream =>
            {
                self.stream.scroll_up(10)
            }
            KeyCode::PageDown
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Stream =>
            {
                self.stream.scroll_down(10)
            }
            KeyCode::Home
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Stream =>
            {
                self.stream.scroll_to_start()
            }
            KeyCode::End
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Stream =>
            {
                self.stream.follow_latest()
            }
            KeyCode::Up
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Oplog
                    && self.activity_list_focused() =>
            {
                self.move_activity_selection(false)
            }
            KeyCode::Down
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Oplog
                    && self.activity_list_focused() =>
            {
                self.move_activity_selection(true)
            }
            KeyCode::PageUp if self.activity_list_focused() => {
                self.move_activity_selection_by(10, false)
            }
            KeyCode::PageDown if self.activity_list_focused() => {
                self.move_activity_selection_by(10, true)
            }
            KeyCode::Home if self.activity_list_focused() => self.select_first_activity(),
            KeyCode::End if self.activity_list_focused() => self.select_last_activity(),
            KeyCode::Up if self.activity_details_focused() => self.scroll_activity_details_up(1),
            KeyCode::Down if self.activity_details_focused() => {
                self.scroll_activity_details_down(1)
            }
            KeyCode::PageUp if self.activity_details_focused() => {
                self.scroll_activity_details_up(10)
            }
            KeyCode::PageDown if self.activity_details_focused() => {
                self.scroll_activity_details_down(10)
            }
            KeyCode::Home if self.activity_details_focused() => self.activity.details_scroll = 0,
            KeyCode::End if self.activity_details_focused() => {
                self.scroll_activity_details_bottom()
            }
            KeyCode::Left
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Oplog =>
            {
                self.set_activity_representation(self.activity.representation.previous(), event_tx)
            }
            KeyCode::Right
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Oplog =>
            {
                self.set_activity_representation(self.activity.representation.next(), event_tx)
            }
            KeyCode::Left
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Metrics
                    && self.metrics_chart_inspection_active()
                    && key.modifiers.contains(KeyModifiers::SHIFT) =>
            {
                self.move_metrics_chart_cursor(false)
            }
            KeyCode::Right
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Metrics
                    && self.metrics_chart_inspection_active()
                    && key.modifiers.contains(KeyModifiers::SHIFT) =>
            {
                self.move_metrics_chart_cursor(true)
            }
            KeyCode::Left
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Metrics
                    && key.modifiers.is_empty()
                    && matches!(self.ops.current_route(), OpsRoute::Root) =>
            {
                self.set_metrics_mode(self.metrics.mode.previous(), event_tx);
            }
            KeyCode::Right
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Metrics
                    && key.modifiers.is_empty()
                    && matches!(self.ops.current_route(), OpsRoute::Root) =>
            {
                self.set_metrics_mode(self.metrics.mode.next(), event_tx);
            }
            KeyCode::Up
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Metrics
                    && matches!(self.ops.current_route(), OpsRoute::Root) =>
            {
                self.move_metrics_selection(false, event_tx)
            }
            KeyCode::Down
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Metrics
                    && matches!(self.ops.current_route(), OpsRoute::Root) =>
            {
                self.move_metrics_selection(true, event_tx)
            }
            KeyCode::Up
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Configuration =>
            {
                self.move_resource_selection(false, event_tx)
            }
            KeyCode::Down
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Configuration =>
            {
                self.move_resource_selection(true, event_tx)
            }
            KeyCode::Left
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Configuration
                    && key.modifiers.is_empty() =>
            {
                self.resources.select_scope(self.resources.scope.previous());
                self.refresh_resources(event_tx, false);
            }
            KeyCode::Right
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Configuration
                    && key.modifiers.is_empty() =>
            {
                self.resources.select_scope(self.resources.scope.next());
                self.refresh_resources(event_tx, false);
            }
            KeyCode::Esc if self.agent_details_focused() => {
                self.agents.focus = AgentOverviewFocus::List
            }
            KeyCode::Esc if self.activity_details_focused() => {
                self.activity.focus = ActivityFocus::List
            }
            KeyCode::Esc
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Oplog
                    && !self.activity.loaded_filter.is_empty()
                    && self.activity.input_kind == ActivityInputKind::Loaded =>
            {
                self.activity.loaded_filter.clear();
                self.activity.selected = 0;
                self.activity.list_first_visible.set(0);
            }
            KeyCode::Esc
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Oplog
                    && self.activity.query != self.activity.applied_query.text =>
            {
                self.activity.query = self.activity.applied_query.text.clone();
            }
            KeyCode::Esc if self.active_workspace == Workspace::Ops => {
                if self.ops.pop_route() {
                    match self.ops.active() {
                        OpsDestination::Oplog => {
                            self.invalidate_activity();
                            self.refresh_activity(event_tx);
                        }
                        OpsDestination::Deployments => {
                            let visible = self.deployment_history_visible_len();
                            self.deployments.selected =
                                self.deployments.selected.min(visible.saturating_sub(1));
                            self.deployments.table_first_visible.set(0);
                            self.deployments.scroll = 0;
                        }
                        _ => {}
                    }
                }
            }
            KeyCode::Enter
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Metrics
                    && self.metrics_chart_inspection_active() =>
            {
                self.open_metric_sample_in_oplog(event_tx)
            }
            KeyCode::Enter
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Oplog
                    && self.activity.input_kind == ActivityInputKind::Server =>
            {
                self.apply_activity_query(event_tx)
            }
            KeyCode::Enter
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Metrics
                    && self.metrics.mode == MetricsMode::Dashboard
                    && matches!(self.ops.current_route(), OpsRoute::Root) =>
            {
                self.open_selected_metric_panel()
            }
            KeyCode::Enter
                if self.active_workspace == Workspace::Ops
                    && matches!(self.ops.current_route(), OpsRoute::Ssh(_))
                    && self.ssh.status == SshStatus::Ready =>
            {
                self.mode.replace(InteractionMode::SshInput)
            }
            KeyCode::PageUp
                if self.active_workspace == Workspace::Ops
                    && matches!(self.ops.current_route(), OpsRoute::Ssh(_)) =>
            {
                self.ssh.scroll_up(10)
            }
            KeyCode::PageDown
                if self.active_workspace == Workspace::Ops
                    && matches!(self.ops.current_route(), OpsRoute::Ssh(_)) =>
            {
                self.ssh.scroll_down(10)
            }
            KeyCode::Enter if self.focused_terminal_is_running() => self.enter_terminal_input(),
            KeyCode::PageUp if self.active_workspace == Workspace::Dev => {
                self.scroll_terminal_up(10);
                self.mode.push(InteractionMode::TerminalScrollback);
            }
            KeyCode::PageDown if self.active_workspace == Workspace::Dev => {
                self.scroll_terminal_down(10);
                self.mode.push(InteractionMode::TerminalScrollback);
            }
            KeyCode::Home if self.active_workspace == Workspace::Dev => {
                self.scroll_terminal_top();
                self.mode.push(InteractionMode::TerminalScrollback);
            }
            KeyCode::End if self.active_workspace == Workspace::Dev => {
                self.scroll_terminal_bottom();
                self.mode.push(InteractionMode::TerminalScrollback);
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
                self.mode.replace(InteractionMode::AgentFilter);
                self.agents.query.pop();
                self.agents.reset_selection();
            }
            KeyCode::Char(character)
                if self.agent_list_focused()
                    && (key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT)
                    && (!character.is_whitespace() || !self.agents.query.is_empty()) =>
            {
                self.mode.replace(InteractionMode::AgentFilter);
                self.agents.query.push(character);
                self.agents.reset_selection();
            }
            KeyCode::Backspace
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Oplog
                    && self.activity.input_kind == ActivityInputKind::Loaded
                    && !self.activity.loaded_filter.is_empty() =>
            {
                self.mode.replace(InteractionMode::OplogFilter);
                self.activity.loaded_filter.pop();
                self.activity_filter_changed();
            }
            KeyCode::Backspace
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Oplog
                    && self.activity.input_kind == ActivityInputKind::Server
                    && !self.activity.query.is_empty() =>
            {
                self.mode.replace(InteractionMode::OplogFilter);
                self.activity.query.pop();
            }
            KeyCode::Char(character)
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Oplog
                    && (key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT)
                    && (!character.is_whitespace() || !self.activity.query.is_empty()) =>
            {
                self.mode.replace(InteractionMode::OplogFilter);
                match self.activity.input_kind {
                    ActivityInputKind::Loaded => {
                        self.activity.loaded_filter.push(character);
                        self.activity_filter_changed();
                    }
                    ActivityInputKind::Server => self.activity.query.push(character),
                }
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
                self.open_palette(self.palette.scope);
            }
            KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.open_help();
            }
            KeyCode::Char('r')
                if self.active_workspace == Workspace::Ops
                    && key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                match self.ops.active() {
                    OpsDestination::Overview => self.refresh_overview(event_tx, true),
                    OpsDestination::Deployments => self.refresh_deployments(event_tx),
                    OpsDestination::Agents => self.refresh_agents(event_tx),
                    OpsDestination::Oplog => self.refresh_activity(event_tx),
                    OpsDestination::Metrics => self.refresh_metrics(event_tx),
                    OpsDestination::Configuration => self.refresh_resources(event_tx, true),
                    OpsDestination::Stream => {}
                }
            }
            KeyCode::Char('k')
                if self.active_workspace == Workspace::Ops
                    && key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.toggle_stream_session(event_tx)
            }
            KeyCode::Char('a')
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Deployments
                    && key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.ops.open_deployment_history();
                self.deployments.table_first_visible.set(0);
            }
            KeyCode::Char('n')
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Oplog
                    && key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.load_more_activity(event_tx)
            }
            KeyCode::Char('g')
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Oplog
                    && key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.stop_activity()
            }
            KeyCode::Char('o')
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Oplog
                    && key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.set_activity_order(self.activity.order.toggle());
            }
            KeyCode::Enter
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Oplog
                    && key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.load_selected_activity_context(event_tx);
            }
            KeyCode::Char('f')
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Oplog
                    && key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.activity.input_kind = ActivityInputKind::Loaded;
                self.mode.replace(InteractionMode::OplogFilter);
            }
            KeyCode::Char('s')
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Oplog
                    && key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.activity.input_kind = ActivityInputKind::Server;
                self.mode.replace(InteractionMode::OplogFilter);
            }
            KeyCode::Char('s')
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Metrics
                    && key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                let source = match self.metrics.source {
                    crate::model::local_observability::LocalMetricSource::Agents => {
                        crate::model::local_observability::LocalMetricSource::Services
                    }
                    crate::model::local_observability::LocalMetricSource::Services => {
                        crate::model::local_observability::LocalMetricSource::Agents
                    }
                };
                self.set_metrics_source(source, event_tx);
            }
            KeyCode::Char('t')
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Metrics
                    && key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.open_metrics_range_form();
            }
            KeyCode::Char('l')
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Metrics
                    && key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.set_metrics_live(!self.metrics.live, event_tx);
            }
            KeyCode::Char(' ')
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Metrics
                    && self.metrics.mode == MetricsMode::Explore
                    && self.metrics.focus == MetricsFocus::Series
                    && key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.toggle_metrics_series(event_tx);
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
            return;
        }
        if key.code == KeyCode::Char('x') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.open_leader();
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
            KeyCode::Left => self.set_palette_scope(self.palette.scope.previous()),
            KeyCode::Right => self.set_palette_scope(self.palette.scope.next()),
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
                    self.execute_palette_action(action.id, event_tx);
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
            KeyCode::Esc => self.close_transient_mode(),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.close_transient_mode();
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
                self.close_transient_mode();
            }
            _ => {}
        }
    }

    fn handle_help_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.close_transient_mode(),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.close_transient_mode();
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
        self.mode.push(InteractionMode::Help);
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
            KeyCode::Esc | KeyCode::Enter => self.mode.replace(InteractionMode::Normal),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.mode.replace(InteractionMode::Normal);
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
            self.mode.replace(InteractionMode::Normal);
            return;
        };
        match key.code {
            KeyCode::Esc => {
                self.agents.dataset_filter = None;
                self.mode.replace(InteractionMode::Normal);
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
                self.mode.replace(InteractionMode::Normal);
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

    fn handle_oplog_filter_key(&mut self, key: KeyEvent, event_tx: Option<&TuiEventSender>) {
        if self.handle_modified_shortcut(key, event_tx) {
            return;
        }
        match key.code {
            KeyCode::Esc => {
                if self.activity.input_kind == ActivityInputKind::Server {
                    self.activity.query = self.activity.applied_query.text.clone();
                }
                self.mode.replace(InteractionMode::Normal);
            }
            KeyCode::Enter => {
                if self.activity.input_kind == ActivityInputKind::Server {
                    self.apply_activity_query(event_tx);
                }
                self.mode.replace(InteractionMode::Normal);
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.mode.replace(InteractionMode::Normal);
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                match self.activity.input_kind {
                    ActivityInputKind::Loaded => {
                        self.activity.loaded_filter.clear();
                        self.activity_filter_changed();
                    }
                    ActivityInputKind::Server => self.activity.query.clear(),
                }
            }
            KeyCode::Backspace => match self.activity.input_kind {
                ActivityInputKind::Loaded => {
                    self.activity.loaded_filter.pop();
                    self.activity_filter_changed();
                }
                ActivityInputKind::Server => {
                    self.activity.query.pop();
                }
            },
            KeyCode::Up => self.move_activity_selection(false),
            KeyCode::Down => self.move_activity_selection(true),
            KeyCode::Tab | KeyCode::BackTab => self.cycle_ops_pane_focus(),
            KeyCode::Left => {
                self.set_activity_representation(self.activity.representation.previous(), event_tx)
            }
            KeyCode::Right => {
                self.set_activity_representation(self.activity.representation.next(), event_tx)
            }
            KeyCode::Char(character)
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                match self.activity.input_kind {
                    ActivityInputKind::Loaded => {
                        self.activity.loaded_filter.push(character);
                        self.activity_filter_changed();
                    }
                    ActivityInputKind::Server => self.activity.query.push(character),
                }
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
            self.close_leader();
            return;
        }

        match self.leader.group {
            None => match key.code {
                KeyCode::Char('w') => self.leader.group = Some(LeaderGroup::Workspaces),
                KeyCode::Char('v') => self.leader.group = Some(LeaderGroup::Views),
                KeyCode::Char('l') => self.mode.replace(InteractionMode::Layout),
                KeyCode::Char('a') if self.mode.previous() == Some(InteractionMode::Palette) => {
                    self.palette.scope = PaletteScope::Context;
                    self.palette.selected = 0;
                    self.leader.group = None;
                    let fallback = self.default_mode();
                    self.mode.pop_or(fallback);
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
                    self.active_workspace = Workspace::Dev;
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
                    (Workspace::Dev, KeyCode::Char('b')) => {
                        self.open_dev_workspace(DevView::BuildDeploy)
                    }
                    (Workspace::Dev, KeyCode::Char('s')) => {
                        self.open_dev_workspace(DevView::Server)
                    }
                    (Workspace::Dev, KeyCode::Char('r')) => self.open_dev_workspace(DevView::Repl),
                    (Workspace::Ops, KeyCode::Char('o')) => {
                        self.select_ops_destination(OpsDestination::Overview, event_tx)
                    }
                    (Workspace::Ops, KeyCode::Char('a')) => {
                        self.select_ops_destination(OpsDestination::Agents, event_tx)
                    }
                    (Workspace::Ops, KeyCode::Char('l')) => {
                        self.select_ops_destination(OpsDestination::Oplog, event_tx)
                    }
                    (Workspace::Ops, KeyCode::Char('s')) => {
                        self.select_ops_destination(OpsDestination::Stream, event_tx)
                    }
                    (Workspace::Ops, KeyCode::Char('m')) => {
                        self.select_ops_destination(OpsDestination::Metrics, event_tx)
                    }
                    (Workspace::Ops, KeyCode::Char('d')) => {
                        self.select_ops_destination(OpsDestination::Deployments, event_tx)
                    }
                    (Workspace::Ops, KeyCode::Char('c')) => {
                        self.select_ops_destination(OpsDestination::Configuration, event_tx)
                    }
                    (Workspace::Ops, KeyCode::Char('n')) => {
                        self.ops.select_next();
                        self.refresh_active_ops_destination(event_tx)
                    }
                    (Workspace::Ops, KeyCode::Char('p')) => {
                        self.ops.select_previous();
                        self.refresh_active_ops_destination(event_tx)
                    }
                    _ => {}
                }
                self.close_leader();
            }
        }
    }

    fn handle_agent_columns_key(&mut self, key: KeyEvent) {
        let Some(chooser) = self.agents.column_chooser.as_mut() else {
            self.mode.replace(InteractionMode::Normal);
            return;
        };
        match key.code {
            KeyCode::Esc => {
                self.agents.column_chooser = None;
                self.mode.replace(InteractionMode::Normal);
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
                self.mode.replace(InteractionMode::Normal);
            }
            _ => {}
        }
    }

    fn handle_mouse(&mut self, mouse: MouseEvent, event_tx: Option<&TuiEventSender>) {
        if self.handle_scrollbar_mouse(mouse, event_tx) {
            return;
        }
        if self.mode == InteractionMode::Help {
            match mouse.kind {
                MouseEventKind::ScrollUp => self.help_scroll = self.help_scroll.saturating_sub(3),
                MouseEventKind::ScrollDown => self.scroll_help_down(3),
                _ => {}
            }
            return;
        }
        if self.mode == InteractionMode::Palette {
            let action_count = self.filtered_actions().len();
            match mouse.kind {
                MouseEventKind::ScrollUp => self.palette.previous(action_count),
                MouseEventKind::ScrollDown => self.palette.next(action_count),
                MouseEventKind::Down(MouseButton::Left) => {
                    self.click_region(mouse.column, mouse.row, event_tx)
                }
                _ => {}
            }
            return;
        }
        if self.mode == InteractionMode::QuitBlocked {
            if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                self.click_region(mouse.column, mouse.row, event_tx);
            }
            return;
        }
        if matches!(
            self.mode.current(),
            InteractionMode::CommandForm
                | InteractionMode::SessionForm
                | InteractionMode::MetricsRange
        ) {
            return;
        }
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.scroll_region_under_pointer(mouse.column, mouse.row, 3, true, event_tx)
            }
            MouseEventKind::ScrollDown => {
                self.scroll_region_under_pointer(mouse.column, mouse.row, 3, false, event_tx)
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let target = self.drag_target_at(mouse.column, mouse.row);
                self.layout.dragging = target;
                self.layout.drag_origin = target.map(|_| (mouse.column, mouse.row));
                self.layout.drag_moved = false;
                if target.is_none() {
                    self.click_region(mouse.column, mouse.row, event_tx);
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                self.layout.drag_moved |= self
                    .layout
                    .drag_origin
                    .is_some_and(|origin| origin != (mouse.column, mouse.row));
                self.drag_layout(mouse.column, mouse.row);
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.layout.drag_moved |= self
                    .layout
                    .drag_origin
                    .is_some_and(|origin| origin != (mouse.column, mouse.row));
                self.drag_layout(mouse.column, mouse.row);
                let activate_title = self.layout.dragging.is_some() && !self.layout.drag_moved;
                self.layout.dragging = None;
                self.layout.drag_origin = None;
                self.layout.drag_moved = false;
                if activate_title {
                    self.click_region_below_splits(mouse.column, mouse.row, event_tx);
                }
                self.save_preferences();
            }
            _ => {}
        }
    }

    fn handle_scrollbar_mouse(
        &mut self,
        mouse: MouseEvent,
        event_tx: Option<&TuiEventSender>,
    ) -> bool {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let Some(target) = self.scrollbar_target_at(mouse.column, mouse.row) else {
                    return false;
                };
                self.scrollbar_dragging = Some(target);
                self.layout.dragging = None;
                self.drag_scrollbar(target, mouse.row, event_tx);
                true
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let Some(target) = self.scrollbar_dragging else {
                    return false;
                };
                self.drag_scrollbar(target, mouse.row, event_tx);
                true
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let Some(target) = self.scrollbar_dragging.take() else {
                    return false;
                };
                self.drag_scrollbar(target, mouse.row, event_tx);
                true
            }
            _ => false,
        }
    }

    fn scrollbar_target_at(&self, x: u16, y: u16) -> Option<ScrollbarTarget> {
        let RegionKind::Scrollbar(target) = self.geometry.hit_test(x, y)? else {
            return None;
        };
        match self.mode.current() {
            InteractionMode::Help if target == ScrollbarTarget::Help => Some(target),
            InteractionMode::Normal
            | InteractionMode::Layout
            | InteractionMode::AgentFilter
            | InteractionMode::OplogFilter
            | InteractionMode::TerminalInput
            | InteractionMode::TerminalScrollback
                if target != ScrollbarTarget::Help =>
            {
                Some(target)
            }
            _ => None,
        }
    }

    fn drag_scrollbar(
        &mut self,
        target: ScrollbarTarget,
        y: u16,
        event_tx: Option<&TuiEventSender>,
    ) {
        let Some(area) = self.geometry.region(RegionKind::Scrollbar(target)) else {
            return;
        };
        match target {
            ScrollbarTarget::Dev(view) => {
                self.active_workspace = Workspace::Dev;
                self.dev_focus = view;
                let screen = match view {
                    DevView::BuildDeploy => self.command_run.as_mut().map(|run| &mut run.terminal),
                    DevView::Server => Some(&mut self.server.run.terminal),
                    DevView::Repl => Some(&mut self.repl.run.screen),
                };
                if let Some(screen) = screen {
                    let top_line = scrollbar_value_from_pointer(area, y, screen.max_scrollback());
                    screen.set_scrollback(screen.max_scrollback().saturating_sub(top_line));
                    if screen.scrollback() > 0
                        && self.mode.current() != InteractionMode::TerminalScrollback
                    {
                        self.mode.push(InteractionMode::TerminalScrollback);
                    }
                }
            }
            ScrollbarTarget::OpsList => {
                if self.ops_activity_focused() {
                    let count = self.activity_visible_len();
                    if count == 0 {
                        return;
                    }
                    self.activity.focus = ActivityFocus::List;
                    self.activity.selected =
                        scrollbar_value_from_pointer(area, y, count.saturating_sub(1));
                    self.activity.details_scroll = 0;
                    self.activity.reveal_selection(count);
                    return;
                }
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Deployments
                {
                    let count = match self.ops.current_route() {
                        OpsRoute::Root => self.deployments.history.len().min(RECENT_LIMIT),
                        OpsRoute::DeploymentHistory => self.deployments.history.len(),
                        OpsRoute::DeploymentCompare(route) => self
                            .deployments
                            .changes(route.base_revision, route.target_revision)
                            .map_or(0, |changes| changes.len()),
                        _ => 0,
                    };
                    if count > 0 {
                        let selected =
                            scrollbar_value_from_pointer(area, y, count.saturating_sub(1));
                        if matches!(self.ops.current_route(), OpsRoute::DeploymentCompare(_)) {
                            self.deployments.selected_change = selected;
                        } else {
                            self.deployments.selected = selected;
                        }
                    }
                    return;
                }
                let count = self.filtered_agents().len();
                if count == 0 {
                    return;
                }
                self.agents.focus = AgentOverviewFocus::List;
                self.agents.selected =
                    scrollbar_value_from_pointer(area, y, count.saturating_sub(1));
                self.agents.table.selected = self.agents.selected;
                self.agents.details_scroll = 0;
                self.agents.reveal_selection(count);
            }
            ScrollbarTarget::OpsDetails | ScrollbarTarget::OpsDocument => {
                if target == ScrollbarTarget::OpsDetails && self.ops_activity_focused() {
                    self.activity.focus = ActivityFocus::Details;
                    let max = self
                        .activity
                        .details_content_height
                        .get()
                        .saturating_sub(self.activity.details_viewport_height.get());
                    self.activity.details_scroll = scrollbar_value_from_pointer(area, y, max);
                    return;
                }
                if target == ScrollbarTarget::OpsDocument
                    && self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Deployments
                    && matches!(self.ops.current_route(), OpsRoute::DeploymentRevision(_))
                {
                    let max = self
                        .deployments
                        .content_height
                        .get()
                        .saturating_sub(self.deployments.viewport_height.get());
                    self.deployments.scroll = scrollbar_value_from_pointer(area, y, max);
                    return;
                }
                if target == ScrollbarTarget::OpsDetails {
                    self.agents.focus = AgentOverviewFocus::Details;
                }
                let max = self
                    .agents
                    .details_content_height
                    .get()
                    .saturating_sub(self.agents.details_viewport_height.get());
                self.agents.details_scroll = scrollbar_value_from_pointer(area, y, max);
            }
            ScrollbarTarget::OpsStream => {
                let max = self
                    .stream
                    .events
                    .len()
                    .saturating_sub(area.height as usize);
                let start = scrollbar_value_from_pointer(area, y, max);
                self.stream.scroll_from_end = max.saturating_sub(start);
                self.stream.follow = self.stream.scroll_from_end == 0;
            }
            ScrollbarTarget::MetricsDashboard => {
                let count = self
                    .metrics
                    .dashboard
                    .as_ref()
                    .map(|dashboard| dashboard.panels.len())
                    .unwrap_or(0);
                if count > 0 {
                    let columns = self.metrics.dashboard_columns.get().max(1);
                    let rows = count.div_ceil(columns);
                    let row = scrollbar_value_from_pointer(area, y, rows.saturating_sub(1));
                    let column = self.metrics.selected_panel % columns;
                    self.metrics.selected_panel = row
                        .saturating_mul(columns)
                        .saturating_add(column)
                        .min(count - 1);
                    self.metrics.chart_cursor = None;
                }
            }
            ScrollbarTarget::MetricsFamilies => {
                let count = self.metrics.families.len();
                if count > 0 {
                    let selected = scrollbar_value_from_pointer(area, y, count.saturating_sub(1));
                    self.select_metrics_family(selected, event_tx);
                }
            }
            ScrollbarTarget::MetricsSeries => {
                let count = self.metrics.series.len();
                if count > 0 {
                    let selected = scrollbar_value_from_pointer(area, y, count.saturating_sub(1));
                    self.select_metrics_series(selected);
                }
            }
            ScrollbarTarget::ResourceKinds => {
                let count = resources::ResourceKind::for_scope(self.resources.scope).len();
                if count > 0 {
                    let selected = scrollbar_value_from_pointer(area, y, count.saturating_sub(1));
                    if self.resources.focus_kind(selected) {
                        self.refresh_resources(event_tx, false);
                    }
                }
            }
            ScrollbarTarget::ResourceItems => {
                let count = self.resources.visible_rows().len();
                if count > 0 {
                    let selected = scrollbar_value_from_pointer(area, y, count.saturating_sub(1));
                    self.resources.focus_row(selected);
                }
            }
            ScrollbarTarget::ResourceDetails => {
                let max = self.resources.details_max_scroll();
                let scroll = scrollbar_value_from_pointer(area, y, max);
                self.resources.focus_details();
                self.resources.set_details_scroll(scroll);
            }
            ScrollbarTarget::Help => {
                let max = self
                    .help_content_height
                    .get()
                    .saturating_sub(self.help_viewport_height.get());
                self.help_scroll = scrollbar_value_from_pointer(area, y, max);
            }
        }
    }

    fn scroll_region_under_pointer(
        &mut self,
        x: u16,
        y: u16,
        amount: usize,
        up: bool,
        event_tx: Option<&TuiEventSender>,
    ) {
        let region = self.geometry.hit_test(x, y);
        if region.is_none() {
            self.scroll_focused_region(amount, up);
            return;
        }
        match region {
            Some(RegionKind::DevViewBody(DevView::BuildDeploy)) => {
                if up {
                    self.scroll_output_up_by(amount);
                    self.mode.push(InteractionMode::TerminalScrollback);
                } else {
                    self.scroll_output_down_by(amount);
                }
            }
            Some(RegionKind::DevViewBody(DevView::Server)) => {
                if up {
                    self.scroll_server_up_by(amount);
                    self.mode.push(InteractionMode::TerminalScrollback);
                } else {
                    self.scroll_server_down_by(amount);
                }
            }
            Some(RegionKind::DevViewBody(DevView::Repl)) => {
                if up {
                    self.repl.run.screen.scroll_up(amount);
                    self.mode.push(InteractionMode::TerminalScrollback);
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
            Some(RegionKind::OpsDetails) if self.ops_activity_focused() => {
                self.activity.focus = ActivityFocus::Details;
                if up {
                    self.scroll_activity_details_up(amount);
                } else {
                    self.scroll_activity_details_down(amount);
                }
            }
            Some(RegionKind::OpsList) if self.ops_activity_focused() => {
                self.activity.focus = ActivityFocus::List;
                self.move_activity_selection_by(amount, !up);
            }
            Some(RegionKind::OpsList) if self.ops_agents_focused() => {
                self.agents.focus = AgentOverviewFocus::List;
                self.scroll_agent_list(amount, up);
            }
            Some(RegionKind::OpsList)
                if self.active_workspace == Workspace::Ops
                    && matches!(self.ops.current_route(), OpsRoute::Ssh(_)) =>
            {
                if up {
                    self.ssh.scroll_up(amount);
                } else {
                    self.ssh.scroll_down(amount);
                }
            }
            Some(RegionKind::OpsList) if self.ops_document_focused() => {
                if up {
                    self.scroll_agent_details_up(amount);
                } else {
                    self.scroll_agent_details_down(amount);
                }
            }
            Some(RegionKind::OpsList)
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Deployments =>
            {
                match self.ops.current_route() {
                    OpsRoute::DeploymentRevision(_) => {
                        if up {
                            self.deployments.scroll_up(amount);
                        } else {
                            self.deployments.scroll_down(amount);
                        }
                    }
                    OpsRoute::DeploymentCompare(_) => {
                        for _ in 0..amount {
                            self.move_deployment_change_selection(!up);
                        }
                    }
                    OpsRoute::Root | OpsRoute::DeploymentHistory => {
                        for _ in 0..amount {
                            self.move_deployment_selection(!up);
                        }
                    }
                    _ => {}
                }
            }
            Some(RegionKind::OpsStream)
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Stream =>
            {
                if up {
                    self.stream.scroll_up(amount);
                } else {
                    self.stream.scroll_down(amount);
                }
            }
            Some(RegionKind::MetricsChart)
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Metrics =>
            {
                self.move_metrics_chart_cursor(!up)
            }
            Some(RegionKind::MetricsDashboard | RegionKind::MetricsPanel(_))
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Metrics
                    && self.metrics.mode == MetricsMode::Dashboard =>
            {
                for _ in 0..amount {
                    self.move_metrics_selection(!up, event_tx);
                }
            }
            Some(RegionKind::MetricsFamilies)
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Metrics =>
            {
                self.metrics.focus = MetricsFocus::Families;
                for _ in 0..amount {
                    self.move_metrics_selection(!up, event_tx);
                }
            }
            Some(RegionKind::MetricsSeries)
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Metrics =>
            {
                self.metrics.focus = MetricsFocus::Series;
                for _ in 0..amount {
                    self.move_metrics_selection(!up, event_tx);
                }
            }
            Some(RegionKind::ResourceKinds)
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Configuration =>
            {
                self.resources
                    .focus_kind(self.resources.selected_kind_index());
                for _ in 0..amount {
                    self.move_resource_selection(!up, event_tx);
                }
            }
            Some(RegionKind::ResourceItems)
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Configuration =>
            {
                self.resources
                    .focus_row(self.resources.selected_row_index());
                for _ in 0..amount {
                    self.move_resource_selection(!up, event_tx);
                }
            }
            Some(RegionKind::ResourceDetails)
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Configuration =>
            {
                self.resources.focus_details();
                self.resources.scroll_details(amount, !up);
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
        } else if self.active_workspace == Workspace::Ops
            && matches!(self.ops.current_route(), OpsRoute::Ssh(_))
        {
            if up {
                self.ssh.scroll_up(amount);
            } else {
                self.ssh.scroll_down(amount);
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
        } else if self.activity_details_focused() {
            if up {
                self.scroll_activity_details_up(amount);
            } else {
                self.scroll_activity_details_down(amount);
            }
        } else if self.activity_list_focused() {
            self.move_activity_selection_by(amount, !up);
        } else if self.active_workspace == Workspace::Ops
            && self.ops.active() == OpsDestination::Stream
        {
            if up {
                self.stream.scroll_up(amount);
            } else {
                self.stream.scroll_down(amount);
            }
        }
    }

    fn click_region(&mut self, x: u16, y: u16, event_tx: Option<&TuiEventSender>) {
        let hit = self.geometry.hit_test(x, y);
        self.click_hit(hit, x, y, event_tx);
    }

    fn click_region_below_splits(&mut self, x: u16, y: u16, event_tx: Option<&TuiEventSender>) {
        let hit = self.geometry.hit_test_below_splits(x, y);
        self.click_hit(hit, x, y, event_tx);
    }

    fn click_hit(
        &mut self,
        hit: Option<RegionKind>,
        x: u16,
        y: u16,
        event_tx: Option<&TuiEventSender>,
    ) {
        if self.mode == InteractionMode::Palette {
            match hit {
                Some(RegionKind::PaletteAll) => self.set_palette_scope(PaletteScope::All),
                Some(RegionKind::PaletteContext) => self.set_palette_scope(PaletteScope::Context),
                _ => {}
            }
            return;
        }
        if self.mode == InteractionMode::QuitBlocked {
            match hit {
                Some(RegionKind::QuitReturn) => {
                    self.quit_choice = QuitChoice::Return;
                    self.close_transient_mode();
                }
                Some(RegionKind::QuitForce) => {
                    self.quit_choice = QuitChoice::ForceQuit;
                    self.should_quit = true;
                }
                _ => {}
            }
            return;
        }
        match hit {
            Some(RegionKind::OpsControl { group, choice }) => {
                self.activate_ops_control(group, choice, event_tx)
            }
            Some(RegionKind::OpsLink(destination)) => {
                self.open_related_destination(destination, event_tx)
            }
            Some(RegionKind::AgentSsh) => self.open_agent_ssh(event_tx),
            Some(RegionKind::AgentSshInput) if self.ssh.status == SshStatus::Ready => {
                self.mode.replace(InteractionMode::SshInput)
            }
            Some(RegionKind::OpsTab(destination)) => {
                self.restore_default_mode();
                self.select_ops_destination(destination, event_tx);
            }
            Some(RegionKind::Workspace(Workspace::Ops)) => {
                self.restore_default_mode();
                self.open_ops_workspace(event_tx);
            }
            Some(RegionKind::Workspace(workspace)) => {
                self.restore_default_mode();
                self.active_workspace = workspace;
            }
            Some(RegionKind::DevSessionStatus(view)) => {
                self.toggle_dev_session_status(view);
            }
            Some(RegionKind::DevViewTitle(view)) => {
                self.active_workspace = Workspace::Dev;
                self.dev_focus = view;
                self.mode.replace(InteractionMode::Normal);
            }
            Some(RegionKind::DevViewBody(view)) => {
                self.active_workspace = Workspace::Dev;
                self.dev_focus = view;
                if self.focused_terminal_is_running() {
                    self.enter_terminal_input();
                } else {
                    self.mode.replace(InteractionMode::Normal);
                }
            }
            Some(RegionKind::OpsList) if self.ops_agents_focused() => {
                self.agents.focus = AgentOverviewFocus::List;
                self.select_agent_at_row(y);
            }
            Some(RegionKind::OpsList) if self.ops_activity_focused() => {
                self.activity.focus = ActivityFocus::List;
                self.select_activity_at_row(y);
            }
            Some(RegionKind::OplogMap) if self.ops_activity_focused() => {
                if let Some(area) = self.geometry.region(RegionKind::OplogMap) {
                    let count = self.activity_visible_len();
                    if count > 0 {
                        let prefix_width =
                            (UnicodeWidthStr::width("loaded map ") as u16).min(area.width);
                        let map_x = area.x.saturating_add(prefix_width);
                        let map_width = area.width.saturating_sub(prefix_width).max(1);
                        let offset = x.saturating_sub(map_x) as usize;
                        self.activity.selected = offset
                            .saturating_mul(count.saturating_sub(1))
                            .checked_div(map_width.saturating_sub(1).max(1) as usize)
                            .unwrap_or(0)
                            .min(count.saturating_sub(1));
                        self.activity.focus = ActivityFocus::List;
                        self.activity.details_scroll = 0;
                        self.activity.reveal_selection(count);
                    }
                }
            }
            Some(RegionKind::OpsList)
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Deployments =>
            {
                self.select_deployment_at_row(y)
            }
            Some(RegionKind::OpsDetails) if self.ops_agents_focused() => {
                self.agents.focus = AgentOverviewFocus::Details
            }
            Some(RegionKind::OpsDetails) if self.ops_activity_focused() => {
                self.activity.focus = ActivityFocus::Details
            }
            Some(RegionKind::MetricsFamilies)
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Metrics =>
            {
                self.select_metrics_family_at(y, event_tx)
            }
            Some(RegionKind::MetricsSeries)
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Metrics =>
            {
                self.select_metrics_series_at(y)
            }
            Some(RegionKind::MetricsSeriesToggle(index))
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Metrics =>
            {
                self.select_metrics_series(index);
                self.toggle_metrics_series(event_tx);
            }
            Some(RegionKind::ResourceKinds)
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Configuration =>
            {
                self.select_resource_kind_at(y, event_tx)
            }
            Some(RegionKind::ResourceItems)
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Configuration =>
            {
                self.select_resource_row_at(y)
            }
            Some(RegionKind::ResourceDetails)
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Configuration =>
            {
                self.resources.focus_details()
            }
            Some(RegionKind::OverviewItem(index))
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Overview
                    && matches!(self.ops.current_route(), OpsRoute::Root) =>
            {
                self.overview.selected_item = index.min(OverviewItem::ALL.len() - 1);
                self.open_selected_overview_item(event_tx);
            }
            Some(RegionKind::OverviewIssue(index))
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Overview
                    && matches!(self.ops.current_route(), OpsRoute::OverviewIssues) =>
            {
                self.overview.selected_issue =
                    index.min(self.overview_issues().len().saturating_sub(1));
                self.open_selected_overview_issue(event_tx);
            }
            Some(RegionKind::MetricsChart)
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Metrics =>
            {
                self.select_metrics_chart_at(x)
            }
            Some(RegionKind::MetricsPanel(index))
                if self.active_workspace == Workspace::Ops
                    && self.ops.active() == OpsDestination::Metrics =>
            {
                self.metrics.selected_panel = index;
                self.open_selected_metric_panel();
            }
            Some(RegionKind::ContextPickerRow(index)) => {
                self.context_switcher.selected = index;
                self.select_context_picker_row(event_tx);
            }
            Some(RegionKind::ContextConfirm) => self.confirm_context_switch(event_tx),
            Some(RegionKind::ContextCancel) => {
                self.context_switcher.pending_action = None;
                self.context_switcher.waiting_for_dev_stop = false;
                self.mode.replace(InteractionMode::ContextPicker);
            }
            _ => {}
        }
    }

    fn drag_target_at(&self, x: u16, y: u16) -> Option<DragTarget> {
        self.geometry
            .hit_test(x, y)
            .and_then(|region| match region {
                RegionKind::DevPrimarySplit => Some(DragTarget::DevPrimary),
                RegionKind::DevSecondarySplit => Some(DragTarget::DevSecondary),
                RegionKind::OpsDetailsSplit => Some(DragTarget::OpsDetails),
                _ => None,
            })
    }

    fn drag_layout(&mut self, x: u16, y: u16) {
        let Some(target) = self.layout.dragging else {
            return;
        };
        let Some(snapshot) = self.geometry.snapshot() else {
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
                let area = self
                    .ops_details_layout_area()
                    .unwrap_or(snapshot.workspace_body);
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
        let Some(list_area) = self.geometry.region(RegionKind::OpsList) else {
            return;
        };
        let filtered = self.filtered_agents();
        if filtered.is_empty() {
            return;
        }
        let query_rows = CollectionQueryBar {
            dataset: &self.agents.dataset_label(),
            mode: self.agents.mode.label(),
            scope: self.agents.query_scope.label(),
            query: &self.agents.query,
            matched: filtered.len(),
            loaded: self.agents.agents.len(),
            more_available: self.agents.has_more(),
            editing: self.mode == InteractionMode::AgentFilter,
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

    fn select_activity_at_row(&mut self, y: u16) {
        let Some(area) = self.geometry.region(RegionKind::OpsList) else {
            return;
        };
        let data_y = area.y.saturating_add(area.height.min(1));
        if y < data_y {
            return;
        }
        let index = self
            .activity
            .list_first_visible
            .get()
            .saturating_add(y.saturating_sub(data_y) as usize);
        if index < self.activity_visible_len() {
            self.activity.selected = index;
            self.activity.details_scroll = 0;
        }
    }

    fn select_metrics_family_at(&mut self, y: u16, event_tx: Option<&TuiEventSender>) {
        let Some(area) = self.geometry.region(RegionKind::MetricsFamilies) else {
            return;
        };
        let index = self
            .metrics
            .families_first_visible
            .get()
            .saturating_add(y.saturating_sub(area.y) as usize);
        if index >= self.metrics.families.len() {
            return;
        }
        self.select_metrics_family(index, event_tx);
    }

    fn select_metrics_series_at(&mut self, y: u16) {
        let Some(area) = self.geometry.region(RegionKind::MetricsSeries) else {
            return;
        };
        let index = self
            .metrics
            .series_first_visible
            .get()
            .saturating_add(y.saturating_sub(area.y) as usize);
        if index >= self.metrics.series.len() {
            return;
        }
        self.select_metrics_series(index);
    }

    fn select_resource_kind_at(&mut self, y: u16, event_tx: Option<&TuiEventSender>) {
        let Some(area) = self.geometry.region(RegionKind::ResourceKinds) else {
            return;
        };
        let index = self
            .resources
            .kind_first_visible()
            .saturating_add(y.saturating_sub(area.y) as usize);
        if index >= resources::ResourceKind::for_scope(self.resources.scope).len() {
            return;
        }
        if self.resources.focus_kind(index) {
            self.refresh_resources(event_tx, false);
        }
    }

    fn select_resource_row_at(&mut self, y: u16) {
        let Some(area) = self.geometry.region(RegionKind::ResourceItems) else {
            return;
        };
        let index = self
            .resources
            .row_first_visible()
            .saturating_add(y.saturating_sub(area.y) as usize);
        if index >= self.resources.visible_rows().len() {
            return;
        }
        self.resources.focus_row(index);
    }

    fn select_deployment_at_row(&mut self, y: u16) {
        let Some(area) = self.geometry.region(RegionKind::OpsList) else {
            return;
        };
        let data_y = area.y.saturating_add(1);
        if y < data_y {
            return;
        }
        let index = self
            .deployments
            .table_first_visible
            .get()
            .saturating_add(y.saturating_sub(data_y) as usize);
        match self.ops.current_route() {
            OpsRoute::Root => {
                if index < self.deployments.history.len().min(RECENT_LIMIT) {
                    self.deployments.selected = index;
                }
            }
            OpsRoute::DeploymentHistory => {
                if index < self.deployments.history.len() {
                    self.deployments.selected = index;
                }
            }
            OpsRoute::DeploymentCompare(route) => {
                let count = self
                    .deployments
                    .changes(route.base_revision, route.target_revision)
                    .map_or(0, |changes| changes.len());
                if index < count {
                    self.deployments.selected_change = index;
                }
            }
            _ => {}
        }
    }

    fn execute_action(&mut self, action: ActionId, event_tx: Option<&TuiEventSender>) {
        match action {
            ActionId::SelectDev => {
                self.close_palette();
                self.active_workspace = Workspace::Dev;
            }
            ActionId::SelectOps => {
                self.close_palette();
                self.open_ops_workspace(event_tx);
            }
            ActionId::Plan => {
                self.close_palette();
                self.open_command_form(CommandKind::Plan);
            }
            ActionId::Build => {
                self.close_palette();
                self.open_command_form(CommandKind::Build);
            }
            ActionId::Deploy => {
                self.close_palette();
                self.open_command_form(CommandKind::Deploy);
            }
            ActionId::Clean => {
                self.close_palette();
                self.open_command_form(CommandKind::Clean);
            }
            ActionId::SelectDevBuildDeploy => {
                self.close_palette();
                self.open_dev_workspace(DevView::BuildDeploy);
            }
            ActionId::SelectDevServer => {
                self.close_palette();
                self.open_dev_workspace(DevView::Server);
            }
            ActionId::SelectDevRepl => {
                self.close_palette();
                self.open_dev_workspace(DevView::Repl);
            }
            ActionId::StartServer => {
                self.close_palette();
                self.open_server_form(false, false);
            }
            ActionId::StopServer | ActionId::ForceStopServer => {
                self.close_palette();
                self.stop_server();
            }
            ActionId::RestartServer => {
                self.close_palette();
                self.open_server_form(true, false);
            }
            ActionId::FlipPanePlacement => {
                self.layout.dev_placement = self.layout.dev_placement.flipped();
                self.close_palette();
            }
            ActionId::ToggleBuildDeployView => {
                self.toggle_dev_view(DevView::BuildDeploy);
                self.close_palette();
            }
            ActionId::ToggleServerView => {
                self.toggle_dev_view(DevView::Server);
                self.close_palette();
            }
            ActionId::ToggleReplView => {
                self.toggle_dev_view(DevView::Repl);
                self.close_palette();
            }
            ActionId::MoveCurrentDevViewToMain => {
                self.make_focused_dev_view_primary();
                self.close_palette();
            }
            ActionId::RefreshAgents => {
                self.close_palette();
                self.refresh_agents(event_tx);
            }
            ActionId::OpenAgentDatasetFilter => {
                self.close_palette();
                self.open_agent_dataset_filter();
            }
            ActionId::OpenAgentColumns => {
                self.close_palette();
                self.agents.column_chooser = Some(ColumnChooserState::new(&self.agents.table));
                self.mode.replace(InteractionMode::AgentColumns);
            }
            ActionId::IncludeAgentMatches => {
                self.include_filtered_agents();
                self.close_palette();
            }
            ActionId::ExcludeAgentMatches => {
                self.exclude_filtered_agents();
                self.close_palette();
            }
            ActionId::LoadMoreAgents => {
                self.close_palette();
                self.load_more_agents(event_tx);
            }
            ActionId::OpenContextPicker => {
                self.palette.reset();
                self.open_context_picker();
            }
            ActionId::ToggleAgentAutoRefresh => {
                self.toggle_agent_auto_refresh(event_tx);
                self.close_palette();
            }
            ActionId::ToggleAgentDetails => {
                self.toggle_agent_details();
                self.close_palette();
            }
            ActionId::ToggleAgentStream => {
                self.close_palette();
                self.toggle_stream_session(event_tx);
            }
            ActionId::OpenAgentScopeInOplog => {
                self.close_palette();
                self.open_agent_scope_in_oplog(event_tx);
            }
            ActionId::OpenAgentScopeInMetrics => {
                self.close_palette();
                self.open_agent_scope_in_metrics(event_tx);
            }
            ActionId::OpenAgentSsh => {
                self.close_palette();
                self.open_agent_ssh(event_tx);
            }
            ActionId::UseEnvironmentMetricsScope => {
                self.close_palette();
                self.use_environment_metrics_scope(event_tx);
            }
            ActionId::CycleAgentMode => {
                self.cycle_agent_mode(event_tx);
                self.close_palette();
            }
            ActionId::NextOpsDestination => {
                self.ops.select_next();
                self.refresh_active_ops_destination(event_tx);
                self.close_palette();
            }
            ActionId::PreviousOpsDestination => {
                self.ops.select_previous();
                self.refresh_active_ops_destination(event_tx);
                self.close_palette();
            }
            ActionId::CycleAgentFindScope => {
                self.agents.query_scope = self.agents.query_scope.next();
                self.agents.reset_selection();
                self.close_palette();
            }
            ActionId::SelectOpsOverview => {
                self.close_palette();
                self.select_ops_destination(OpsDestination::Overview, event_tx);
            }
            ActionId::SelectOpsAgents => {
                self.close_palette();
                self.select_ops_destination(OpsDestination::Agents, event_tx);
            }
            ActionId::SelectOpsOplog => {
                self.close_palette();
                self.select_ops_destination(OpsDestination::Oplog, event_tx);
            }
            ActionId::SelectOpsStream => {
                self.close_palette();
                self.select_ops_destination(OpsDestination::Stream, event_tx);
            }
            ActionId::SelectOpsMetrics => {
                self.close_palette();
                self.select_ops_destination(OpsDestination::Metrics, event_tx);
            }
            ActionId::SelectOpsDeployments => {
                self.close_palette();
                self.select_ops_destination(OpsDestination::Deployments, event_tx);
            }
            ActionId::SelectOpsConfiguration => {
                self.close_palette();
                self.select_ops_destination(OpsDestination::Configuration, event_tx);
            }
            ActionId::StartRepl => {
                self.close_palette();
                self.open_repl_form(false);
            }
            ActionId::StopRepl | ActionId::ForceStopRepl => {
                self.close_palette();
                self.stop_repl();
            }
            ActionId::RestartRepl => {
                self.close_palette();
                self.open_repl_form(true);
            }
            ActionId::OpenPalette => {
                self.open_palette(self.palette.scope);
            }
            ActionId::ShowHelp => {
                self.palette.reset();
                self.open_help();
            }
            ActionId::Quit => {
                self.close_palette();
                self.request_quit();
            }
        }
    }

    fn execute_palette_action(&mut self, action_id: ActionId, event_tx: Option<&TuiEventSender>) {
        let metadata = action(action_id);

        if self.action_availability(metadata).is_unavailable() {
            return;
        }

        self.execute_action(action_id, event_tx);
    }

    fn action_availability(&self, action: &ActionDefinition) -> ActionAvailability {
        match action.id {
            ActionId::Plan | ActionId::Build | ActionId::Deploy | ActionId::Clean
                if let Err(reason) = self
                    .selected_target
                    .context
                    .capability(DevView::BuildDeploy) =>
            {
                ActionAvailability::Unavailable(reason)
            }
            ActionId::StartRepl | ActionId::RestartRepl
                if let Err(reason) = self.selected_target.context.capability(DevView::Repl) =>
            {
                ActionAvailability::Unavailable(reason)
            }
            ActionId::Plan | ActionId::Build | ActionId::Deploy | ActionId::Clean
                if self.command_is_running() =>
            {
                ActionAvailability::Unavailable("command already running")
            }
            ActionId::RefreshAgents | ActionId::LoadMoreAgents if self.agents.refresh_running => {
                ActionAvailability::Unavailable("refresh already running")
            }
            ActionId::RefreshAgents | ActionId::LoadMoreAgents
                if self.context_executor.is_none() =>
            {
                ActionAvailability::Unavailable("context executor unavailable")
            }
            ActionId::LoadMoreAgents if !self.agents.has_more() => {
                ActionAvailability::Unavailable("no additional agent batch")
            }
            ActionId::IncludeAgentMatches | ActionId::ExcludeAgentMatches
                if self.filtered_agents().is_empty() =>
            {
                ActionAvailability::Unavailable("no loaded agents match")
            }
            ActionId::ToggleAgentStream if self.stream_target_candidate().is_none() => {
                ActionAvailability::Unavailable("select one exact Agent")
            }
            ActionId::OpenAgentSsh if self.stream_target_candidate().is_none() => {
                ActionAvailability::Unavailable("select one exact Agent")
            }
            ActionId::OpenAgentSsh
                if matches!(self.ssh.status, SshStatus::Connecting | SshStatus::Running)
                    && self.stream_target_candidate().as_ref() != self.ssh.target.as_ref() =>
            {
                ActionAvailability::Unavailable("another Agent SSH command is active")
            }
            ActionId::OpenAgentScopeInOplog | ActionId::OpenAgentScopeInMetrics
                if self.agent_handoff_scope().is_empty() =>
            {
                ActionAvailability::Unavailable("select or include at least one Agent")
            }
            ActionId::OpenAgentScopeInOplog | ActionId::OpenAgentScopeInMetrics
                if self.agent_handoff_scope().len() > activity::HARD_AGENT_LIMIT =>
            {
                ActionAvailability::Unavailable("include at most 20 Agents")
            }
            ActionId::OpenContextPicker if self.context_switch_busy() => {
                ActionAvailability::Unavailable("context switch already running")
            }
            ActionId::StartServer
            | ActionId::StopServer
            | ActionId::ForceStopServer
            | ActionId::RestartServer
                if !self.server.available
                    || self
                        .selected_target
                        .context
                        .capability(DevView::Server)
                        .is_err() =>
            {
                ActionAvailability::Unavailable("requires an available local server target")
            }
            ActionId::ToggleBuildDeployView
                if self.dev_view_visible(DevView::BuildDeploy)
                    && self.layout.visible_dev_views(self.dev_primary).len() == 1 =>
            {
                ActionAvailability::Unavailable("at least one Dev view must remain visible")
            }
            ActionId::ToggleServerView
                if self.dev_view_visible(DevView::Server)
                    && self.layout.visible_dev_views(self.dev_primary).len() == 1 =>
            {
                ActionAvailability::Unavailable("at least one Dev view must remain visible")
            }
            ActionId::ToggleReplView
                if self.dev_view_visible(DevView::Repl)
                    && self.layout.visible_dev_views(self.dev_primary).len() == 1 =>
            {
                ActionAvailability::Unavailable("at least one Dev view must remain visible")
            }
            ActionId::MoveCurrentDevViewToMain if self.dev_focus == self.dev_primary => {
                ActionAvailability::Unavailable("focused view is already main")
            }
            ActionId::ForceStopServer
                if !force_stop_available(self.server.run.stop_requested_at) =>
            {
                ActionAvailability::Unavailable("available five seconds after stop is requested")
            }
            ActionId::ForceStopRepl if !force_stop_available(self.repl.run.stop_requested_at) => {
                ActionAvailability::Unavailable("available five seconds after stop is requested")
            }
            _ => ActionAvailability::Available,
        }
    }

    fn open_palette(&mut self, scope: PaletteScope) {
        match self.mode.current() {
            InteractionMode::Palette => {}
            InteractionMode::Leader => {
                self.palette.reset();
                self.mode.replace(InteractionMode::Palette);
            }
            _ => {
                self.palette.reset();
                self.mode.push(InteractionMode::Palette);
            }
        }
        self.set_palette_scope(scope);
    }

    fn set_palette_scope(&mut self, scope: PaletteScope) {
        self.palette.scope = scope;
        self.palette.selected = 0;
    }

    fn open_jobs_palette(&mut self) {
        self.selected_job = 0;
        self.open_transient_mode(InteractionMode::Jobs);
    }

    fn filtered_actions(&self) -> Vec<ActionDefinition> {
        filtered_actions(&self.palette.query, self.palette.scope, self)
    }

    fn palette_matches(&self) -> Vec<PaletteMatch> {
        ranked_palette_actions(&self.palette.query, self.palette.scope, self)
    }

    fn open_leader(&mut self) {
        self.leader.group = None;
        self.mode.push(InteractionMode::Leader);
    }

    fn close_leader(&mut self) {
        let fallback = self.default_mode();
        self.mode.pop_or(fallback);
        self.normalize_mode();
        self.leader.group = None;
    }

    fn open_context_picker(&mut self) {
        self.context_switcher.last_error = None;
        self.context_switcher.clamp_selection();
        self.open_transient_mode(InteractionMode::ContextPicker);
    }

    fn close_palette(&mut self) {
        self.palette.reset();
        let fallback = self.default_mode();
        self.mode.pop_or(fallback);
        self.normalize_mode();
    }

    fn default_mode(&self) -> InteractionMode {
        InteractionMode::Normal
    }

    fn focused_nested_cli_target(&self) -> Option<NestedCliTarget> {
        if self.active_workspace != Workspace::Dev {
            return None;
        }
        match self.dev_focus {
            DevView::BuildDeploy if self.command_is_running() => Some(NestedCliTarget::Command),
            DevView::Server if self.server.run.is_running() => Some(NestedCliTarget::Server),
            DevView::Repl if self.repl.is_running() => Some(NestedCliTarget::Repl),
            _ => None,
        }
    }

    fn focused_terminal_is_running(&self) -> bool {
        self.focused_nested_cli_target().is_some()
    }

    fn terminal_input_owns(&self, target: NestedCliTarget) -> bool {
        self.mode == InteractionMode::TerminalInput
            && self.focused_nested_cli_target() == Some(target)
    }

    fn enter_terminal_input(&mut self) {
        if self.focused_terminal_is_running() {
            self.scroll_terminal_bottom();
            self.mode.replace(InteractionMode::TerminalInput);
        }
    }

    fn restore_default_mode(&mut self) {
        let mode = self.default_mode();
        self.mode.reset(mode);
    }

    fn open_transient_mode(&mut self, mode: InteractionMode) {
        match self.mode.current() {
            InteractionMode::Leader | InteractionMode::Palette => self.mode.replace(mode),
            _ => self.mode.push(mode),
        }
    }

    fn close_transient_mode(&mut self) {
        let fallback = self.default_mode();
        self.mode.pop_or(fallback);
        self.normalize_mode();
    }

    fn normalize_mode(&mut self) {
        let valid = match self.mode.current() {
            InteractionMode::CommandForm => self.command_form.is_some(),
            InteractionMode::SessionForm => self.session_form.is_some(),
            InteractionMode::MetricsRange => self.metrics_range_form.is_some(),
            _ => true,
        };
        if !valid {
            self.restore_default_mode();
        }
    }

    fn next_workspace(&mut self) {
        self.active_workspace =
            Workspace::from_index((self.active_workspace.index() + 1) % Workspace::ALL.len());
    }

    fn previous_workspace(&mut self) {
        self.active_workspace = Workspace::from_index(
            (self.active_workspace.index() + Workspace::ALL.len() - 1) % Workspace::ALL.len(),
        );
    }

    fn focus_next_pane(&mut self) {
        match self.active_workspace {
            Workspace::Dev => {
                let visible = self.layout.visible_dev_views(self.dev_primary);
                let current = visible
                    .iter()
                    .position(|view| *view == self.dev_focus)
                    .unwrap_or(0);
                self.dev_focus = visible[(current + 1) % visible.len()];
            }
            Workspace::Ops
                if self.ops.active() == OpsDestination::Metrics
                    && matches!(self.ops.current_route(), OpsRoute::Root)
                    && self.metrics.mode == MetricsMode::Explore =>
            {
                self.metrics.focus = self.metrics.focus.next()
            }
            Workspace::Ops if self.ops.active() == OpsDestination::Configuration => {
                self.resources.cycle_focus(true)
            }
            Workspace::Ops => self.cycle_ops_pane_focus(),
        }
    }

    fn focus_previous_pane(&mut self) {
        match self.active_workspace {
            Workspace::Dev => {
                let visible = self.layout.visible_dev_views(self.dev_primary);
                let current = visible
                    .iter()
                    .position(|view| *view == self.dev_focus)
                    .unwrap_or(0);
                self.dev_focus = visible[(current + visible.len() - 1) % visible.len()];
            }
            Workspace::Ops
                if self.ops.active() == OpsDestination::Metrics
                    && matches!(self.ops.current_route(), OpsRoute::Root)
                    && self.metrics.mode == MetricsMode::Explore =>
            {
                self.metrics.focus = self.metrics.focus.previous()
            }
            Workspace::Ops if self.ops.active() == OpsDestination::Configuration => {
                self.resources.cycle_focus(false)
            }
            Workspace::Ops => self.cycle_ops_pane_focus(),
        }
    }

    fn open_selected_agent(&mut self) {
        let identity = self
            .filtered_agents()
            .get(self.agents.selected)
            .map(|agent| agent.identity());
        if let Some(identity) = identity {
            self.ops.open_agent(identity);
            self.agents.details_scroll = 0;
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
                Workspace::Dev => self.layout.dev_placement = self.layout.dev_placement.flipped(),
                Workspace::Ops => {
                    self.layout.ops_details_placement = self.layout.ops_details_placement.flipped()
                }
            },
            KeyCode::Char('b') if self.active_workspace == Workspace::Dev => {
                self.toggle_dev_view(DevView::BuildDeploy);
            }
            KeyCode::Char('s') if self.active_workspace == Workspace::Dev => {
                self.toggle_dev_view(DevView::Server);
            }
            KeyCode::Char('r') if self.active_workspace == Workspace::Dev => {
                self.toggle_dev_view(DevView::Repl);
            }
            KeyCode::Char('m') if self.active_workspace == Workspace::Dev => {
                self.make_focused_dev_view_primary()
            }
            KeyCode::Char('d') if self.active_workspace == Workspace::Ops => {
                match self.ops.active() {
                    OpsDestination::Agents => self.toggle_agent_details(),
                    OpsDestination::Oplog => self.toggle_activity_details(),
                    _ => {}
                }
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
            KeyCode::Esc => self.close_transient_mode(),
            KeyCode::Up => self.selected_job = self.selected_job.saturating_sub(1),
            KeyCode::Down => {
                self.selected_job = self
                    .selected_job
                    .saturating_add(1)
                    .min(count.saturating_sub(1))
            }
            KeyCode::Enter if count > 0 => {
                self.open_dev_workspace(DevView::BuildDeploy);
                self.close_transient_mode();
            }
            _ => {}
        }
    }

    fn handle_quit_blocked_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.close_transient_mode(),
            KeyCode::Left | KeyCode::BackTab => self.quit_choice = self.quit_choice.previous(),
            KeyCode::Right | KeyCode::Tab => self.quit_choice = self.quit_choice.next(),
            KeyCode::Enter => match self.quit_choice {
                QuitChoice::Return => self.close_transient_mode(),
                QuitChoice::ForceQuit => self.should_quit = true,
            },
            _ => {}
        }
    }

    fn request_quit(&mut self) {
        if self.has_active_work() {
            self.quit_choice = QuitChoice::Return;
            self.mode.push(InteractionMode::QuitBlocked);
        } else {
            self.should_quit = true;
        }
    }

    fn has_active_work(&self) -> bool {
        self.command_is_running()
            || self.server.run.is_running()
            || self.repl.is_running()
            || matches!(self.ssh.status, SshStatus::Connecting | SshStatus::Running)
    }

    fn job_count(&self) -> usize {
        usize::from(self.command_run.is_some()) + self.completed_jobs.len()
    }

    fn dev_view_visible(&self, view: DevView) -> bool {
        view == self.dev_primary || self.layout.dev_is_visible(view)
    }

    fn toggle_dev_view(&mut self, view: DevView) -> bool {
        if !self.dev_view_visible(view) {
            self.layout.set_dev_visible(view, true);
            self.dev_focus = view;
            return true;
        }

        let visible = self.layout.visible_dev_views(self.dev_primary);
        if visible.len() == 1 {
            return false;
        }

        if view == self.dev_primary {
            let replacement = if self.dev_focus != view && self.dev_view_visible(self.dev_focus) {
                self.dev_focus
            } else {
                DevView::ALL
                    .into_iter()
                    .find(|candidate| *candidate != view && self.dev_view_visible(*candidate))
                    .expect("multiple visible Dev views require a replacement primary")
            };
            self.layout.set_dev_visible(view, false);
            self.layout.set_dev_visible(replacement, true);
            self.dev_primary = replacement;
            if self.dev_focus == view {
                self.dev_focus = replacement;
            }
        } else {
            self.layout.set_dev_visible(view, false);
            if self.dev_focus == view {
                self.dev_focus = self.dev_primary;
            }
        }
        true
    }

    fn make_focused_dev_view_primary(&mut self) {
        if self.active_workspace != Workspace::Dev || self.dev_focus == self.dev_primary {
            return;
        }
        let old_primary = self.dev_primary;
        self.layout.set_dev_visible(old_primary, true);
        self.layout.set_dev_visible(self.dev_focus, true);
        self.dev_primary = self.dev_focus;
    }

    fn move_layout_divider(&mut self, direction: KeyCode, cells: u16) {
        let Some(workspace_area) = self.geometry.workspace_body() else {
            return;
        };
        match self.active_workspace {
            Workspace::Ops if self.agent_details_open() || self.activity_details_open() => {
                let area = self.ops_details_layout_area().unwrap_or(workspace_area);
                let placement = self.layout.ops_details_placement;
                let delta = physical_ratio_delta(area, placement, direction, cells);
                if delta != 0 {
                    let requested = apply_ratio_delta(self.layout.ops_details_ratio, delta);
                    self.layout.ops_details_ratio =
                        layout::clamp_ops_details_ratio(area, placement, requested);
                }
            }
            Workspace::Dev => {
                let area = workspace_area;
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
            Workspace::Ops => {}
        }
    }

    fn open_dev_workspace(&mut self, panel: DevView) {
        self.active_workspace = Workspace::Dev;
        self.layout.set_dev_visible(panel, true);
        self.dev_focus = panel;
    }

    fn toggle_dev_session_status(&mut self, view: DevView) {
        let hide_focused_view = self.dev_panel_focused(view);
        self.restore_default_mode();
        if !hide_focused_view || !self.toggle_dev_view(view) {
            self.open_dev_workspace(view);
        }
    }

    fn open_ops_workspace(&mut self, event_tx: Option<&TuiEventSender>) {
        self.active_workspace = Workspace::Ops;
        self.refresh_active_ops_destination(event_tx);
    }

    fn select_ops_destination(
        &mut self,
        destination: OpsDestination,
        event_tx: Option<&TuiEventSender>,
    ) {
        self.active_workspace = Workspace::Ops;
        self.ops.select(destination);
        if destination == OpsDestination::Stream {
            self.stream.mark_visible();
        }
        self.refresh_active_ops_destination(event_tx);
    }

    fn refresh_active_ops_destination(&mut self, event_tx: Option<&TuiEventSender>) {
        match self.ops.active() {
            OpsDestination::Overview => {
                self.refresh_overview(event_tx, false);
            }
            OpsDestination::Deployments => {
                if self.ops_environment.snapshot.is_none() && !self.ops_environment.loading {
                    self.refresh_ops_environment(event_tx);
                }
                if self.deployments.history.is_empty() && !self.deployments.loading {
                    self.refresh_deployment_history(event_tx);
                }
            }
            OpsDestination::Agents => {
                if self.agents.agents.is_empty() && !self.agents.refresh_running {
                    self.refresh_agents(event_tx);
                }
            }
            OpsDestination::Oplog => self.refresh_activity(event_tx),
            OpsDestination::Metrics => self.refresh_metrics(event_tx),
            OpsDestination::Configuration => self.refresh_resources(event_tx, false),
            OpsDestination::Stream => self.stream.mark_visible(),
        }
    }

    fn stream_target_candidate(&self) -> Option<AgentIdentity> {
        if self.ops.active() == OpsDestination::Stream {
            return self
                .ops
                .current_stream()
                .cloned()
                .or_else(|| self.stream.target.clone());
        }
        if self.ops.active() == OpsDestination::Agents {
            if let Some(agent) = self.ops.current_agent() {
                return Some(agent.clone());
            }
            return match self.agents.included.len() {
                0 => self
                    .filtered_agents()
                    .get(self.agents.selected)
                    .map(|agent| agent.identity()),
                1 => self.agents.included.iter().next().cloned(),
                _ => None,
            };
        }
        if self.ops.active() == OpsDestination::Oplog {
            return self
                .ops
                .current_oplog()
                .and_then(|route| (route.agents.len() == 1).then(|| route.agents[0].clone()));
        }
        self.stream.target.clone()
    }

    fn agent_handoff_scope(&self) -> Vec<AgentIdentity> {
        if let Some(agent) = self.ops.current_agent() {
            return vec![agent.clone()];
        }
        let mut agents = if self.agents.included.is_empty() {
            self.filtered_agents()
                .get(self.agents.selected)
                .map(|agent| vec![agent.identity()])
                .unwrap_or_default()
        } else {
            self.agents.included.iter().cloned().collect::<Vec<_>>()
        };
        agents.sort_by(|left, right| {
            left.component
                .cmp(&right.component)
                .then_with(|| left.agent_id.cmp(&right.agent_id))
        });
        agents
    }

    fn activity_selected_agent(&self) -> Option<AgentIdentity> {
        match self.activity.representation {
            ActivityRepresentation::Entries => self
                .activity_filtered_entries()
                .get(self.activity.selected)
                .map(|entry| AgentIdentity {
                    component: entry.component.clone(),
                    agent_id: entry.agent_id.clone(),
                }),
            ActivityRepresentation::Grouped => self
                .activity_groups()
                .get(self.activity.selected)
                .map(|group| AgentIdentity {
                    component: group.component.clone(),
                    agent_id: group.agent_id.clone(),
                }),
        }
    }

    fn related_exact_agent(&self) -> Option<AgentIdentity> {
        match self.ops.active() {
            OpsDestination::Agents => self.stream_target_candidate(),
            OpsDestination::Oplog => self.activity_selected_agent().or_else(|| {
                self.ops
                    .current_oplog()
                    .and_then(|route| (route.agents.len() == 1).then(|| route.agents[0].clone()))
            }),
            OpsDestination::Stream => self
                .ops
                .current_stream()
                .cloned()
                .or_else(|| self.stream.target.clone()),
            OpsDestination::Metrics => self.metric_selected_agent(),
            _ => None,
        }
    }

    fn metric_selected_agent(&self) -> Option<AgentIdentity> {
        if self.metrics.source != crate::model::local_observability::LocalMetricSource::Agents
            || self.metrics.mode != MetricsMode::Explore
            || matches!(self.ops.current_route(), OpsRoute::MetricsPanel(_))
        {
            return None;
        }
        if self.metrics.agent_scope.len() == 1 {
            return self.metrics.agent_scope.first().cloned();
        }
        let agent_id = self
            .metrics
            .series
            .get(self.metrics.selected_series)?
            .agent_id
            .as_deref()?;
        let mut matches = self
            .agents
            .agents
            .iter()
            .filter(|agent| agent.agent_id == agent_id)
            .map(AgentListItem::identity);
        let agent = matches.next()?;
        matches.next().is_none().then_some(agent)
    }

    fn stream_handoff_available(&self, agent: &AgentIdentity) -> bool {
        !self.stream_session_active() || self.stream.target.as_ref() == Some(agent)
    }

    fn related_stream_available(&self) -> bool {
        self.related_exact_agent()
            .is_some_and(|agent| self.stream_handoff_available(&agent))
    }

    fn ops_handoff_scope(&self) -> Vec<AgentIdentity> {
        match self.ops.active() {
            OpsDestination::Agents => self.agent_handoff_scope(),
            OpsDestination::Oplog => self
                .ops
                .current_oplog()
                .map(|route| route.agents.clone())
                .unwrap_or_default(),
            OpsDestination::Stream => self.related_exact_agent().into_iter().collect(),
            OpsDestination::Metrics
                if self.metrics.source
                    == crate::model::local_observability::LocalMetricSource::Agents =>
            {
                self.metrics.agent_scope.clone()
            }
            _ => Vec::new(),
        }
    }

    fn open_scope_in_oplog(
        &mut self,
        agents: Vec<AgentIdentity>,
        event_tx: Option<&TuiEventSender>,
    ) {
        if agents.is_empty() || agents.len() > activity::HARD_AGENT_LIMIT {
            return;
        }
        self.ops.open_oplog(OplogRoute {
            agents,
            focus: None,
            from_unix_nanos: None,
            to_unix_nanos: None,
        });
        self.invalidate_activity();
        self.refresh_activity(event_tx);
    }

    fn open_scope_in_metrics(
        &mut self,
        agents: Vec<AgentIdentity>,
        event_tx: Option<&TuiEventSender>,
    ) {
        if agents.is_empty() || agents.len() > activity::HARD_AGENT_LIMIT {
            return;
        }
        self.metrics.source = crate::model::local_observability::LocalMetricSource::Agents;
        self.metrics.agent_scope = agents;
        self.active_workspace = Workspace::Ops;
        self.ops.select(OpsDestination::Metrics);
        self.reload_metrics(event_tx);
    }

    fn open_related_destination(
        &mut self,
        destination: OpsDestination,
        event_tx: Option<&TuiEventSender>,
    ) {
        match destination {
            OpsDestination::Agents => {
                let Some(agent) = self.related_exact_agent() else {
                    return;
                };
                self.active_workspace = Workspace::Ops;
                self.ops.open_agent(agent);
            }
            OpsDestination::Oplog if self.ops.active() == OpsDestination::Metrics => {
                self.open_metric_sample_in_oplog(event_tx)
            }
            OpsDestination::Oplog => self.open_scope_in_oplog(self.ops_handoff_scope(), event_tx),
            OpsDestination::Metrics => {
                self.open_scope_in_metrics(self.ops_handoff_scope(), event_tx)
            }
            OpsDestination::Stream => {
                let Some(agent) = self.related_exact_agent() else {
                    return;
                };
                if !self.stream_handoff_available(&agent) {
                    return;
                }
                if self.stream.target.as_ref() != Some(&agent) {
                    self.stream.reset();
                }
                self.active_workspace = Workspace::Ops;
                self.ops.open_stream(agent);
                self.stream.mark_visible();
            }
            OpsDestination::Overview
            | OpsDestination::Deployments
            | OpsDestination::Configuration => {}
        }
    }

    fn activate_ops_control(
        &mut self,
        group: usize,
        choice: usize,
        event_tx: Option<&TuiEventSender>,
    ) {
        match self.ops.active() {
            OpsDestination::Oplog => match (group, choice) {
                (0, 0) => {
                    self.set_activity_representation(ActivityRepresentation::Grouped, event_tx)
                }
                (0, 1) => {
                    self.set_activity_representation(ActivityRepresentation::Entries, event_tx)
                }
                (1, 0) => self.set_activity_order(ActivityOrder::Newest),
                (1, 1) => self.set_activity_order(ActivityOrder::Oldest),
                (2, 0) => {
                    self.activity.input_kind = ActivityInputKind::Loaded;
                    self.mode.replace(InteractionMode::OplogFilter);
                }
                (2, 1) => {
                    self.activity.input_kind = ActivityInputKind::Server;
                    self.mode.replace(InteractionMode::OplogFilter);
                }
                _ => {}
            },
            OpsDestination::Metrics => match (group, choice) {
                (0, 0) => self.set_metrics_mode(MetricsMode::Dashboard, event_tx),
                (0, 1) => self.set_metrics_mode(MetricsMode::Explore, event_tx),
                (1, 0) => self.set_metrics_source(
                    crate::model::local_observability::LocalMetricSource::Agents,
                    event_tx,
                ),
                (1, 1) => self.set_metrics_source(
                    crate::model::local_observability::LocalMetricSource::Services,
                    event_tx,
                ),
                (2, 0) => self.set_metrics_live(true, event_tx),
                (2, 1) => self.set_metrics_live(false, event_tx),
                _ => {}
            },
            OpsDestination::Deployments
                if matches!(self.ops.current_route(), OpsRoute::DeploymentCompare(_)) =>
            {
                if let Some(route) = self.ops.deployment_compare_mut() {
                    route.focus = match (group, choice) {
                        (0, 0) => DeploymentCompareFocus::Base,
                        (0, 1) => DeploymentCompareFocus::Target,
                        _ => route.focus,
                    };
                }
            }
            OpsDestination::Configuration if group == 0 => {
                if let Some(scope) = ResourceScope::ALL.get(choice).copied()
                    && scope != self.resources.scope
                {
                    self.resources.select_scope(scope);
                    self.refresh_resources(event_tx, false);
                }
            }
            OpsDestination::Overview
            | OpsDestination::Agents
            | OpsDestination::Stream
            | OpsDestination::Deployments
            | OpsDestination::Configuration => {}
        }
    }

    fn open_agent_scope_in_oplog(&mut self, event_tx: Option<&TuiEventSender>) {
        self.open_scope_in_oplog(self.agent_handoff_scope(), event_tx);
    }

    fn open_agent_scope_in_metrics(&mut self, event_tx: Option<&TuiEventSender>) {
        self.open_scope_in_metrics(self.agent_handoff_scope(), event_tx);
    }

    fn use_environment_metrics_scope(&mut self, event_tx: Option<&TuiEventSender>) {
        self.metrics.agent_scope.clear();
        self.reload_metrics(event_tx);
    }

    fn stream_session_active(&self) -> bool {
        !matches!(
            self.stream.status,
            StreamStatus::Disconnected | StreamStatus::Failed(_)
        )
    }

    fn toggle_stream_session(&mut self, event_tx: Option<&TuiEventSender>) {
        if self.ops.active() == OpsDestination::Stream && self.stream_session_active() {
            self.stream.disconnect();
            return;
        }
        let Some(target) = self.stream_target_candidate() else {
            self.stream.status = StreamStatus::Failed(
                "Select one exact Agent before connecting the Stream".to_string(),
            );
            return;
        };
        self.ops.open_stream(target.clone());
        self.active_workspace = Workspace::Ops;
        self.start_stream(target, event_tx);
    }

    fn start_stream(&mut self, target: AgentIdentity, event_tx: Option<&TuiEventSender>) {
        let Some(event_tx) = event_tx.cloned() else {
            return;
        };
        let Some(context_executor) = self.context_executor.clone() else {
            self.stream.status =
                StreamStatus::Failed("TUI context executor is not available".to_string());
            return;
        };
        let context_id = context_executor.current_context_id();
        let generation = self.stream.prepare_connect(target.clone(), context_id);
        let control_tx = event_tx.control();
        let output_tx = event_tx.output();
        let auth_presenter = Some(tui_auth_presenter(&event_tx));
        let task = context_executor.spawn(
            event_tx.control(),
            auth_presenter,
            move |launch_context| async move {
                stream::run_session(
                    launch_context.into_context(),
                    target,
                    generation,
                    control_tx,
                    output_tx,
                )
                .await
            },
            move |result| TuiEvent::StreamSessionFinished { generation, result },
        );
        self.stream.attach_task(task);
    }

    fn open_agent_ssh(&mut self, event_tx: Option<&TuiEventSender>) {
        let Some(target) = self.stream_target_candidate() else {
            return;
        };
        self.active_workspace = Workspace::Ops;
        let already_open =
            matches!(self.ops.current_route(), OpsRoute::Ssh(current) if *current == target);
        let retained =
            self.ssh.target.as_ref() == Some(&target) && self.ssh.status != SshStatus::Disconnected;
        if !already_open {
            self.ops.open_ssh(target.clone());
        }
        if retained {
            if already_open && self.ssh.status == SshStatus::Failed {
                self.start_ssh_connect(target, event_tx);
            } else if self.ssh.status == SshStatus::Ready {
                self.mode.replace(InteractionMode::SshInput);
            }
            return;
        }
        self.start_ssh_connect(target, event_tx);
    }

    fn start_ssh_connect(&mut self, target: AgentIdentity, event_tx: Option<&TuiEventSender>) {
        let generation = self.ssh.begin_connect(target.clone());
        let Some(event_tx) = event_tx.cloned() else {
            self.ssh
                .failed("TUI event channel is not available".to_string());
            return;
        };
        let Some(context_executor) = self.context_executor.clone() else {
            self.ssh
                .failed("TUI context executor is not available".to_string());
            return;
        };
        let auth_presenter = Some(tui_auth_presenter(&event_tx));
        context_executor.spawn(
            event_tx.control(),
            auth_presenter,
            move |launch_context| async move {
                let handler = SshCommandHandler::new(launch_context.into_context());
                let tool = ToolName::try_from("bash").map_err(anyhow::Error::msg)?;
                handler
                    .connect_embedded(RawAgentId(target.agent_id), tool, None, None)
                    .await
            },
            move |result| TuiEvent::SshConnected { generation, result },
        );
    }

    fn finish_ssh_connect(
        &mut self,
        generation: u64,
        result: TuiContextTaskResult<crate::command_handler::SshSession>,
    ) {
        if generation != self.ssh.generation {
            return;
        }
        let (context_id, result, logs) = result.into_parts();
        match result {
            Ok(session) => {
                self.ssh.connected(context_id, session);
                if matches!(self.ops.current_route(), OpsRoute::Ssh(_)) {
                    self.mode.replace(InteractionMode::SshInput);
                }
            }
            Err(error) => self.ssh.failed(agent_refresh_error(error, logs)),
        }
    }

    fn submit_ssh_command(&mut self, event_tx: Option<&TuiEventSender>) {
        if self.ssh.status != SshStatus::Ready {
            return;
        }
        let script = self.ssh.input.trim().to_string();
        if script.is_empty() {
            return;
        }
        self.ssh.input.clear();
        self.ssh.cursor = 0;
        let (cancel_tx, cancel_rx) = oneshot::channel();
        let Some(mut session) = self.ssh.begin_command(script.clone(), cancel_tx) else {
            return;
        };
        let generation = self.ssh.generation;
        let Some(event_tx) = event_tx.cloned() else {
            self.ssh
                .failed("TUI event channel is not available".to_string());
            return;
        };
        let Some(context_executor) = self.context_executor.clone() else {
            self.ssh
                .failed("TUI context executor is not available".to_string());
            return;
        };
        let auth_presenter = Some(tui_auth_presenter(&event_tx));
        context_executor.spawn(
            event_tx.control(),
            auth_presenter,
            move |launch_context| async move {
                let handler = SshCommandHandler::new(launch_context.into_context());
                let result = handler.run_embedded(&mut session, script, cancel_rx).await;
                Ok((session, result))
            },
            move |result| TuiEvent::SshCommandFinished { generation, result },
        );
    }

    fn finish_ssh_command(
        &mut self,
        generation: u64,
        result: TuiContextTaskResult<(
            crate::command_handler::SshSession,
            crate::command_handler::EmbeddedSshResult,
        )>,
    ) {
        if generation != self.ssh.generation {
            return;
        }
        let (context_id, result, logs) = result.into_parts();
        match result {
            Ok((session, result)) => self.ssh.finish_command(context_id, session, result),
            Err(error) => self.ssh.failed(agent_refresh_error(error, logs)),
        }
    }

    fn handle_ssh_input_key(&mut self, key: KeyEvent, event_tx: Option<&TuiEventSender>) {
        if key.code == KeyCode::Char('c')
            && key.modifiers.contains(KeyModifiers::CONTROL)
            && self.ssh.status == SshStatus::Running
        {
            self.ssh.cancel_command();
            return;
        }
        if self.handle_modified_shortcut(key, event_tx) {
            return;
        }
        match key.code {
            KeyCode::Esc => self.mode.replace(InteractionMode::Normal),
            KeyCode::Enter => self.submit_ssh_command(event_tx),
            KeyCode::Left => {
                self.ssh.cursor = previous_char_boundary(&self.ssh.input, self.ssh.cursor)
            }
            KeyCode::Right => {
                self.ssh.cursor = next_char_boundary(&self.ssh.input, self.ssh.cursor)
            }
            KeyCode::Home => self.ssh.cursor = 0,
            KeyCode::End => self.ssh.cursor = self.ssh.input.len(),
            KeyCode::Backspace => {
                let previous = previous_char_boundary(&self.ssh.input, self.ssh.cursor);
                self.ssh.input.drain(previous..self.ssh.cursor);
                self.ssh.cursor = previous;
            }
            KeyCode::Delete => {
                let next = next_char_boundary(&self.ssh.input, self.ssh.cursor);
                self.ssh.input.drain(self.ssh.cursor..next);
            }
            KeyCode::Up => {
                let next = self.ssh.history_cursor.map_or_else(
                    || self.ssh.history.len().checked_sub(1),
                    |cursor| cursor.checked_sub(1),
                );
                if let Some(cursor) = next {
                    self.ssh.history_cursor = Some(cursor);
                    self.ssh.input = self.ssh.history[cursor].clone();
                    self.ssh.cursor = self.ssh.input.len();
                }
            }
            KeyCode::Down => {
                if let Some(cursor) = self.ssh.history_cursor {
                    if cursor + 1 < self.ssh.history.len() {
                        self.ssh.history_cursor = Some(cursor + 1);
                        self.ssh.input = self.ssh.history[cursor + 1].clone();
                    } else {
                        self.ssh.history_cursor = None;
                        self.ssh.input.clear();
                    }
                    self.ssh.cursor = self.ssh.input.len();
                }
            }
            KeyCode::PageUp => self.ssh.scroll_up(10),
            KeyCode::PageDown => self.ssh.scroll_down(10),
            KeyCode::Char(character)
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                self.ssh.input.insert(self.ssh.cursor, character);
                self.ssh.cursor += character.len_utf8();
                self.ssh.history_cursor = None;
            }
            _ => {}
        }
    }

    fn activity_scope(&self) -> Option<Vec<ActivityAgent>> {
        self.ops.current_oplog().map(|route| {
            route
                .agents
                .iter()
                .map(|agent| ActivityAgent {
                    component: agent.component.clone(),
                    agent_id: agent.agent_id.clone(),
                })
                .collect()
        })
    }

    fn refresh_activity(&mut self, event_tx: Option<&TuiEventSender>) {
        if self.activity.loading {
            return;
        }
        let Some(agents) = self.activity_scope() else {
            self.activity.error = Some(
                "Oplog has no Agent scope; use an Agent action to open the selected Agent or included set"
                    .to_string(),
            );
            return;
        };
        if agents.is_empty() {
            self.activity.error = Some("Select an Agent before opening Oplog".to_string());
            return;
        }
        if agents.len() > activity::HARD_AGENT_LIMIT {
            self.activity.error = Some(format!(
                "Oplog is limited to {} Agents; narrow the loaded filter or include fewer Agents",
                activity::HARD_AGENT_LIMIT
            ));
            return;
        }
        let route = self.ops.current_oplog().cloned();
        self.activity.applied_query.from = route
            .as_ref()
            .and_then(|route| route.from_unix_nanos)
            .and_then(datetime_from_unix_nanos);
        self.activity.applied_query.to = route
            .as_ref()
            .and_then(|route| route.to_unix_nanos)
            .and_then(datetime_from_unix_nanos);
        let focus = route.and_then(|route| route.focus);
        let requests = agents
            .into_iter()
            .map(|agent| {
                let position = focus
                    .as_ref()
                    .filter(|focus| {
                        focus.agent.component == agent.component
                            && focus.agent.agent_id == agent.agent_id
                    })
                    .map(|focus| ActivityLoadPosition::Around(focus.index))
                    .unwrap_or_else(|| {
                        if self.activity.applied_query.active() {
                            ActivityLoadPosition::Search(None)
                        } else {
                            ActivityLoadPosition::Latest
                        }
                    });
                ActivityAgentRequest { agent, position }
            })
            .collect();
        self.activity.pending_focus = focus.map(|focus| {
            (
                ActivityAgent {
                    component: focus.agent.component,
                    agent_id: focus.agent.agent_id,
                },
                focus.index,
            )
        });
        self.start_activity_load(requests, false, event_tx);
    }

    fn start_activity_load(
        &mut self,
        requests: Vec<ActivityAgentRequest>,
        append: bool,
        event_tx: Option<&TuiEventSender>,
    ) {
        if requests.is_empty() {
            self.activity.paused = false;
            return;
        }
        let Some(event_tx) = event_tx.cloned() else {
            return;
        };
        let Some(context_executor) = self.context_executor.clone() else {
            self.activity.error = Some("TUI context executor is not available".to_string());
            return;
        };
        self.activity.generation = self.activity.generation.saturating_add(1);
        let generation = self.activity.generation;
        self.activity.loading = true;
        self.activity.paused = false;
        self.activity.stopped = false;
        self.activity.error = None;
        self.activity.context_id = Some(context_executor.current_context_id());
        let query = self.activity.applied_query.clone();
        let auth_presenter = Some(tui_auth_presenter(&event_tx));
        let task = context_executor.spawn(
            event_tx.control(),
            auth_presenter,
            move |launch_context| async move {
                activity::load(launch_context.into_context(), requests, query, append).await
            },
            move |result| TuiEvent::ActivityRefreshFinished { generation, result },
        );
        self.activity.task = Some(task);
    }

    fn finish_activity_refresh(
        &mut self,
        generation: u64,
        result: TuiContextTaskResult<ActivityRefreshPayload>,
    ) {
        if generation != self.activity.generation {
            return;
        }
        let (context_id, result, logs) = result.into_parts();
        if Some(context_id) != self.activity.context_id {
            return;
        }
        self.activity.task = None;
        self.activity.loading = false;
        self.activity.stopped = false;
        match result {
            Ok(payload) => {
                let selected = self.activity_selection_key();
                if payload.append {
                    for agent in payload.agents {
                        if !self.activity.agents.contains(&agent) {
                            self.activity.agents.push(agent);
                        }
                    }
                    self.activity.entries.extend(payload.entries);
                    let mut seen = HashSet::new();
                    self.activity.entries.retain(|entry| {
                        seen.insert((
                            entry.component.clone(),
                            entry.agent_id.clone(),
                            entry.presentation.index,
                        ))
                    });
                    for progress in payload.progress {
                        if let Some(existing) = self
                            .activity
                            .progress
                            .iter_mut()
                            .find(|existing| existing.agent == progress.agent)
                        {
                            *existing = progress;
                        } else {
                            self.activity.progress.push(progress);
                        }
                    }
                    self.activity.failures.extend(payload.failures);
                } else {
                    self.activity.agents = payload.agents;
                    self.activity.entries = payload.entries;
                    self.activity.progress = payload.progress;
                    self.activity.failures = payload.failures;
                }
                self.sort_activity_entries();
                self.activity.paused = self
                    .activity
                    .progress
                    .iter()
                    .any(|progress| progress.has_more);
                self.activity.selected = self
                    .activity
                    .selected
                    .min(self.activity_visible_len().saturating_sub(1));
                if let Some((agent, index)) = self.activity.pending_focus.take()
                    && let Some(position) = self.activity.entries.iter().position(|entry| {
                        entry.component == agent.component
                            && entry.agent_id == agent.agent_id
                            && entry.presentation.index == index
                    })
                {
                    self.activity.representation = ActivityRepresentation::Entries;
                    self.activity.selected = position;
                } else {
                    self.restore_activity_selection(selected);
                }
                self.activity.details_scroll = 0;
                self.activity.reveal_selection(self.activity_visible_len());
                self.activity.error = None;
            }
            Err(error) => self.activity.error = Some(agent_refresh_error(error, logs)),
        }
    }

    fn apply_activity_query(&mut self, event_tx: Option<&TuiEventSender>) {
        let query = self.activity.query.trim().to_string();
        if query == self.activity.applied_query.text && !self.activity.entries.is_empty() {
            return;
        }
        self.activity.query = query.clone();
        self.activity.applied_query.text = query;
        self.activity.selected = 0;
        self.activity.list_first_visible.set(0);
        self.activity.details_scroll = 0;
        self.ops.clear_oplog_focus();
        self.refresh_activity(event_tx);
    }

    fn activity_filter_changed(&mut self) {
        self.activity.selected = 0;
        self.activity.list_first_visible.set(0);
        self.activity.details_scroll = 0;
    }

    fn load_selected_activity_context(&mut self, event_tx: Option<&TuiEventSender>) {
        let target = match self.activity.representation {
            ActivityRepresentation::Entries => self
                .activity_filtered_entries()
                .get(self.activity.selected)
                .map(|entry| {
                    (
                        ActivityAgent {
                            component: entry.component.clone(),
                            agent_id: entry.agent_id.clone(),
                        },
                        entry.presentation.index,
                    )
                }),
            ActivityRepresentation::Grouped => self
                .activity_groups()
                .get(self.activity.selected)
                .map(|group| {
                    (
                        ActivityAgent {
                            component: group.component.clone(),
                            agent_id: group.agent_id.clone(),
                        },
                        group.first_index,
                    )
                }),
        };
        let Some((agent, index)) = target else {
            return;
        };
        self.activity.query.clear();
        self.activity.applied_query = activity::ActivityQuery::default();
        self.activity.loaded_filter.clear();
        self.activity.input_kind = ActivityInputKind::Loaded;
        self.activity.pending_focus = Some((agent.clone(), index));
        self.start_activity_load(
            vec![ActivityAgentRequest {
                agent,
                position: ActivityLoadPosition::Around(index),
            }],
            false,
            event_tx,
        );
    }

    fn load_more_activity(&mut self, event_tx: Option<&TuiEventSender>) {
        if self.activity.loading {
            return;
        }
        if self.activity.stopped && self.activity.progress.is_empty() {
            self.refresh_activity(event_tx);
            return;
        }
        let search = self.activity.applied_query.active();
        let requests = self
            .activity
            .progress
            .iter()
            .filter(|progress| progress.has_more)
            .filter_map(|progress| {
                let position = if search {
                    Some(ActivityLoadPosition::Search(progress.next.clone()))
                } else {
                    progress
                        .oldest_loaded_index
                        .map(ActivityLoadPosition::Before)
                }?;
                Some(ActivityAgentRequest {
                    agent: progress.agent.clone(),
                    position,
                })
            })
            .collect::<Vec<_>>();
        self.start_activity_load(requests, true, event_tx);
    }

    fn stop_activity(&mut self) {
        if !self.activity.loading {
            return;
        }
        self.activity.generation = self.activity.generation.saturating_add(1);
        if let Some(task) = self.activity.task.take() {
            task.abort();
        }
        self.activity.loading = false;
        self.activity.paused = true;
        self.activity.stopped = true;
    }

    fn invalidate_activity(&mut self) {
        if let Some(task) = self.activity.task.take() {
            task.abort();
        }
        let representation = self.activity.representation;
        let order = self.activity.order;
        let input_kind = self.activity.input_kind;
        let details_visible = self.activity.details_visible;
        let generation = self.activity.generation.saturating_add(1);
        let mut activity = ActivityState::default();
        activity.generation = generation;
        activity.representation = representation;
        activity.order = order;
        activity.input_kind = input_kind;
        activity.details_visible = details_visible;
        self.activity = activity;
    }

    fn move_activity_selection(&mut self, down: bool) {
        self.move_activity_selection_by(1, down);
    }

    fn move_activity_selection_by(&mut self, amount: usize, down: bool) {
        let last = self.activity_visible_len().saturating_sub(1);
        self.activity.selected = if down {
            self.activity.selected.saturating_add(amount).min(last)
        } else {
            self.activity.selected.saturating_sub(amount)
        };
        self.activity.details_scroll = 0;
        self.activity.reveal_selection(self.activity_visible_len());
    }

    fn select_first_activity(&mut self) {
        self.activity.selected = 0;
        self.activity.details_scroll = 0;
        self.activity.list_first_visible.set(0);
    }

    fn select_last_activity(&mut self) {
        self.activity.selected = self.activity_visible_len().saturating_sub(1);
        self.activity.details_scroll = 0;
        self.activity.reveal_selection(self.activity_visible_len());
    }

    fn set_activity_representation(
        &mut self,
        representation: ActivityRepresentation,
        _event_tx: Option<&TuiEventSender>,
    ) {
        let selected = self.activity_selection_key();
        self.activity.representation = representation;
        self.restore_activity_selection(selected);
        self.activity.details_scroll = 0;
        self.activity.reveal_selection(self.activity_visible_len());
        self.activity.error = None;
    }

    fn set_activity_order(&mut self, order: ActivityOrder) {
        if self.activity.order == order {
            return;
        }
        let selected = self.activity_selection_key();
        self.activity.order = order;
        self.sort_activity_entries();
        self.restore_activity_selection(selected);
        self.activity.reveal_selection(self.activity_visible_len());
    }

    fn activity_visible_len(&self) -> usize {
        match self.activity.representation {
            ActivityRepresentation::Grouped => self.activity_groups().len(),
            ActivityRepresentation::Entries => self.activity_filtered_entries().len(),
        }
    }

    fn activity_filtered_entries(&self) -> Vec<&activity::ActivityEntry> {
        activity::filtered_entries(&self.activity.entries, &self.activity.loaded_filter)
    }

    fn activity_groups(&self) -> Vec<activity::ActivityGroup> {
        activity::grouped_entries(
            &self.activity.entries,
            &self.activity.loaded_filter,
            !self.activity.applied_query.active(),
        )
    }

    fn sort_activity_entries(&mut self) {
        let order = self.activity.order;
        self.activity.entries.sort_by(|left, right| match order {
            ActivityOrder::Newest => right
                .presentation
                .timestamp
                .cmp(&left.presentation.timestamp)
                .then_with(|| left.agent_id.cmp(&right.agent_id))
                .then_with(|| right.presentation.index.cmp(&left.presentation.index)),
            ActivityOrder::Oldest => left
                .presentation
                .timestamp
                .cmp(&right.presentation.timestamp)
                .then_with(|| left.agent_id.cmp(&right.agent_id))
                .then_with(|| left.presentation.index.cmp(&right.presentation.index)),
        });
    }

    fn activity_selection_key(
        &self,
    ) -> Option<(String, String, Option<OplogEntryGroupKey>, u64, bool)> {
        match self.activity.representation {
            ActivityRepresentation::Entries => self
                .activity_filtered_entries()
                .get(self.activity.selected)
                .map(|entry| {
                    (
                        entry.component.clone(),
                        entry.agent_id.clone(),
                        entry.presentation.group.clone(),
                        entry.presentation.index,
                        true,
                    )
                }),
            ActivityRepresentation::Grouped => self
                .activity_groups()
                .get(self.activity.selected)
                .map(|group| {
                    (
                        group.component.clone(),
                        group.agent_id.clone(),
                        group.group.clone(),
                        group.first_index,
                        false,
                    )
                }),
        }
    }

    fn restore_activity_selection(
        &mut self,
        key: Option<(String, String, Option<OplogEntryGroupKey>, u64, bool)>,
    ) {
        let Some((component, agent_id, group, index, exact)) = key else {
            self.activity.selected = self
                .activity
                .selected
                .min(self.activity_visible_len().saturating_sub(1));
            return;
        };
        let position = match self.activity.representation {
            ActivityRepresentation::Entries => {
                self.activity_filtered_entries().iter().position(|entry| {
                    entry.component == component
                        && entry.agent_id == agent_id
                        && if exact {
                            entry.presentation.index == index
                        } else if group.is_some() {
                            entry.presentation.group == group || entry.presentation.index == index
                        } else {
                            entry.presentation.index == index
                        }
                })
            }
            ActivityRepresentation::Grouped => self.activity_groups().iter().position(|entry| {
                entry.component == component
                    && entry.agent_id == agent_id
                    && if group.is_some() {
                        entry.group == group
                    } else {
                        entry.first_index == index
                    }
            }),
        };
        self.activity.selected = position.unwrap_or(0);
    }

    fn toggle_activity_details(&mut self) {
        self.activity.details_visible = !self.activity.details_visible;
        if !self.activity.details_visible {
            self.activity.focus = ActivityFocus::List;
        }
    }

    fn scroll_activity_details_up(&mut self, amount: usize) {
        self.activity.details_scroll = self.activity.details_scroll.saturating_sub(amount);
    }

    fn scroll_activity_details_down(&mut self, amount: usize) {
        self.activity.details_scroll = self.activity.details_scroll.saturating_add(amount);
        self.activity.clamp_details_scroll();
    }

    fn scroll_activity_details_bottom(&mut self) {
        self.activity.details_scroll = self
            .activity
            .details_content_height
            .get()
            .saturating_sub(self.activity.details_viewport_height.get());
    }

    fn refresh_resources(&mut self, event_tx: Option<&TuiEventSender>, force: bool) {
        let kind = self.resources.selected_kind();
        if self.resources.loading == Some(kind) {
            return;
        }
        if !force && self.resources.is_loaded(kind) {
            return;
        }
        let Some(event_tx) = event_tx.cloned() else {
            return;
        };
        let Some(context_executor) = self.context_executor.clone() else {
            return;
        };

        let (generation, scope, kind) = self
            .resources
            .begin_load(context_executor.current_context_id());
        let environment_reference = self.selected_target.environment_reference.clone();
        let server = resources::ServerResourceSnapshot {
            status: self.server.run.status.title().to_string(),
            command: (self.server.run.id > 0).then(|| command_display(&self.server.run.args)),
            router_port: self
                .server
                .run
                .bound_ports
                .as_ref()
                .map(|ports| ports.router_port),
            custom_request_port: self
                .server
                .run
                .bound_ports
                .as_ref()
                .map(|ports| ports.custom_request_port),
            mcp_port: self
                .server
                .run
                .bound_ports
                .as_ref()
                .map(|ports| ports.mcp_port),
            exit_code: self.server.run.exit_code,
        };
        let auth_presenter = Some(tui_auth_presenter(&event_tx));
        context_executor.spawn(
            event_tx.control(),
            auth_presenter,
            move |launch_context| async move {
                resources::load(
                    launch_context.into_context(),
                    environment_reference.as_ref(),
                    scope,
                    kind,
                    server,
                    force,
                )
                .await
            },
            move |result| TuiEvent::ResourcesRefreshFinished { generation, result },
        );
    }

    fn finish_resources_refresh(
        &mut self,
        generation: u64,
        result: TuiContextTaskResult<ResourcesRefreshPayload>,
    ) {
        if generation != self.resources.generation {
            return;
        }
        let (context_id, result, logs) = result.into_parts();
        if Some(context_id) != self.resources.context_id {
            return;
        }
        self.resources.finish(
            generation,
            context_id,
            result.map_err(|error| anyhow::anyhow!(agent_refresh_error(error, logs))),
        );
    }

    fn invalidate_resources(&mut self) {
        self.resources.invalidate();
    }

    fn move_resource_selection(&mut self, down: bool, event_tx: Option<&TuiEventSender>) {
        if self.resources.move_selection(down) {
            self.refresh_resources(event_tx, false);
        }
    }

    fn refresh_overview(&mut self, event_tx: Option<&TuiEventSender>, force: bool) {
        if force || (self.ops_environment.snapshot.is_none() && !self.ops_environment.loading) {
            self.refresh_ops_environment(event_tx);
        }
        self.refresh_overview_observability(event_tx, force);
    }

    fn refresh_overview_observability(&mut self, event_tx: Option<&TuiEventSender>, force: bool) {
        if self.overview.observability.loading
            || (!force && self.overview.observability.value.is_some())
        {
            return;
        }
        let Some(event_tx) = event_tx.cloned() else {
            return;
        };
        let Some(context_executor) = self.context_executor.clone() else {
            self.overview.observability.error =
                Some("TUI context executor is not available".to_string());
            return;
        };
        let generation = self
            .overview
            .observability
            .begin(context_executor.current_context_id());
        let auth_presenter = Some(tui_auth_presenter(&event_tx));
        let task = context_executor.spawn(
            event_tx.control(),
            auth_presenter,
            move |launch_context| async move {
                overview::load_observability(launch_context.into_context()).await
            },
            move |result| TuiEvent::OverviewObservabilityFinished { generation, result },
        );
        self.overview.observability.attach_task(task);
    }

    fn finish_overview_observability(
        &mut self,
        generation: u64,
        result: TuiContextTaskResult<OverviewObservability>,
    ) {
        let (context_id, result, logs) = result.into_parts();
        self.overview.observability.finish(
            generation,
            context_id,
            result.map_err(|error| agent_refresh_error(error, logs)),
        );
    }

    fn open_selected_overview_item(&mut self, event_tx: Option<&TuiEventSender>) {
        match self.overview.selected_item() {
            OverviewItem::Agents => self.select_ops_destination(OpsDestination::Agents, event_tx),
            OverviewItem::Oplog => self.select_ops_destination(OpsDestination::Oplog, event_tx),
            OverviewItem::Stream => self.select_ops_destination(OpsDestination::Stream, event_tx),
            OverviewItem::Metrics => self.open_overview_metrics(event_tx),
            OverviewItem::Deployments => {
                self.select_ops_destination(OpsDestination::Deployments, event_tx)
            }
            OverviewItem::Configuration => {
                self.select_ops_destination(OpsDestination::Configuration, event_tx)
            }
            OverviewItem::ServerCollector => {
                self.ops.open_overview_server();
                self.refresh_overview_observability(event_tx, false);
            }
            OverviewItem::Issues => {
                if !self.overview_issues().is_empty() {
                    self.overview.selected_issue = self
                        .overview
                        .selected_issue
                        .min(self.overview_issues().len().saturating_sub(1));
                    self.ops.open_overview_issues();
                }
            }
        }
    }

    fn open_overview_metrics(&mut self, event_tx: Option<&TuiEventSender>) {
        self.metrics.source = crate::model::local_observability::LocalMetricSource::Agents;
        self.metrics.agent_scope.clear();
        self.metrics.mode = MetricsMode::Dashboard;
        self.metrics.range = metrics::MetricsRange::Minutes15;
        self.metrics.live = true;
        self.ops.select(OpsDestination::Metrics);
        self.reload_metrics(event_tx);
    }

    fn open_selected_overview_issue(&mut self, event_tx: Option<&TuiEventSender>) {
        let Some(issue) = self
            .overview_issues()
            .get(self.overview.selected_issue)
            .cloned()
        else {
            return;
        };
        match issue.target {
            OverviewIssueTarget::Deployments => {
                self.ops.open_deployment_history();
                self.refresh_deployments(event_tx);
            }
            OverviewIssueTarget::ServerCollector => self.ops.open_overview_server(),
            OverviewIssueTarget::Stream => {
                self.select_ops_destination(OpsDestination::Stream, event_tx)
            }
            OverviewIssueTarget::Agents => {
                self.select_ops_destination(OpsDestination::Agents, event_tx)
            }
            OverviewIssueTarget::Configuration => {
                self.select_ops_destination(OpsDestination::Configuration, event_tx)
            }
        }
    }

    fn overview_issues(&self) -> Vec<OverviewIssue> {
        let mut issues = Vec::new();
        if let Some(error) = &self.ops_environment.error {
            issues.push(OverviewIssue {
                title: "Environment lookup failed".to_string(),
                detail: error.clone(),
                target: OverviewIssueTarget::Deployments,
            });
        } else if self
            .ops_environment
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.deployment.is_none())
        {
            issues.push(OverviewIssue {
                title: "No active deployment".to_string(),
                detail: "Deploy the application environment before inspecting Agents or telemetry"
                    .to_string(),
                target: OverviewIssueTarget::Deployments,
            });
        }
        if let Some(error) = &self.overview.observability.error {
            issues.push(OverviewIssue {
                title: "Collector is unreachable".to_string(),
                detail: error.clone(),
                target: OverviewIssueTarget::ServerCollector,
            });
        } else if let Some(snapshot) = &self.overview.observability.value {
            match &snapshot.capability {
                None if self.selected_target.context.uses_local_server => {
                    issues.push(OverviewIssue {
                        title: "Local observability is unavailable".to_string(),
                        detail:
                            "The selected target does not expose the local observability capability"
                                .to_string(),
                        target: OverviewIssueTarget::ServerCollector,
                    });
                }
                None => {}
                Some(capability)
                    if capability.status
                        != crate::model::local_observability::LocalObservabilityStatus::Ready =>
                {
                    issues.push(OverviewIssue {
                        title: format!("Collector is {:?}", capability.status),
                        detail: capability
                            .message
                            .clone()
                            .unwrap_or_else(|| "No collector diagnostic was provided".to_string()),
                        target: OverviewIssueTarget::ServerCollector,
                    });
                }
                Some(capability) if capability.discarded_points > 0 => {
                    issues.push(OverviewIssue {
                        title: "Metric points were discarded".to_string(),
                        detail: format!(
                            "{} points were discarded by local retention",
                            capability.discarded_points
                        ),
                        target: OverviewIssueTarget::ServerCollector,
                    });
                }
                Some(_) => {}
            }
        }
        match &self.stream.status {
            StreamStatus::Failed(error) => issues.push(OverviewIssue {
                title: "Stream failed".to_string(),
                detail: error.clone(),
                target: OverviewIssueTarget::Stream,
            }),
            StreamStatus::Reconnecting { error, .. } => issues.push(OverviewIssue {
                title: "Stream is reconnecting".to_string(),
                detail: error.clone().unwrap_or_else(|| {
                    "The exact-Agent stream connection was interrupted".to_string()
                }),
                target: OverviewIssueTarget::Stream,
            }),
            _ => {}
        }
        if self.stream.server_missed > 0 || self.stream.locally_dropped > 0 {
            issues.push(OverviewIssue {
                title: "Stream events were missed".to_string(),
                detail: format!(
                    "{} server-missed · {} locally dropped",
                    self.stream.server_missed, self.stream.locally_dropped
                ),
                target: OverviewIssueTarget::Stream,
            });
        }
        if let Some(error) = &self.agents.last_error {
            issues.push(OverviewIssue {
                title: "Agent collection refresh failed".to_string(),
                detail: error.clone(),
                target: OverviewIssueTarget::Agents,
            });
        }
        if let Some(error) = self.resources.current_error() {
            issues.push(OverviewIssue {
                title: "Configuration read failed".to_string(),
                detail: error.to_string(),
                target: OverviewIssueTarget::Configuration,
            });
        }
        issues
    }

    fn open_metrics_range_form(&mut self) {
        self.metrics_range_form = Some(MetricsRangeForm::new(
            self.metrics.range,
            self.metrics.time_range,
        ));
        self.open_transient_mode(InteractionMode::MetricsRange);
    }

    fn handle_metrics_range_key(&mut self, key: KeyEvent, event_tx: Option<&TuiEventSender>) {
        if key.code == KeyCode::Char('x') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.open_leader();
            return;
        }
        if key.code == KeyCode::Char('p') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.open_palette(self.palette.scope);
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

        let Some(form) = self.metrics_range_form.as_mut() else {
            self.close_transient_mode();
            return;
        };
        if form.editing {
            match key.code {
                KeyCode::Esc => form.cancel_edit(),
                KeyCode::Enter => form.finish_edit(),
                KeyCode::Left => form.editor_mut().expect("range editor").move_left(),
                KeyCode::Right => form.editor_mut().expect("range editor").move_right(),
                KeyCode::Home => form.editor_mut().expect("range editor").cursor = 0,
                KeyCode::End => {
                    let editor = form.editor_mut().expect("range editor");
                    editor.cursor = editor.draft.len();
                }
                KeyCode::Backspace => form.editor_mut().expect("range editor").backspace(),
                KeyCode::Delete => form.editor_mut().expect("range editor").delete(),
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    form.editor_mut().expect("range editor").clear()
                }
                KeyCode::Char(character)
                    if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
                {
                    form.editor_mut().expect("range editor").insert(character)
                }
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Esc | KeyCode::Char('c')
                if key.code == KeyCode::Esc || key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.metrics_range_form = None;
                self.close_transient_mode();
            }
            KeyCode::Up | KeyCode::BackTab => form.move_focus(false),
            KeyCode::Down | KeyCode::Tab => form.move_focus(true),
            KeyCode::Left if form.selected_field() == MetricsRangeFormField::Preset => {
                form.cycle_preset(false)
            }
            KeyCode::Right | KeyCode::Char(' ')
                if form.selected_field() == MetricsRangeFormField::Preset =>
            {
                form.cycle_preset(true)
            }
            KeyCode::Enter => match form.selected_field() {
                MetricsRangeFormField::Apply | MetricsRangeFormField::Preset => {
                    self.apply_metrics_range_form(event_tx)
                }
                MetricsRangeFormField::From | MetricsRangeFormField::To => form.begin_edit(),
            },
            _ => {}
        }
    }

    fn apply_metrics_range_form(&mut self, event_tx: Option<&TuiEventSender>) {
        let result = self
            .metrics_range_form
            .as_ref()
            .ok_or_else(|| "The time range form is unavailable".to_string())
            .and_then(|form| {
                if form.custom {
                    parse_metrics_range(&form.from.draft, &form.to.draft).map(MetricsRange::Custom)
                } else {
                    Ok(form.preset)
                }
            });
        let range = match result {
            Ok(range) => range,
            Err(error) => {
                if let Some(form) = self.metrics_range_form.as_mut() {
                    form.error = Some(error);
                }
                return;
            }
        };
        self.metrics.range = range;
        if range.is_custom() {
            self.metrics.live = false;
        }
        self.metrics_range_form = None;
        self.close_transient_mode();
        self.reload_metrics(event_tx);
    }

    fn set_metrics_mode(&mut self, mode: MetricsMode, event_tx: Option<&TuiEventSender>) {
        if self.metrics.mode == mode {
            return;
        }
        self.metrics.mode = mode;
        self.reload_metrics(event_tx);
    }

    fn set_metrics_source(
        &mut self,
        source: crate::model::local_observability::LocalMetricSource,
        event_tx: Option<&TuiEventSender>,
    ) {
        if self.metrics.source == source {
            return;
        }
        self.metrics.source = source;
        self.metrics.selected_family = 0;
        self.metrics.selected_series = 0;
        self.metrics.selected_panel = 0;
        self.reload_metrics(event_tx);
    }

    fn set_metrics_live(&mut self, live: bool, event_tx: Option<&TuiEventSender>) {
        if self.metrics.live == live {
            return;
        }
        if live && self.metrics.range.is_custom() {
            self.metrics.live = false;
            self.open_metrics_range_form();
            if let Some(form) = self.metrics_range_form.as_mut() {
                form.error = Some(
                    "Live refresh requires a relative preset; choose one and Apply".to_string(),
                );
            }
            return;
        }
        self.metrics.live = live;
        if live {
            self.refresh_metrics_live(event_tx);
        }
    }

    fn metrics_query(&self, load: MetricsLoad) -> Option<MetricsQuery> {
        let source = self.metrics.source;
        let agent_ids = (source == crate::model::local_observability::LocalMetricSource::Agents)
            .then(|| {
                self.metrics
                    .agent_scope
                    .iter()
                    .map(|agent| agent.agent_id.clone())
                    .collect()
            })
            .unwrap_or_default();
        Some(MetricsQuery {
            source,
            mode: self.metrics.mode,
            agent_ids,
            selected_family: self.metrics.selected_family_name(),
            selected_series: self.metrics.selected_series_key(),
            selected_series_keys: self.metrics.selected_series_keys.clone(),
            range: self.metrics.range,
            load,
            families_cursor: (load == MetricsLoad::AppendFamilies)
                .then(|| self.metrics.families_next_cursor.clone())
                .flatten(),
            series_cursor: (load == MetricsLoad::AppendSeries)
                .then(|| self.metrics.series_next_cursor.clone())
                .flatten(),
        })
    }

    fn refresh_metrics(&mut self, event_tx: Option<&TuiEventSender>) {
        self.refresh_metrics_with_load(MetricsLoad::Replace, event_tx);
    }

    fn refresh_metrics_live(&mut self, event_tx: Option<&TuiEventSender>) {
        let load = if self.metrics.mode == MetricsMode::Explore
            && !self.metrics.selected_series_keys.is_empty()
        {
            MetricsLoad::Samples
        } else {
            MetricsLoad::Replace
        };
        self.refresh_metrics_with_load(load, event_tx);
    }

    fn refresh_metrics_with_load(&mut self, load: MetricsLoad, event_tx: Option<&TuiEventSender>) {
        if self.metrics.loading {
            return;
        }
        if !self.selected_target.context.uses_local_server {
            self.metrics.error = Some(
                "Metrics are currently available only for the built-in local server".to_string(),
            );
            return;
        }
        let Some(query) = self.metrics_query(load) else {
            self.metrics.error =
                Some("Metrics are available for Agents, individual Agents, and Server".to_string());
            return;
        };
        let Some(event_tx) = event_tx.cloned() else {
            return;
        };
        let Some(context_executor) = self.context_executor.clone() else {
            self.metrics.error = Some("TUI context executor is not available".to_string());
            return;
        };

        self.metrics.generation = self.metrics.generation.saturating_add(1);
        let generation = self.metrics.generation;
        self.metrics.loading = true;
        self.metrics.error = None;
        self.metrics.context_id = Some(context_executor.current_context_id());
        let environment_reference = self.selected_target.environment_reference.clone();
        let auth_presenter = Some(tui_auth_presenter(&event_tx));
        context_executor.spawn(
            event_tx.control(),
            auth_presenter,
            move |launch_context| async move {
                metrics::load(
                    launch_context.context(),
                    environment_reference.as_ref(),
                    query,
                )
                .await
            },
            move |result| TuiEvent::MetricsRefreshFinished { generation, result },
        );
    }

    fn reload_metrics(&mut self, event_tx: Option<&TuiEventSender>) {
        self.metrics.generation = self.metrics.generation.saturating_add(1);
        self.metrics.loading = false;
        self.metrics.notice = None;
        self.metrics.chart_cursor = None;
        self.metrics.time_range = None;
        match self.metrics.mode {
            MetricsMode::Dashboard => self.metrics.dashboard = None,
            MetricsMode::Explore => {
                self.metrics.families.clear();
                self.metrics.series.clear();
                self.metrics.samples.clear();
                self.metrics.series_samples.clear();
                self.metrics.selected_series_keys.clear();
                self.metrics.families_next_cursor = None;
                self.metrics.series_next_cursor = None;
                self.metrics.selected_family = 0;
                self.metrics.selected_series = 0;
            }
        }
        self.refresh_metrics(event_tx);
    }

    fn finish_metrics_refresh(
        &mut self,
        generation: u64,
        result: TuiContextTaskResult<MetricsRefreshPayload>,
    ) {
        if generation != self.metrics.generation {
            return;
        }
        let (context_id, result, logs) = result.into_parts();
        if Some(context_id) != self.metrics.context_id {
            return;
        }
        self.metrics.loading = false;
        match result {
            Ok(payload) => {
                self.metrics.capability = Some(payload.capability);
                self.metrics.source = payload.source;
                self.metrics.mode = payload.mode;
                self.metrics.time_range = Some(payload.time_range);
                match payload.load {
                    MetricsLoad::Replace => {
                        self.metrics.dashboard = payload.dashboard;
                        self.metrics.families = payload.families;
                        self.metrics.series = payload.series;
                        self.metrics.samples = payload.samples;
                        self.metrics.series_samples = payload.series_samples;
                        self.metrics.selected_series_keys = payload.selected_series_keys;
                        self.metrics.families_next_cursor = payload.families_next_cursor;
                        self.metrics.series_next_cursor = payload.series_next_cursor;
                        self.metrics.selected_family = payload.selected_family;
                        self.metrics.selected_series = payload.selected_series;
                    }
                    MetricsLoad::AppendFamilies => {
                        for family in payload.families {
                            if !self
                                .metrics
                                .families
                                .iter()
                                .any(|loaded| loaded.name == family.name)
                            {
                                self.metrics.families.push(family);
                            }
                        }
                        self.metrics.families_next_cursor = payload.families_next_cursor;
                        self.metrics.selected_family = self
                            .metrics
                            .selected_family
                            .min(self.metrics.families.len().saturating_sub(1));
                    }
                    MetricsLoad::SelectFamily => {
                        self.metrics.series = payload.series;
                        self.metrics.samples = payload.samples;
                        self.metrics.series_samples = payload.series_samples;
                        self.metrics.selected_series_keys = payload.selected_series_keys;
                        self.metrics.series_next_cursor = payload.series_next_cursor;
                        self.metrics.selected_series = payload.selected_series;
                    }
                    MetricsLoad::AppendSeries => {
                        let first_new = self.metrics.series.len();
                        for series in payload.series {
                            if !self
                                .metrics
                                .series
                                .iter()
                                .any(|loaded| loaded.series_key == series.series_key)
                            {
                                self.metrics.series.push(series);
                            }
                        }
                        self.metrics.series_next_cursor = payload.series_next_cursor;
                        self.metrics.selected_series =
                            first_new.min(self.metrics.series.len().saturating_sub(1));
                    }
                    MetricsLoad::Samples => {
                        self.metrics.samples = payload.samples;
                        self.metrics.series_samples = payload.series_samples;
                        self.metrics.selected_series_keys = payload.selected_series_keys;
                    }
                }
                self.metrics.error = None;
            }
            Err(error) => self.metrics.error = Some(agent_refresh_error(error, logs)),
        }
    }

    fn invalidate_metrics(&mut self) {
        self.metrics_range_form = None;
        self.metrics = MetricsState {
            generation: self.metrics.generation.saturating_add(1),
            ..MetricsState::default()
        };
    }

    fn move_metrics_selection(&mut self, down: bool, event_tx: Option<&TuiEventSender>) {
        if self.metrics.mode == MetricsMode::Dashboard {
            let panel_count = self
                .metrics
                .dashboard
                .as_ref()
                .map(|dashboard| dashboard.panels.len())
                .unwrap_or(0);
            if panel_count == 0 {
                return;
            }
            self.metrics.selected_panel = if down {
                (self.metrics.selected_panel + 1).min(panel_count - 1)
            } else {
                self.metrics.selected_panel.saturating_sub(1)
            };
            self.metrics.chart_cursor = None;
            return;
        }
        match self.metrics.focus {
            MetricsFocus::Families => {
                let last = self.metrics.families.len().saturating_sub(1);
                if down
                    && self.metrics.selected_family == last
                    && self.metrics.families_next_cursor.is_some()
                {
                    self.refresh_metrics_with_load(MetricsLoad::AppendFamilies, event_tx);
                    return;
                }
                let selected = if down {
                    self.metrics.selected_family.saturating_add(1).min(last)
                } else {
                    self.metrics.selected_family.saturating_sub(1)
                };
                self.select_metrics_family(selected, event_tx);
            }
            MetricsFocus::Series => {
                let last = self.metrics.series.len().saturating_sub(1);
                if down
                    && self.metrics.selected_series == last
                    && self.metrics.series_next_cursor.is_some()
                {
                    self.refresh_metrics_with_load(MetricsLoad::AppendSeries, event_tx);
                    return;
                }
                let selected = if down {
                    self.metrics.selected_series.saturating_add(1).min(last)
                } else {
                    self.metrics.selected_series.saturating_sub(1)
                };
                self.select_metrics_series(selected);
            }
            MetricsFocus::Chart => return,
        }
    }

    fn select_metrics_family(&mut self, index: usize, event_tx: Option<&TuiEventSender>) {
        if self.metrics.families.is_empty() {
            return;
        }
        self.metrics.focus = MetricsFocus::Families;
        let selected = index.min(self.metrics.families.len() - 1);
        if selected == self.metrics.selected_family {
            self.metrics.reveal_family_selection();
            return;
        }
        self.metrics.selected_family = selected;
        self.metrics.notice = None;
        self.metrics.chart_cursor = None;
        self.metrics.selected_series = 0;
        self.metrics.series.clear();
        self.metrics.samples.clear();
        self.metrics.series_samples.clear();
        self.metrics.selected_series_keys.clear();
        self.metrics.series_next_cursor = None;
        self.metrics.reveal_family_selection();
        self.metrics.series_first_visible.set(0);
        self.refresh_metrics_with_load(MetricsLoad::SelectFamily, event_tx);
    }

    fn select_metrics_series(&mut self, index: usize) {
        if self.metrics.series.is_empty() {
            return;
        }
        self.metrics.focus = MetricsFocus::Series;
        self.metrics.selected_series = index.min(self.metrics.series.len() - 1);
        self.metrics.reveal_series_selection();
    }

    fn toggle_metrics_series(&mut self, event_tx: Option<&TuiEventSender>) {
        if self.metrics.mode != MetricsMode::Explore || self.metrics.focus != MetricsFocus::Series {
            return;
        }
        let Some(series_key) = self.metrics.selected_series_key() else {
            return;
        };
        if let Some(index) = self
            .metrics
            .selected_series_keys
            .iter()
            .position(|selected| selected == &series_key)
        {
            self.metrics.selected_series_keys.remove(index);
            self.metrics.notice = None;
        } else if self
            .metrics
            .selected_series_keys
            .first()
            .and_then(|selected| {
                self.metrics
                    .series
                    .iter()
                    .find(|series| &series.series_key == selected)
            })
            .zip(
                self.metrics
                    .series
                    .iter()
                    .find(|series| series.series_key == series_key),
            )
            .is_some_and(|(selected, candidate)| {
                selected.statistic != candidate.statistic
                    || selected.temporality != candidate.temporality
            })
        {
            self.metrics.notice = Some(
                "Only series with the same statistic and temporality can share a chart".to_string(),
            );
            return;
        } else if self.metrics.selected_series_keys.len() >= MAX_EXPLORE_SERIES {
            self.metrics.notice = Some(format!(
                "Explore overlays are limited to {MAX_EXPLORE_SERIES} compatible series"
            ));
            return;
        } else {
            self.metrics.selected_series_keys.push(series_key);
            self.metrics.notice = None;
        }
        self.metrics.chart_cursor = None;
        self.refresh_metrics_with_load(MetricsLoad::Samples, event_tx);
    }

    fn open_selected_metric_panel(&mut self) {
        let Some(panel_id) = self.metrics.dashboard.as_ref().and_then(|dashboard| {
            dashboard
                .panels
                .get(self.metrics.selected_panel)
                .map(|panel| panel.id.clone())
        }) else {
            return;
        };
        self.metrics.chart_cursor = None;
        self.ops.open_metrics_panel(panel_id);
    }

    fn metrics_chart_inspection_active(&self) -> bool {
        matches!(self.ops.current_route(), OpsRoute::MetricsPanel(_))
            || (matches!(self.ops.current_route(), OpsRoute::Root)
                && self.metrics.mode == MetricsMode::Explore
                && self.metrics.focus == MetricsFocus::Chart)
    }

    fn metrics_chart_timestamps(&self) -> Vec<i64> {
        let mut timestamps: Vec<i64> = match self.ops.current_route() {
            OpsRoute::MetricsPanel(panel_id) => self
                .metrics
                .dashboard
                .as_ref()
                .and_then(|dashboard| dashboard.panels.iter().find(|panel| panel.id == *panel_id))
                .into_iter()
                .flat_map(|panel| panel.series.iter())
                .flat_map(|series| series.samples.iter())
                .map(|sample| sample.timestamp_unix_nanos)
                .collect(),
            _ => self
                .metrics
                .series_samples
                .iter()
                .flat_map(|series| series.response.samples.iter())
                .map(|sample| sample.timestamp_unix_nanos)
                .collect(),
        };
        timestamps.sort_unstable();
        timestamps.dedup();
        timestamps
    }

    fn move_metrics_chart_cursor(&mut self, right: bool) {
        let timestamps = self.metrics_chart_timestamps();
        if timestamps.is_empty() {
            return;
        }
        let current = self
            .metrics
            .chart_cursor
            .unwrap_or(timestamps.len() - 1)
            .min(timestamps.len() - 1);
        self.metrics.chart_cursor = Some(if right {
            current.saturating_add(1).min(timestamps.len() - 1)
        } else {
            current.saturating_sub(1)
        });
    }

    fn open_metric_sample_in_oplog(&mut self, event_tx: Option<&TuiEventSender>) {
        let target = match self.metric_sample_oplog_target() {
            Ok(target) => target,
            Err(message) => {
                self.metrics.notice = Some(message);
                return;
            }
        };
        self.metrics.notice = None;
        let (route, representation) = match target {
            MetricOplogTarget::Exact { agent, index } => (
                OplogRoute {
                    agents: vec![agent.clone()],
                    focus: Some(OplogEntryIdentity { agent, index }),
                    from_unix_nanos: None,
                    to_unix_nanos: None,
                },
                ActivityRepresentation::Entries,
            ),
            MetricOplogTarget::Range {
                agent,
                from_unix_nanos,
                to_unix_nanos,
            } => (
                OplogRoute {
                    agents: vec![agent],
                    focus: None,
                    from_unix_nanos: Some(from_unix_nanos),
                    to_unix_nanos: Some(to_unix_nanos),
                },
                ActivityRepresentation::Grouped,
            ),
        };
        self.ops.open_oplog(route);
        self.invalidate_activity();
        self.activity.representation = representation;
        self.refresh_activity(event_tx);
    }

    fn metric_sample_oplog_target(&self) -> Result<MetricOplogTarget, String> {
        if self.metrics.source != crate::model::local_observability::LocalMetricSource::Agents {
            return Err("Server metrics do not have Agent Oplog entries".to_string());
        }
        if matches!(self.ops.current_route(), OpsRoute::MetricsPanel(_)) {
            return Err(
                "Dashboard points combine a scope; use Explore to open an exact Agent sample"
                    .to_string(),
            );
        }
        let timestamp = metrics_cursor_timestamp(self, &self.metrics_chart_timestamps())
            .ok_or_else(|| "Select a metric sample before opening Oplog".to_string())?;
        let series = self
            .metrics
            .series
            .get(self.metrics.selected_series)
            .ok_or_else(|| "Select an Agent metric series before opening Oplog".to_string())?;
        let agent_id = series
            .agent_id
            .as_deref()
            .ok_or_else(|| "The selected series is not scoped to one Agent".to_string())?;
        let response = self
            .metrics
            .series_samples
            .iter()
            .find(|samples| samples.series_key == series.series_key)
            .map(|samples| &samples.response)
            .ok_or_else(|| "The selected series has no loaded samples".to_string())?;
        let sample = response
            .samples
            .iter()
            .find(|sample| sample.timestamp_unix_nanos == timestamp)
            .ok_or_else(|| "The selected series has no value at this chart position".to_string())?;
        let mut matches = self
            .agents
            .agents
            .iter()
            .filter(|agent| agent.agent_id == agent_id)
            .map(AgentListItem::identity);
        let agent = matches
            .next()
            .ok_or_else(|| "Load the sample's Agent before opening its Oplog".to_string())?;
        if matches.next().is_some() {
            return Err(
                "The AgentID matches multiple loaded components; narrow the Agent scope first"
                    .to_string(),
            );
        }
        if let Some(index) = sample.source_oplog_index {
            return Ok(MetricOplogTarget::Exact { agent, index });
        }
        let bucket_width = response.bucket_width_unix_nanos.ok_or_else(|| {
            "The selected sample has no exact Oplog source or aggregate time bucket".to_string()
        })?;
        let query_range = self.metrics.time_range.ok_or_else(|| {
            "The loaded metric range is unavailable; refresh before opening Oplog".to_string()
        })?;
        let bucket =
            metric_bucket_range(query_range, bucket_width, timestamp).ok_or_else(|| {
                "The selected aggregate point is outside the loaded metric range".to_string()
            })?;
        Ok(MetricOplogTarget::Range {
            agent,
            from_unix_nanos: bucket.from_unix_nanos,
            to_unix_nanos: bucket.to_unix_nanos,
        })
    }

    fn select_metrics_chart_at(&mut self, x: u16) {
        let Some(area) = self.geometry.region(RegionKind::MetricsChart) else {
            return;
        };
        let timestamps = self.metrics_chart_timestamps();
        if timestamps.is_empty() {
            return;
        }
        let offset = x.saturating_sub(area.x).min(area.width.saturating_sub(1));
        let index = if area.width <= 1 {
            0
        } else {
            let first = timestamps[0] as i128;
            let span = timestamps[timestamps.len() - 1] as i128 - first;
            let target =
                first + span * offset as i128 / area.width.saturating_sub(1).max(1) as i128;
            timestamps
                .iter()
                .enumerate()
                .min_by_key(|(_, timestamp)| (**timestamp as i128 - target).abs())
                .map(|(index, _)| index)
                .unwrap_or(0)
        };
        self.metrics.chart_cursor = Some(index);
        self.metrics.focus = MetricsFocus::Chart;
    }

    fn dev_panel_focused(&self, panel: DevView) -> bool {
        self.active_workspace == Workspace::Dev
            && self.dev_focus == panel
            && self.dev_view_visible(panel)
    }

    fn ops_agents_focused(&self) -> bool {
        self.active_workspace == Workspace::Ops
            && self.ops.active() == OpsDestination::Agents
            && matches!(self.ops.current_route(), OpsRoute::Root)
    }

    fn agents_focused(&self) -> bool {
        self.ops_agents_focused()
    }

    fn ops_document_focused(&self) -> bool {
        self.active_workspace == Workspace::Ops
            && self.ops.active() == OpsDestination::Agents
            && matches!(self.ops.current_route(), OpsRoute::Agent(_))
    }

    fn ops_activity_focused(&self) -> bool {
        self.active_workspace == Workspace::Ops && self.ops.active() == OpsDestination::Oplog
    }

    fn ops_details_requested(&self) -> bool {
        (self.ops_agents_focused() && self.agents.detail_visible)
            || (self.ops_activity_focused() && self.activity.details_visible)
    }

    fn agent_details_open(&self) -> bool {
        self.agents_focused()
            && self.agents.detail_visible
            && self.agent_details_layout_area().is_none_or(|area| {
                layout::ops_details_visible(area, true, self.layout.ops_details_placement)
            })
    }

    fn activity_details_open(&self) -> bool {
        self.ops_activity_focused()
            && self.activity.details_visible
            && self.geometry.region(RegionKind::OpsDetails).is_some()
    }

    fn ops_details_layout_area(&self) -> Option<Rect> {
        let list = self.geometry.region(RegionKind::OpsList)?;
        let details = self.geometry.region(RegionKind::OpsDetails)?;
        let left = list.x.min(details.x);
        let top = list.y.min(details.y);
        let right = list
            .x
            .saturating_add(list.width)
            .max(details.x.saturating_add(details.width));
        let bottom = list
            .y
            .saturating_add(list.height)
            .max(details.y.saturating_add(details.height));
        Some(Rect::new(
            left,
            top,
            right.saturating_sub(left),
            bottom.saturating_sub(top),
        ))
    }

    fn activity_list_focused(&self) -> bool {
        self.ops_activity_focused()
            && (!self.activity_details_open() || self.activity.focus == ActivityFocus::List)
    }

    fn activity_details_focused(&self) -> bool {
        self.activity_details_open() && self.activity.focus == ActivityFocus::Details
    }

    fn agent_details_layout_area(&self) -> Option<Rect> {
        let snapshot = self.geometry.snapshot()?;
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
        match self.ops.active() {
            OpsDestination::Agents if self.agent_details_open() => {
                self.agents.focus = match self.agents.focus {
                    AgentOverviewFocus::List => AgentOverviewFocus::Details,
                    AgentOverviewFocus::Details => AgentOverviewFocus::List,
                };
                if self.agents.focus == AgentOverviewFocus::Details {
                    self.mode.replace(InteractionMode::Normal);
                }
            }
            OpsDestination::Oplog if self.activity_details_open() => {
                self.activity.focus = match self.activity.focus {
                    ActivityFocus::List => ActivityFocus::Details,
                    ActivityFocus::Details => ActivityFocus::List,
                };
                if self.activity.focus == ActivityFocus::Details {
                    self.mode.replace(InteractionMode::Normal);
                }
            }
            OpsDestination::Metrics
                if matches!(self.ops.current_route(), OpsRoute::Root)
                    && self.metrics.mode == MetricsMode::Explore =>
            {
                self.metrics.focus = self.metrics.focus.next()
            }
            OpsDestination::Configuration => self.resources.cycle_focus(true),
            OpsDestination::Overview
            | OpsDestination::Agents
            | OpsDestination::Metrics
            | OpsDestination::Oplog
            | OpsDestination::Stream
            | OpsDestination::Deployments => {}
        }
    }

    fn context_switch_dev_blockers(
        &self,
        action: &PendingContextSwitchAction,
    ) -> Vec<&'static str> {
        let mut blockers = Vec::new();
        if self.command_is_running() {
            blockers.push("command");
        }
        if self.repl.is_running() {
            blockers.push("repl");
        }
        if self.context_switch_stops_server(action) {
            blockers.push("server");
        }
        if matches!(self.ssh.status, SshStatus::Connecting | SshStatus::Running) {
            blockers.push("ssh");
        }
        blockers
    }

    fn context_switch_dev_blocker_message(&self) -> Option<String> {
        let blockers = self
            .context_switcher
            .pending_action
            .as_ref()
            .map(|action| self.context_switch_dev_blockers(action))
            .unwrap_or_default();
        if blockers.is_empty() {
            None
        } else {
            Some(format!(
                "Switching context will stop context-bound work: {}",
                blockers.join(", ")
            ))
        }
    }

    fn context_switch_stops_server(&self, action: &PendingContextSwitchAction) -> bool {
        let PendingContextSwitchAction::Switch(request) = action else {
            return false;
        };
        if !self.server.run.is_running() {
            return false;
        }
        let running_identity = self
            .server
            .run
            .server_identity
            .as_ref()
            .unwrap_or(&self.selected_target.server_identity);
        running_identity != &request.server_identity
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
                    PendingContextSwitchAction::Switch(ContextSwitchRequest {
                        global_flags: target.global_flags.clone(),
                        environment_reference: None,
                        server_identity: target.server_identity.clone(),
                        dev_eligible: true,
                    }),
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
        if target.is_current {
            self.context_switcher.last_error = None;
            return;
        }

        self.request_context_switch_action(
            PendingContextSwitchAction::Switch(ContextSwitchRequest {
                global_flags: target.server_flags.clone(),
                environment_reference: target.environment_reference.clone(),
                server_identity: target.server_identity.clone(),
                dev_eligible: target.dev_eligible,
            }),
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
        if self.context_switch_dev_blockers(&action).is_empty() {
            self.start_context_switch_action(action, event_tx);
        } else {
            self.context_switcher.pending_action = Some(action);
            self.context_switcher.waiting_for_dev_stop = false;
            self.mode.push(InteractionMode::ContextSwitchConfirm);
        }
    }

    fn confirm_context_switch(&mut self, event_tx: Option<&TuiEventSender>) {
        if self.context_switcher.pending_action.is_none() {
            self.close_transient_mode();
            return;
        }
        self.close_ops_for_context_switch();
        let action = self
            .context_switcher
            .pending_action
            .as_ref()
            .expect("pending context switch")
            .clone();
        self.request_dev_stop_for_context_switch(&action);
        self.context_switcher.waiting_for_dev_stop = true;
        self.resume_pending_context_switch_if_ready(event_tx);
    }

    fn start_context_switch_action(
        &mut self,
        action: PendingContextSwitchAction,
        event_tx: Option<&TuiEventSender>,
    ) {
        match action {
            PendingContextSwitchAction::Switch(request) => {
                self.start_context_switch(request, event_tx)
            }
            PendingContextSwitchAction::LoadEnvironments { target } => {
                self.start_context_environment_list(target, event_tx)
            }
        }
    }

    fn resume_pending_context_switch_if_ready(&mut self, event_tx: Option<&TuiEventSender>) {
        let blockers_remain = self
            .context_switcher
            .pending_action
            .as_ref()
            .is_some_and(|action| !self.context_switch_dev_blockers(action).is_empty());
        if !self.context_switcher.waiting_for_dev_stop || blockers_remain {
            return;
        }
        self.context_switcher.waiting_for_dev_stop = false;
        if let Some(action) = self.context_switcher.pending_action.take() {
            self.mode.replace(InteractionMode::ContextPicker);
            self.start_context_switch_action(action, event_tx);
        } else {
            self.mode.replace(InteractionMode::ContextPicker);
        }
    }

    fn close_ops_for_context_switch(&mut self) {
        self.agents.refresh_running = false;
        self.agents.refresh_context_id = None;
        self.agents.refresh_generation += 1;
    }

    fn request_dev_stop_for_context_switch(&mut self, action: &PendingContextSwitchAction) {
        if self.command_is_running() {
            self.cancel_or_force_kill_command();
        }
        if self.repl.is_running() {
            self.stop_repl();
        }
        if self.context_switch_stops_server(action) {
            self.stop_server();
        }
        if matches!(self.ssh.status, SshStatus::Connecting | SshStatus::Running) {
            self.ssh.reset();
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
        request: ContextSwitchRequest,
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
        self.context_switcher.pending_key = request
            .environment_reference
            .as_ref()
            .map(ToString::to_string)
            .or_else(|| Some("context".to_string()));
        self.context_switcher.last_error = None;
        let generation = self.context_switcher.generation;
        let auth_presenter = Some(tui_auth_presenter(&event_tx));
        context_executor.spawn(
            event_tx.control(),
            auth_presenter,
            move |_launch_context| async move {
                Context::new(request.global_flags.clone(), Some(Output::Captured))
                    .await
                    .map(|context| ContextSwitchResult {
                        context: Arc::new(context),
                        request,
                    })
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
            event_tx.control(),
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
        result: TuiContextTaskResult<ContextSwitchResult>,
        event_tx: &TuiEventSender,
    ) {
        if generation != self.context_switcher.generation {
            return;
        }
        self.context_switcher.switch_running = false;
        let (_, result, logs) = result.into_parts();
        match result {
            Ok(result) => {
                let Some(context_executor) = self.context_executor.as_ref() else {
                    self.context_switcher.last_error =
                        Some("TUI context executor is not available".to_string());
                    return;
                };
                context_executor.select_context(result.context.clone());
                let context = TuiContextInfo::from_selected(
                    result.context.as_ref(),
                    result.request.environment_reference.as_ref(),
                    result.request.dev_eligible,
                );
                self.selected_target = SelectedTarget {
                    context,
                    cli_args: context_args_from_flags(&result.request.global_flags),
                    environment_reference: result.request.environment_reference,
                    server_identity: result.request.server_identity,
                };
                self.refresh_context_target_markers();
                self.reset_agents_for_context_switch();
                self.invalidate_ops_environment();
                self.overview.reset();
                self.invalidate_metrics();
                self.invalidate_activity();
                self.invalidate_resources();
                self.deployments.reset();
                self.stream.reset();
                self.ssh.reset();
                self.ops.reset_routes();
                self.context_switcher.last_error = None;
                self.restore_default_mode();
                if self.active_workspace == Workspace::Ops {
                    self.refresh_active_ops_destination(Some(event_tx));
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
                    &self.selected_target.context,
                    &self.selected_target.server_identity,
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

    fn refresh_context_target_markers(&mut self) {
        let context = self.selected_target.context.clone();
        let server_identity = self.selected_target.server_identity.clone();
        for target in &mut self.context_switcher.targets {
            target.is_current = target_matches_selected_context(target, &context, &server_identity);
        }
        for target in &mut self.context_switcher.app_environments {
            target.is_current = target.app.as_deref() == Some(context.application.as_str())
                && target.environment.as_deref() == Some(context.environment.as_str())
                && target.server_identity == server_identity;
        }
    }

    fn local_server_unavailable(&self) -> bool {
        !self.server.available
    }

    fn dev_context_unavailable(&self) -> bool {
        !self.selected_target.context.dev_eligible
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
        self.mode.replace(InteractionMode::AgentDatasetFilter);
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
        self.agents.refresh_context_label = Some(self.selected_target.context.short_label());
        let mode = self.agents.mode;
        let environment_reference = self.selected_target.environment_reference.clone();
        let dataset_scope = self.agents.dataset_scope.clone();
        let page_cursor = append.then(|| AgentListPageCursor {
            components: self.agents.paging.cursors().clone(),
        });
        let max_count = self.agents.paging.request_limit(append);
        let auth_presenter = Some(tui_auth_presenter(&event_tx));
        context_executor.spawn(
            event_tx.control(),
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
        let environment_reference = self.selected_target.environment_reference.clone();
        let auth_presenter = Some(tui_auth_presenter(&event_tx));
        context_executor.spawn(
            event_tx.control(),
            auth_presenter,
            move |launch_context| async move {
                let context = launch_context.context();
                let environment = context
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

    fn refresh_deployment_history(&mut self, event_tx: Option<&TuiEventSender>) {
        let Some(event_tx) = event_tx.cloned() else {
            return;
        };
        let Some(context_executor) = self.context_executor.clone() else {
            self.deployments.error = Some("TUI context executor is not available".to_string());
            return;
        };
        let context_id = context_executor.current_context_id();
        let generation = self.deployments.begin(context_id);
        let environment_reference = self.selected_target.environment_reference.clone();
        let auth_presenter = Some(tui_auth_presenter(&event_tx));
        let task = context_executor.spawn(
            event_tx.control(),
            auth_presenter,
            move |launch_context| async move {
                deployments::load_history(launch_context.into_context(), environment_reference)
                    .await
            },
            move |result| TuiEvent::DeploymentsRefreshFinished { generation, result },
        );
        self.deployments.attach_task(task);
    }

    fn refresh_deployments(&mut self, event_tx: Option<&TuiEventSender>) {
        self.refresh_ops_environment(event_tx);
        match self.ops.current_route().clone() {
            OpsRoute::DeploymentRevision(revision) => {
                self.deployments.details.remove(&revision);
                self.load_deployment_revisions(vec![revision], event_tx);
            }
            OpsRoute::DeploymentCompare(route) => {
                self.deployments.details.remove(&route.base_revision);
                self.deployments.details.remove(&route.target_revision);
                self.load_deployment_revisions(
                    vec![route.base_revision, route.target_revision],
                    event_tx,
                );
            }
            _ => self.refresh_deployment_history(event_tx),
        }
    }

    fn load_deployment_revisions(
        &mut self,
        revisions: Vec<u64>,
        event_tx: Option<&TuiEventSender>,
    ) {
        let revisions = revisions
            .into_iter()
            .filter(|revision| !self.deployments.details.contains_key(revision))
            .collect::<Vec<_>>();
        if revisions.is_empty() {
            return;
        }
        let Some(event_tx) = event_tx.cloned() else {
            return;
        };
        let Some(context_executor) = self.context_executor.clone() else {
            self.deployments.error = Some("TUI context executor is not available".to_string());
            return;
        };
        let context_id = context_executor.current_context_id();
        let generation = self.deployments.begin(context_id);
        let environment_reference = self.selected_target.environment_reference.clone();
        let auth_presenter = Some(tui_auth_presenter(&event_tx));
        let task = context_executor.spawn(
            event_tx.control(),
            auth_presenter,
            move |launch_context| async move {
                deployments::load_revisions(
                    launch_context.into_context(),
                    environment_reference,
                    revisions,
                )
                .await
            },
            move |result| TuiEvent::DeploymentsRefreshFinished { generation, result },
        );
        self.deployments.attach_task(task);
    }

    fn deployment_history_visible_len(&self) -> usize {
        match self.ops.current_route() {
            OpsRoute::Root => self.deployments.history.len().min(RECENT_LIMIT),
            OpsRoute::DeploymentHistory => self.deployments.history.len(),
            _ => 0,
        }
    }

    fn move_deployment_selection(&mut self, forward: bool) {
        let visible = self.deployment_history_visible_len();
        self.deployments.selected = self.deployments.selected.min(visible.saturating_sub(1));
        if forward {
            self.deployments.select_next(visible);
        } else {
            self.deployments.select_previous(visible);
        }
    }

    fn open_selected_deployment(&mut self, event_tx: Option<&TuiEventSender>) {
        let visible = self.deployment_history_visible_len();
        self.deployments.selected = self.deployments.selected.min(visible.saturating_sub(1));
        let Some(revision) = self
            .deployments
            .history
            .get(self.deployments.selected)
            .map(|record| record.revision)
        else {
            return;
        };
        self.ops.open_deployment_revision(revision);
        self.deployments.scroll = 0;
        self.load_deployment_revisions(vec![revision], event_tx);
    }

    fn compare_deployment_with_predecessor(&mut self, event_tx: Option<&TuiEventSender>) {
        let OpsRoute::DeploymentRevision(target) = self.ops.current_route() else {
            return;
        };
        let target = *target;
        let Some(base) = self.deployments.predecessor(target) else {
            return;
        };
        self.ops.open_deployment_compare(base, target);
        self.deployments.selected_change = 0;
        self.deployments.table_first_visible.set(0);
        self.load_deployment_revisions(vec![base, target], event_tx);
    }

    fn move_deployment_change_selection(&mut self, forward: bool) {
        let OpsRoute::DeploymentCompare(route) = self.ops.current_route() else {
            return;
        };
        let count = self
            .deployments
            .changes(route.base_revision, route.target_revision)
            .map_or(0, |changes| changes.len());
        if forward {
            self.deployments.selected_change = self
                .deployments
                .selected_change
                .saturating_add(1)
                .min(count.saturating_sub(1));
        } else {
            self.deployments.selected_change = self.deployments.selected_change.saturating_sub(1);
        }
    }

    fn toggle_deployment_compare_focus(&mut self) {
        let Some(route) = self.ops.deployment_compare_mut() else {
            return;
        };
        route.focus = match route.focus {
            DeploymentCompareFocus::Base => DeploymentCompareFocus::Target,
            DeploymentCompareFocus::Target => DeploymentCompareFocus::Base,
        };
    }

    fn move_deployment_compare_revision(&mut self, newer: bool, event_tx: Option<&TuiEventSender>) {
        let OpsRoute::DeploymentCompare(current) = self.ops.current_route().clone() else {
            return;
        };
        let revision = match current.focus {
            DeploymentCompareFocus::Base => current.base_revision,
            DeploymentCompareFocus::Target => current.target_revision,
        };
        let Some(index) = self
            .deployments
            .history
            .iter()
            .position(|record| record.revision == revision)
        else {
            return;
        };
        let next_index = if newer {
            index.saturating_sub(1)
        } else {
            index
                .saturating_add(1)
                .min(self.deployments.history.len().saturating_sub(1))
        };
        let Some(next_revision) = self
            .deployments
            .history
            .get(next_index)
            .map(|record| record.revision)
        else {
            return;
        };
        let Some(route) = self.ops.deployment_compare_mut() else {
            return;
        };
        match route.focus {
            DeploymentCompareFocus::Base => route.base_revision = next_revision,
            DeploymentCompareFocus::Target => route.target_revision = next_revision,
        }
        let revisions = vec![route.base_revision, route.target_revision];
        self.deployments.selected_change = 0;
        self.deployments.table_first_visible.set(0);
        self.load_deployment_revisions(revisions, event_tx);
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

    fn handle_terminal_input_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Char('x') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.mode.replace(InteractionMode::Normal);
            return;
        }

        let Some(target) = self.focused_nested_cli_target() else {
            self.mode.replace(InteractionMode::Normal);
            return;
        };
        if let Some(bytes) = encode_key_for_pty(key) {
            self.write_terminal_input(target, &bytes);
        }
    }

    fn write_terminal_input(&mut self, target: NestedCliTarget, bytes: &[u8]) {
        let result = match target {
            NestedCliTarget::Command => self
                .command_run
                .as_mut()
                .and_then(|run| run.runtime.as_mut())
                .map(|runtime| runtime.write_all(bytes)),
            NestedCliTarget::Server => self
                .server
                .run
                .runtime
                .as_mut()
                .map(|runtime| runtime.write_all(bytes)),
            NestedCliTarget::Repl => self
                .repl
                .run
                .runtime
                .as_mut()
                .map(|runtime| runtime.write_all(bytes)),
        };
        let Some(Err(error)) = result else {
            return;
        };
        let message = format!("failed to send input: {error:#}");
        match target {
            NestedCliTarget::Command => self.append_local_command_line(message),
            NestedCliTarget::Server => self.server.run.terminal.feed_local_line(message),
            NestedCliTarget::Repl => self.repl.run.last_error = Some(message),
        }
    }

    fn open_command_form(&mut self, kind: CommandKind) {
        let key = self.selected_target.context.short_label();
        let values = self
            .command_form_memory
            .get(&key)
            .cloned()
            .unwrap_or_default();
        self.command_form = Some(CommandReviewForm::new(kind, values));
        self.mode.replace(InteractionMode::CommandForm);
    }

    fn handle_command_form_key(&mut self, key: KeyEvent, event_tx: Option<&TuiEventSender>) {
        if key.code == KeyCode::Char('x') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.open_leader();
            return;
        }
        if key.code == KeyCode::Char('p') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.open_palette(self.palette.scope);
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
            self.restore_default_mode();
            return;
        };
        if form.editing.is_some() {
            match key.code {
                KeyCode::Esc => form.editing = None,
                KeyCode::Enter => {
                    let field = form.selected_field();
                    let draft = form.editing.take().expect("active field editor").draft;
                    apply_command_form_edit(form, field, draft);
                }
                KeyCode::Left => form
                    .editing
                    .as_mut()
                    .expect("active field editor")
                    .move_left(),
                KeyCode::Right => form
                    .editing
                    .as_mut()
                    .expect("active field editor")
                    .move_right(),
                KeyCode::Home => form.editing.as_mut().expect("active field editor").cursor = 0,
                KeyCode::End => {
                    let editor = form.editing.as_mut().expect("active field editor");
                    editor.cursor = editor.draft.len();
                }
                KeyCode::Backspace => form
                    .editing
                    .as_mut()
                    .expect("active field editor")
                    .backspace(),
                KeyCode::Delete => form.editing.as_mut().expect("active field editor").delete(),
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    form.editing.as_mut().expect("active field editor").clear()
                }
                KeyCode::Char(character)
                    if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
                {
                    form.editing
                        .as_mut()
                        .expect("active field editor")
                        .insert(character);
                }
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Esc => {
                self.command_form = None;
                self.restore_default_mode();
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
            .insert(self.selected_target.context.short_label(), remembered);
        self.mode.replace(InteractionMode::Normal);
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
            return;
        }
        self.open_dev_workspace(DevView::BuildDeploy);
        let take_input = self.dev_panel_focused(DevView::BuildDeploy);
        self.archive_current_command();
        if self.dev_context_unavailable() {
            self.command_run = Some(CommandRun::new(
                self.next_command_id,
                kind,
                self.command_args(kind, &values),
                self.command_options,
                self.selected_target.context.short_label(),
            ));
            self.next_command_id += 1;
            self.append_local_command_line("selected context is ops-only");
            self.set_command_status(CommandStatus::Failed);
            return;
        }
        let args = self.command_args(kind, &values);
        let command_id = self.next_command_id;
        self.next_command_id += 1;
        self.mode.replace(if take_input {
            InteractionMode::TerminalInput
        } else {
            InteractionMode::Normal
        });
        self.command_run = Some(CommandRun::new(
            command_id,
            kind,
            args.clone(),
            self.command_options,
            self.selected_target.context.short_label(),
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
                self.mode.replace(InteractionMode::Normal);
                return;
            }
        };

        match spawn_nested_cli(
            spec,
            event_tx.control(),
            event_tx.output(),
            NestedCliTarget::Command,
        ) {
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
                self.mode.replace(InteractionMode::Normal);
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
            if values.skip_check {
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
        if (self.mode == InteractionMode::TerminalInput
            && self.dev_panel_focused(DevView::BuildDeploy))
            || (self.mode == InteractionMode::TerminalScrollback
                && self.dev_focus == DevView::BuildDeploy)
        {
            self.mode.reset(InteractionMode::Normal);
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
                self.mode.replace(InteractionMode::Normal);
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
        self.stream.disconnect();
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
            mode: self.mode.current(),
            context_picker_rows: context_picker_layout_rows(&self.context_switcher),
            context_picker_selected: self.context_switcher.selected,
            context_picker_prefix_height: context_picker_prefix_height(&self.context_switcher),
            ops_details_visible: self.ops_details_requested(),
            dev_session_status_widths: footer_session_status_widths(self, cols),
            layout: self.layout.clone(),
        });
        self.geometry.update(snapshot);
        self.sync_nested_session_sizes();
    }

    fn sync_nested_session_sizes(&mut self) {
        if let Some(area) = self
            .geometry
            .region(RegionKind::DevViewBody(DevView::BuildDeploy))
            && let Some(run) = self.command_run.as_mut()
        {
            let (cols, rows) = pty_size_for_region(area);
            if run.terminal.size() != (rows, cols) {
                run.terminal.resize(rows, cols);
                if let Some(runtime) = run.runtime.as_mut() {
                    let _ = runtime.resize(cols, rows);
                }
            }
        }
        if let Some(area) = self
            .geometry
            .region(RegionKind::DevViewBody(DevView::Server))
        {
            let (cols, rows) = pty_size_for_region(area);
            if self.server.run.terminal.size() != (rows, cols) {
                self.server.run.terminal.resize(rows, cols);
                if let Some(runtime) = self.server.run.runtime.as_mut() {
                    let _ = runtime.resize(cols, rows);
                }
            }
        }
        if let Some(area) = self.geometry.region(RegionKind::DevViewBody(DevView::Repl)) {
            let (cols, rows) = pty_size_for_region(area);
            if self.repl.run.screen.size() != (rows, cols) {
                self.repl.run.screen.resize(rows, cols);
                if let Some(runtime) = self.repl.run.runtime.as_mut() {
                    let _ = runtime.resize(cols, rows);
                }
            }
        }
    }

    fn scroll_output_up_by(&mut self, amount: usize) {
        if let Some(run) = self.command_run.as_mut() {
            run.terminal.scroll_up(amount);
        }
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

    fn open_server_form(&mut self, restart: bool, clean: bool) {
        let key = self.selected_target.context.short_label();
        let mut values = self
            .server_form_memory
            .get(&key)
            .cloned()
            .unwrap_or_else(|| ServerFormValues::new(&self.selected_target.context.config_dir));
        values.clean |= clean;
        values.yes = false;
        self.session_form = Some(SessionReviewForm::Server {
            values,
            selected: 0,
            editing: None,
            restart,
            error: None,
        });
        self.mode.replace(InteractionMode::SessionForm);
    }

    fn open_repl_form(&mut self, restart: bool) {
        let key = self.selected_target.context.short_label();
        let mut values = self.repl_form_memory.get(&key).cloned().unwrap_or_default();
        values.yes = false;
        self.session_form = Some(SessionReviewForm::Repl {
            values,
            selected: 0,
            editing: None,
            restart,
        });
        self.mode.replace(InteractionMode::SessionForm);
    }

    fn handle_session_form_key(&mut self, key: KeyEvent, event_tx: Option<&TuiEventSender>) {
        if key.code == KeyCode::Char('x') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.open_leader();
            return;
        }
        if key.code == KeyCode::Char('p') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.open_palette(self.palette.scope);
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
            self.restore_default_mode();
            return;
        };

        if form.editing().is_some() {
            match key.code {
                KeyCode::Esc => {
                    form.take_editing();
                }
                KeyCode::Enter => {
                    let field = form.selected_field();
                    let draft = form.take_editing().expect("active field editor").draft;
                    apply_session_form_edit(form, field, draft);
                }
                KeyCode::Left => form.editing_mut().expect("active field editor").move_left(),
                KeyCode::Right => form
                    .editing_mut()
                    .expect("active field editor")
                    .move_right(),
                KeyCode::Home => form.editing_mut().expect("active field editor").cursor = 0,
                KeyCode::End => {
                    let editor = form.editing_mut().expect("active field editor");
                    editor.cursor = editor.draft.len();
                }
                KeyCode::Backspace => form.editing_mut().expect("active field editor").backspace(),
                KeyCode::Delete => form.editing_mut().expect("active field editor").delete(),
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    form.editing_mut().expect("active field editor").clear()
                }
                KeyCode::Char(character)
                    if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
                {
                    form.editing_mut()
                        .expect("active field editor")
                        .insert(character);
                }
                _ => {}
            }
            form.clear_error();
            return;
        }

        let count = form.field_count();
        match key.code {
            KeyCode::Esc => {
                self.session_form = None;
                self.restore_default_mode();
            }
            KeyCode::Up => form.set_selected(form.selected().saturating_sub(1)),
            KeyCode::Down | KeyCode::Tab => form.set_selected((form.selected() + 1) % count),
            KeyCode::BackTab => form.set_selected((form.selected() + count - 1) % count),
            KeyCode::Char(' ') | KeyCode::Enter => {
                if form.selected() == 0 {
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
                    .insert(self.selected_target.context.short_label(), remembered);
                self.mode.replace(InteractionMode::Normal);
                if *restart && self.server.run.is_running() {
                    self.open_dev_workspace(DevView::Server);
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
                    .insert(self.selected_target.context.short_label(), remembered);
                self.mode.replace(InteractionMode::Normal);
                if *restart && self.repl.is_running() {
                    self.open_dev_workspace(DevView::Repl);
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
        self.open_dev_workspace(DevView::Server);
        self.mode.replace(InteractionMode::Normal);
        if self.local_server_unavailable() {
            self.server
                .run
                .terminal
                .feed_local_line("local server is unavailable");
            return;
        }
        if self.server.run.is_running() {
            self.server
                .run
                .terminal
                .feed_local_line("server already running");
            return;
        }

        let clean = values.clean;
        let args = server_args_from_values(&values);
        let ports_file =
            (!values.ports_file.trim().is_empty()).then(|| PathBuf::from(values.ports_file.trim()));
        let ports_file_before_modified = ports_file
            .as_ref()
            .and_then(|path| std::fs::metadata(path).ok())
            .and_then(|metadata| metadata.modified().ok());
        self.mode.replace(InteractionMode::Normal);
        self.server.run = ServerRun::new(
            self.server.next_id,
            clean,
            args.clone(),
            self.server.launch_label.clone(),
            self.selected_target.server_identity.clone(),
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
                self.mode.replace(InteractionMode::Normal);
                return;
            }
        };

        match spawn_nested_cli(
            spec,
            event_tx.control(),
            event_tx.output(),
            NestedCliTarget::Server,
        ) {
            Ok(mut runtime) => {
                if let Some((cols, rows)) = self.dev_pty_size(DevView::Server) {
                    let _ = runtime.resize(cols, rows);
                }
                self.server.run.runtime = Some(runtime);
                self.server.run.mark_spawned();
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
                self.mode.replace(InteractionMode::Normal);
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

        if (self.mode == InteractionMode::TerminalInput && self.dev_panel_focused(DevView::Server))
            || (self.mode == InteractionMode::TerminalScrollback
                && self.dev_focus == DevView::Server)
        {
            self.mode.reset(InteractionMode::Normal);
        }

        if let Some(values) = pending_restart {
            self.start_server_with_values(values, Some(event_tx));
        }
        self.resume_pending_context_switch_if_ready(Some(event_tx));
    }

    fn handle_server_spinner_tick(&mut self, server_id: u64) {
        if self.server.run.id == server_id && self.server.run.is_running() {
            self.server.run.spinner_frame = self.server.run.spinner_frame.wrapping_add(1);
            if self.server.run.bound_ports.is_none()
                && let Some(ports) = fresh_bound_server_ports(&self.server.run)
            {
                self.server.run.bound_ports = Some(ports);
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

    fn start_repl_with_values(
        &mut self,
        values: ReplFormValues,
        event_tx: Option<&TuiEventSender>,
    ) {
        self.open_dev_workspace(DevView::Repl);
        let take_input = self.dev_panel_focused(DevView::Repl);
        if self.dev_context_unavailable() {
            self.repl.run.last_error = Some("selected context is ops-only".to_string());
            self.mode.replace(InteractionMode::Normal);
            return;
        }
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
            self.selected_target.context.short_label(),
        );
        self.mode.replace(if take_input {
            InteractionMode::TerminalInput
        } else {
            InteractionMode::Normal
        });
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
                self.mode.replace(InteractionMode::Normal);
                return;
            }
        };

        match spawn_nested_cli(
            spec,
            event_tx.control(),
            event_tx.output(),
            NestedCliTarget::Repl,
        ) {
            Ok(mut runtime) => {
                let _ = runtime.resize(screen_cols, screen_rows);
                self.repl.run.status = ReplStatus::Running;
                self.repl.run.runtime = Some(runtime);
            }
            Err(error) => {
                self.repl.run.status = ReplStatus::Failed;
                self.repl.run.last_error = Some(format!("failed to start REPL: {error:#}"));
                self.mode.replace(InteractionMode::Normal);
            }
        }
    }

    fn repl_spec(&self, args: Vec<String>) -> anyhow::Result<NestedCliSpec> {
        self.nested_cli_spec(args)
    }

    fn nested_cli_spec(&self, args: Vec<String>) -> anyhow::Result<NestedCliSpec> {
        self.nested_cli_spec_with_context_args(args, &self.selected_target.cli_args)
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
                self.close_transient_mode();
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
        if (self.mode == InteractionMode::TerminalInput && self.dev_panel_focused(DevView::Repl))
            || (self.mode == InteractionMode::TerminalScrollback && self.dev_focus == DevView::Repl)
        {
            self.mode.reset(InteractionMode::Normal);
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
        self.geometry
            .region(RegionKind::DevViewBody(view))
            .map(pty_size_for_region)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum LeaderGroup {
    #[default]
    Workspaces,
    Views,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum QuitChoice {
    #[default]
    Return,
    ForceQuit,
}

impl QuitChoice {
    fn next(self) -> Self {
        match self {
            Self::Return => Self::ForceQuit,
            Self::ForceQuit => Self::Return,
        }
    }

    fn previous(self) -> Self {
        self.next()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct LeaderState {
    group: Option<LeaderGroup>,
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
    skip_check: bool,
    force_build: bool,
    full_diff: bool,
    rollback_version: String,
    rollback_revision: String,
    post_deploy: PostDeployPolicy,
    yes: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReviewField {
    Run,
    Yes,
    Clean,
    SkipCheck,
    ForceBuild,
    FullDiff,
    RollbackVersion,
    RollbackRevision,
    PostDeploy,
    SystemMemory,
    RouterAddress,
    RouterPort,
    CustomRequestPort,
    McpPort,
    PortsFile,
    DataDirectory,
    AgentFilesystemRoot,
    Language,
    StreamLogs,
    AutomaticImports,
}

const BUILD_REVIEW_FIELDS: &[ReviewField] = &[
    ReviewField::Run,
    ReviewField::Yes,
    ReviewField::SkipCheck,
    ReviewField::ForceBuild,
];
const DEPLOY_REVIEW_FIELDS: &[ReviewField] = &[
    ReviewField::Run,
    ReviewField::Yes,
    ReviewField::FullDiff,
    ReviewField::ForceBuild,
    ReviewField::RollbackVersion,
    ReviewField::RollbackRevision,
    ReviewField::PostDeploy,
];
const CLEAN_REVIEW_FIELDS: &[ReviewField] = &[ReviewField::Run, ReviewField::Yes];
const SERVER_REVIEW_FIELDS: &[ReviewField] = &[
    ReviewField::Run,
    ReviewField::Clean,
    ReviewField::Yes,
    ReviewField::SystemMemory,
    ReviewField::RouterAddress,
    ReviewField::RouterPort,
    ReviewField::CustomRequestPort,
    ReviewField::McpPort,
    ReviewField::PortsFile,
    ReviewField::DataDirectory,
    ReviewField::AgentFilesystemRoot,
];
const REPL_REVIEW_FIELDS: &[ReviewField] = &[
    ReviewField::Run,
    ReviewField::Yes,
    ReviewField::Language,
    ReviewField::StreamLogs,
    ReviewField::AutomaticImports,
    ReviewField::PostDeploy,
];

#[derive(Debug, Clone)]
struct TextEditState {
    draft: String,
    cursor: usize,
}

impl TextEditState {
    fn new(value: &str) -> Self {
        Self {
            draft: value.to_string(),
            cursor: value.len(),
        }
    }

    fn insert(&mut self, character: char) {
        self.draft.insert(self.cursor, character);
        self.cursor += character.len_utf8();
    }

    fn move_left(&mut self) {
        self.cursor = previous_char_boundary(&self.draft, self.cursor);
    }

    fn move_right(&mut self) {
        self.cursor = next_char_boundary(&self.draft, self.cursor);
    }

    fn backspace(&mut self) {
        let previous = previous_char_boundary(&self.draft, self.cursor);
        if previous < self.cursor {
            self.draft.drain(previous..self.cursor);
            self.cursor = previous;
        }
    }

    fn delete(&mut self) {
        let next = next_char_boundary(&self.draft, self.cursor);
        if self.cursor < next {
            self.draft.drain(self.cursor..next);
        }
    }

    fn clear(&mut self) {
        self.draft.clear();
        self.cursor = 0;
    }

    fn viewport(&self, width: usize) -> (String, usize) {
        if width == 0 {
            return (String::new(), 0);
        }
        let maximum_cursor_column = width.saturating_sub(1);
        let mut start = 0;
        while UnicodeWidthStr::width(&self.draft[start..self.cursor]) > maximum_cursor_column {
            start = next_char_boundary(&self.draft, start);
        }
        let cursor_column = UnicodeWidthStr::width(&self.draft[start..self.cursor]);
        let mut end = start;
        let mut used = 0_usize;
        for (offset, character) in self.draft[start..].char_indices() {
            let character_width = character.width().unwrap_or_default();
            if used.saturating_add(character_width) > width {
                break;
            }
            used = used.saturating_add(character_width);
            end = start + offset + character.len_utf8();
        }
        (self.draft[start..end].to_string(), cursor_column)
    }
}

fn previous_char_boundary(value: &str, cursor: usize) -> usize {
    value[..cursor]
        .char_indices()
        .next_back()
        .map(|(index, _)| index)
        .unwrap_or_default()
}

fn next_char_boundary(value: &str, cursor: usize) -> usize {
    value[cursor..]
        .chars()
        .next()
        .map(|character| cursor + character.len_utf8())
        .unwrap_or(cursor)
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
    editing: Option<TextEditState>,
}

impl CommandReviewForm {
    fn new(kind: CommandKind, mut values: CommandFormValues) -> Self {
        values.yes = false;
        Self {
            kind,
            values,
            selected: 0,
            editing: None,
        }
    }

    fn field_count(&self) -> usize {
        self.fields().len()
    }

    fn fields(&self) -> &'static [ReviewField] {
        match self.kind {
            CommandKind::Build => BUILD_REVIEW_FIELDS,
            CommandKind::Plan | CommandKind::Deploy => DEPLOY_REVIEW_FIELDS,
            CommandKind::Clean => CLEAN_REVIEW_FIELDS,
        }
    }

    fn selected_field(&self) -> ReviewField {
        self.fields()[self.selected]
    }
}

fn command_form_is_launch(form: &CommandReviewForm) -> bool {
    form.selected_field() == ReviewField::Run
}

fn update_command_form_field(form: &mut CommandReviewForm) {
    match form.selected_field() {
        ReviewField::Yes => form.values.yes = !form.values.yes,
        ReviewField::SkipCheck => form.values.skip_check = !form.values.skip_check,
        ReviewField::ForceBuild => form.values.force_build = !form.values.force_build,
        ReviewField::FullDiff => form.values.full_diff = !form.values.full_diff,
        ReviewField::RollbackVersion => {
            form.editing = Some(TextEditState::new(&form.values.rollback_version));
        }
        ReviewField::RollbackRevision => {
            form.editing = Some(TextEditState::new(&form.values.rollback_revision));
        }
        ReviewField::PostDeploy => form.values.post_deploy = form.values.post_deploy.next(),
        _ => {}
    }
}

fn apply_command_form_edit(form: &mut CommandReviewForm, field: ReviewField, value: String) {
    match field {
        ReviewField::RollbackVersion => {
            form.values.rollback_version = value;
            if !form.values.rollback_version.is_empty() {
                form.values.rollback_revision.clear();
            }
        }
        ReviewField::RollbackRevision => {
            form.values.rollback_revision = value;
            if !form.values.rollback_revision.is_empty() {
                form.values.rollback_version.clear();
            }
        }
        _ => {}
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

const AGENT_COLUMNS: [PaneTableColumn<'static>; 7] = [
    PaneTableColumn {
        id: "agent_id",
        title: "AgentID",
        minimum_width: 18,
        width: 32,
        alignment: Alignment::Left,
        required: true,
        default_visible: true,
        policy: CellPolicy::Ellipsis,
    },
    PaneTableColumn {
        id: "status",
        title: "Status",
        minimum_width: 8,
        width: 14,
        alignment: Alignment::Left,
        required: true,
        default_visible: true,
        policy: CellPolicy::Ellipsis,
    },
    PaneTableColumn {
        id: "type",
        title: "Type",
        minimum_width: 10,
        width: 22,
        alignment: Alignment::Left,
        required: false,
        default_visible: true,
        policy: CellPolicy::Ellipsis,
    },
    PaneTableColumn {
        id: "component",
        title: "Component",
        minimum_width: 12,
        width: 28,
        alignment: Alignment::Left,
        required: false,
        default_visible: true,
        policy: CellPolicy::Ellipsis,
    },
    PaneTableColumn {
        id: "revision",
        title: "Revision",
        minimum_width: 8,
        width: 10,
        alignment: Alignment::Right,
        required: false,
        default_visible: false,
        policy: CellPolicy::Ellipsis,
    },
    PaneTableColumn {
        id: "pending",
        title: "Pending",
        minimum_width: 7,
        width: 10,
        alignment: Alignment::Right,
        required: false,
        default_visible: false,
        policy: CellPolicy::Ellipsis,
    },
    PaneTableColumn {
        id: "created_at",
        title: "Created at",
        minimum_width: 12,
        width: 24,
        alignment: Alignment::Left,
        required: false,
        default_visible: false,
        policy: CellPolicy::Ellipsis,
    },
];

const DEPLOYMENT_COLUMNS: [PaneTableColumn<'static>; 3] = [
    PaneTableColumn {
        id: "revision",
        title: "Revision",
        minimum_width: 8,
        width: 10,
        alignment: Alignment::Right,
        required: true,
        default_visible: true,
        policy: CellPolicy::Ellipsis,
    },
    PaneTableColumn {
        id: "version",
        title: "Version",
        minimum_width: 12,
        width: 28,
        alignment: Alignment::Left,
        required: true,
        default_visible: true,
        policy: CellPolicy::Ellipsis,
    },
    PaneTableColumn {
        id: "hash",
        title: "Deployment hash",
        minimum_width: 16,
        width: 44,
        alignment: Alignment::Left,
        required: true,
        default_visible: true,
        policy: CellPolicy::Ellipsis,
    },
];

const DEPLOYMENT_CHANGE_COLUMNS: [PaneTableColumn<'static>; 5] = [
    PaneTableColumn {
        id: "change",
        title: "Change",
        minimum_width: 8,
        width: 10,
        alignment: Alignment::Left,
        required: true,
        default_visible: true,
        policy: CellPolicy::Ellipsis,
    },
    PaneTableColumn {
        id: "kind",
        title: "Kind",
        minimum_width: 12,
        width: 26,
        alignment: Alignment::Left,
        required: true,
        default_visible: true,
        policy: CellPolicy::Ellipsis,
    },
    PaneTableColumn {
        id: "name",
        title: "Name",
        minimum_width: 14,
        width: 30,
        alignment: Alignment::Left,
        required: true,
        default_visible: true,
        policy: CellPolicy::Ellipsis,
    },
    PaneTableColumn {
        id: "before",
        title: "Before",
        minimum_width: 12,
        width: 34,
        alignment: Alignment::Left,
        required: true,
        default_visible: true,
        policy: CellPolicy::Ellipsis,
    },
    PaneTableColumn {
        id: "after",
        title: "After",
        minimum_width: 12,
        width: 34,
        alignment: Alignment::Left,
        required: true,
        default_visible: true,
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

    fn label(self) -> &'static str {
        match self {
            Self::Durable => "durable",
            Self::Ephemeral => "ephemeral",
            Self::All => "all",
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
    deployment: Option<OpsDeploymentSnapshot>,
}

#[derive(Clone, Debug)]
struct OpsDeploymentSnapshot {
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
                deployment_revision: deployment.deployment_revision.to_string(),
                version: deployment.deployment_version.0,
                hash: deployment.deployment_hash.to_string(),
            });
    OpsEnvironmentSnapshot { deployment }
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
    server_identity: Option<ServerIdentity>,
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
            server_identity: None,
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
        server_identity: ServerIdentity,
        ports_file: Option<PathBuf>,
        ports_file_before_modified: Option<SystemTime>,
    ) -> Self {
        Self {
            id,
            status: ServerStatus::Starting,
            clean,
            args,
            launch_context,
            server_identity: Some(server_identity),
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

    fn mark_spawned(&mut self) {
        self.status = ServerStatus::Running;
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
    fn new(_config_dir: &str) -> Self {
        Self {
            system_memory_override: String::new(),
            router_address: String::new(),
            router_port: String::new(),
            custom_request_port: String::new(),
            mcp_port: String::new(),
            ports_file: String::new(),
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
        editing: Option<TextEditState>,
        restart: bool,
        error: Option<String>,
    },
    Repl {
        values: ReplFormValues,
        selected: usize,
        editing: Option<TextEditState>,
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
        self.fields().len()
    }

    fn set_selected(&mut self, value: usize) {
        match self {
            Self::Server { selected, .. } | Self::Repl { selected, .. } => *selected = value,
        }
    }

    fn fields(&self) -> &'static [ReviewField] {
        match self {
            Self::Server { .. } => SERVER_REVIEW_FIELDS,
            Self::Repl { .. } => REPL_REVIEW_FIELDS,
        }
    }

    fn selected_field(&self) -> ReviewField {
        self.fields()[self.selected()]
    }

    fn editing(&self) -> Option<&TextEditState> {
        match self {
            Self::Server { editing, .. } | Self::Repl { editing, .. } => editing.as_ref(),
        }
    }

    fn editing_mut(&mut self) -> Option<&mut TextEditState> {
        match self {
            Self::Server { editing, .. } | Self::Repl { editing, .. } => editing.as_mut(),
        }
    }

    fn take_editing(&mut self) -> Option<TextEditState> {
        match self {
            Self::Server { editing, .. } | Self::Repl { editing, .. } => editing.take(),
        }
    }

    fn clear_error(&mut self) {
        if let Self::Server { error, .. } = self {
            *error = None;
        }
    }
}

fn update_session_form_field(form: &mut SessionReviewForm) {
    let field = form.selected_field();
    match form {
        SessionReviewForm::Server {
            values, editing, ..
        } => match field {
            ReviewField::Clean => values.clean = !values.clean,
            ReviewField::Yes => values.yes = !values.yes,
            ReviewField::SystemMemory => {
                *editing = Some(TextEditState::new(&values.system_memory_override))
            }
            ReviewField::RouterAddress => {
                *editing = Some(TextEditState::new(&values.router_address))
            }
            ReviewField::RouterPort => *editing = Some(TextEditState::new(&values.router_port)),
            ReviewField::CustomRequestPort => {
                *editing = Some(TextEditState::new(&values.custom_request_port))
            }
            ReviewField::McpPort => *editing = Some(TextEditState::new(&values.mcp_port)),
            ReviewField::PortsFile => *editing = Some(TextEditState::new(&values.ports_file)),
            ReviewField::DataDirectory => *editing = Some(TextEditState::new(&values.data_dir)),
            ReviewField::AgentFilesystemRoot => {
                *editing = Some(TextEditState::new(&values.agent_filesystem_root))
            }
            _ => {}
        },
        SessionReviewForm::Repl {
            values, editing, ..
        } => match field {
            ReviewField::Yes => values.yes = !values.yes,
            ReviewField::Language => {
                *editing = Some(TextEditState::new(&values.language));
            }
            ReviewField::StreamLogs => values.stream_logs = !values.stream_logs,
            ReviewField::AutomaticImports => values.auto_imports = !values.auto_imports,
            ReviewField::PostDeploy => values.post_deploy = values.post_deploy.next(),
            _ => {}
        },
    }
}

fn apply_session_form_edit(form: &mut SessionReviewForm, field: ReviewField, value: String) {
    match form {
        SessionReviewForm::Server { values, .. } => match field {
            ReviewField::SystemMemory => values.system_memory_override = value,
            ReviewField::RouterAddress => values.router_address = value,
            ReviewField::RouterPort => values.router_port = value,
            ReviewField::CustomRequestPort => values.custom_request_port = value,
            ReviewField::McpPort => values.mcp_port = value,
            ReviewField::PortsFile => values.ports_file = value,
            ReviewField::DataDirectory => values.data_dir = value,
            ReviewField::AgentFilesystemRoot => values.agent_filesystem_root = value,
            _ => {}
        },
        SessionReviewForm::Repl { values, .. } => {
            if field == ReviewField::Language {
                values.language = value;
            }
        }
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum PaletteScope {
    #[default]
    All,
    Context,
}

impl PaletteScope {
    fn next(self) -> Self {
        match self {
            Self::All => Self::Context,
            Self::Context => Self::All,
        }
    }

    fn previous(self) -> Self {
        self.next()
    }
}

#[derive(Debug, Clone, Default)]
struct CommandPalette {
    query: String,
    selected: usize,
    scope: PaletteScope,
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
    Switch(ContextSwitchRequest),
    LoadEnvironments { target: TuiContextTarget },
}

#[derive(Debug, Clone)]
struct ContextSwitchRequest {
    global_flags: GolemCliGlobalFlags,
    environment_reference: Option<EnvironmentReference>,
    server_identity: ServerIdentity,
    dev_eligible: bool,
}

pub(super) struct ContextSwitchResult {
    context: Arc<Context>,
    request: ContextSwitchRequest,
}

#[derive(Debug, Clone)]
struct AuthPromptState {
    url: String,
}

#[derive(Debug, Clone)]
struct TuiContextTarget {
    key: String,
    server_identity: ServerIdentity,
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
            server_identity: ServerIdentity::Unavailable,
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
    server_identity: ServerIdentity,
    server_label: String,
    server_flags: GolemCliGlobalFlags,
    environment_reference: Option<EnvironmentReference>,
    app: Option<String>,
    environment: Option<String>,
    dev_eligible: bool,
    error: Option<String>,
    is_current: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ServerIdentity {
    Local,
    Named(String),
    Unavailable,
}

impl ServerIdentity {
    fn from_context(context: &TuiContextInfo) -> Self {
        if context.uses_local_server {
            Self::Local
        } else if context.server == "cloud" || context.server.contains("golem.cloud") {
            Self::Named("cloud".to_string())
        } else {
            Self::Named(context.server.clone())
        }
    }

    fn from_server(server: Option<&Server>) -> Self {
        match server {
            None | Some(Server::Builtin(BuiltinServer::Local)) => Self::Local,
            Some(Server::Builtin(BuiltinServer::Cloud)) => Self::Named("cloud".to_string()),
            Some(Server::Custom(custom)) => Self::Named(custom.url.to_string()),
        }
    }
}

#[derive(Debug, Clone)]
struct SelectedTarget {
    context: TuiContextInfo,
    cli_args: Vec<String>,
    environment_reference: Option<EnvironmentReference>,
    server_identity: ServerIdentity,
}

impl SelectedTarget {
    fn from_context(context: &Context, flags: &GolemCliGlobalFlags) -> Self {
        let context = TuiContextInfo::from_context(context);
        let server_identity = ServerIdentity::from_context(&context);
        Self {
            context,
            cli_args: context_args_from_flags(flags),
            environment_reference: None,
            server_identity,
        }
    }
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
    if target.is_current {
        Some("current".to_string())
    } else if target.error.is_some() {
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
            let server_identity = ServerIdentity::from_server(environment.server.as_ref());
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
                        server_identity: server_identity.clone(),
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
                            server_identity: server_identity.clone(),
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
                server_identity: ServerIdentity::Local,
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
                server_identity: ServerIdentity::Named("cloud".to_string()),
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
                    server_identity: ServerIdentity::Named(format!("profile:{}", profile_name.0)),
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
                    server_identity: ServerIdentity::from_context(&current_context),
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
    target.is_current = target_matches_selected_context(
        &target,
        current_context,
        &ServerIdentity::from_context(current_context),
    );
    target
}

fn target_matches_selected_context(
    target: &TuiContextTarget,
    current_context: &TuiContextInfo,
    server_identity: &ServerIdentity,
) -> bool {
    match &target.kind {
        TuiContextTargetKind::ManifestAppContext { app, environment } => {
            current_context.application == *app
                && current_context.environment == *environment
                && &target.server_identity == server_identity
        }
        TuiContextTargetKind::ServerTarget { .. } => {
            current_context.application == "no application manifest"
                && &target.server_identity == server_identity
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
    current_server_identity: &ServerIdentity,
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
        let is_current = current_context.application == application_name.0
            && current_context.environment == environment_name.0
            && &server_target.server_identity == current_server_identity;
        let dev_eligible = current_context.dev_eligible && is_current;
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
            server_identity: server_target.server_identity.clone(),
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
            is_current,
        });
    }
    if targets.is_empty() {
        targets.push(TuiAppEnvironmentTarget {
            label: "No visible app environments".to_string(),
            detail: "This server returned no environments for the active credentials".to_string(),
            server_identity: server_target.server_identity.clone(),
            server_label: server_target.label.clone(),
            server_flags: server_target.global_flags.clone(),
            environment_reference: None,
            app: None,
            environment: None,
            dev_eligible: false,
            error: Some("no visible app environments".to_string()),
            is_current: false,
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
    let context_picker_rows = context_picker_layout_rows(&app.context_switcher);
    let snapshot = layout::compute(LayoutInput {
        area: frame.area(),
        active_workspace: app.active_workspace,
        dev_primary: app.dev_primary,
        focused_dev_view: app.dev_focus,
        mode: app.mode.current(),
        context_picker_rows,
        context_picker_selected: app.context_switcher.selected,
        context_picker_prefix_height: context_picker_prefix_height(&app.context_switcher),
        ops_details_visible: app.ops_details_requested(),
        dev_session_status_widths: footer_session_status_widths(app, frame.area().width),
        layout: app.layout.clone(),
    });
    app.geometry.update(snapshot.clone());

    render_shell_header(frame, snapshot.header, app);

    match app.active_workspace {
        Workspace::Dev => render_dev_workspace(frame, snapshot.workspace_body, app),
        Workspace::Ops => render_ops_workspace(frame, snapshot.workspace_body, app),
    }
    render_shell_footer(frame, snapshot.footer, app);

    match app.mode.current() {
        InteractionMode::Normal => {}
        InteractionMode::Leader => {}
        InteractionMode::Layout => {}
        InteractionMode::Jobs => render_jobs(frame, app),
        InteractionMode::QuitBlocked => render_quit_blocked(frame, app),
        InteractionMode::ContextPicker => render_context_picker(frame, app),
        InteractionMode::ContextSwitchConfirm => render_context_switch_confirm(frame, app),
        InteractionMode::AgentFilter => {}
        InteractionMode::AgentDatasetFilter => render_agent_dataset_filter(frame, app),
        InteractionMode::AgentColumns => render_agent_columns(frame, app),
        InteractionMode::OplogFilter => {}
        InteractionMode::MetricsRange => render_metrics_range_form(frame, app),
        InteractionMode::CommandForm => render_command_form(frame, app),
        InteractionMode::SessionForm => render_session_form(frame, app),
        InteractionMode::TerminalInput => {}
        InteractionMode::TerminalScrollback => {}
        InteractionMode::SshInput => {}
        InteractionMode::Palette => render_palette(frame, app),
        InteractionMode::Help => render_help(frame, app),
    }
}

fn render_ops_workspace(frame: &mut Frame<'_>, area: Rect, app: &TuiApp) {
    frame.render_widget(Paragraph::new("").style(surface_style()), area);
    let tabs = Rect {
        height: area.height.min(1),
        ..area
    };
    let content = layout::ops_content_area(area);
    render_ops_tabs(frame, tabs, app);
    match app.ops.active() {
        OpsDestination::Overview => render_ops_overview(frame, content, app),
        OpsDestination::Agents => match app.ops.current_route() {
            OpsRoute::Agent(identity) => render_ops_agent_overview(frame, content, app, identity),
            OpsRoute::Ssh(identity) => render_ops_ssh(frame, content, app, identity),
            _ => render_ops_agents_overview(frame, content, app),
        },
        OpsDestination::Oplog => render_ops_activity(frame, content, app),
        OpsDestination::Stream => render_ops_stream(frame, content, app),
        OpsDestination::Metrics => match app.ops.current_route() {
            OpsRoute::MetricsPanel(panel_id) => {
                render_ops_metric_panel_route(frame, content, app, panel_id)
            }
            _ => render_ops_metrics(frame, content, app),
        },
        OpsDestination::Deployments => render_ops_deployments(frame, content, app),
        OpsDestination::Configuration => render_ops_resources(frame, content, app),
    }
}

fn render_ops_tabs(frame: &mut Frame<'_>, area: Rect, app: &TuiApp) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let visual = theme();
    let surface = surface_style();
    frame.render_widget(Paragraph::new("").style(surface), area);
    frame.render_widget(
        Paragraph::new(Span::styled("├─", surface.fg(visual.border_subtle))),
        Rect {
            width: area.width.min(2),
            ..area
        },
    );

    let available = area.width.saturating_sub(2) as usize;
    if available == 0 {
        return;
    }
    let active = app.ops.active().index();
    let widths = OpsDestination::ALL
        .iter()
        .map(|destination| destination.label().chars().count() + 2)
        .collect::<Vec<_>>();
    let mut start = app.ops.tab_offset().min(active);
    while start < active {
        let left_marker = usize::from(start > 0) * 2;
        let through_active = widths[start..=active].iter().sum::<usize>()
            + active.saturating_sub(start)
            + left_marker;
        if through_active <= available {
            break;
        }
        start += 1;
    }

    let mut used = usize::from(start > 0) * 2;
    let mut end = start;
    while end < OpsDestination::ALL.len() {
        let separator = usize::from(end > start);
        let right_marker = usize::from(end + 1 < OpsDestination::ALL.len()) * 2;
        if used + separator + widths[end] + right_marker > available {
            break;
        }
        used += separator + widths[end];
        end += 1;
    }
    if end <= active {
        end = (active + 1).min(OpsDestination::ALL.len());
    }
    app.ops.set_tab_offset(start);

    let mut x = area.x.saturating_add(2);
    if start > 0 && x < area.right() {
        let marker = Rect::new(x, area.y, area.right().saturating_sub(x).min(2), 1);
        frame.render_widget(
            Paragraph::new(Span::styled("‹ ", surface.fg(visual.text_muted))),
            marker,
        );
        x = x.saturating_add(marker.width);
    }
    for (visible_index, destination) in OpsDestination::ALL[start..end].iter().enumerate() {
        if visible_index > 0 && x < area.right() {
            frame.render_widget(
                Paragraph::new(Span::styled("·", surface.fg(visual.border_subtle))),
                Rect::new(x, area.y, 1, 1),
            );
            x = x.saturating_add(1);
        }
        let selected = *destination == app.ops.active();
        let label = if selected {
            format!("[{}]", destination.label())
        } else {
            format!(" {} ", destination.label())
        };
        let width = (label.chars().count() as u16).min(area.right().saturating_sub(x));
        if width == 0 {
            break;
        }
        let style = if selected {
            surface.fg(visual.accent_hover).add_modifier(Modifier::BOLD)
        } else {
            surface.fg(visual.text_muted)
        };
        let tab_area = Rect::new(x, area.y, width, 1);
        frame.render_widget(Paragraph::new(Span::styled(label, style)), tab_area);
        app.geometry
            .add_region(RegionKind::OpsTab(*destination), tab_area);
        x = x.saturating_add(width);
    }
    if end < OpsDestination::ALL.len() && x < area.right() {
        frame.render_widget(
            Paragraph::new(Span::styled(" ›", surface.fg(visual.text_muted))),
            Rect::new(x, area.y, area.right().saturating_sub(x).min(2), 1),
        );
    }
}

fn render_ops_related_links(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &TuiApp,
    links: &[(OpsDestination, &str, bool)],
    reserved_left: u16,
    reserved_right: u16,
) {
    let actions = links
        .iter()
        .map(|(_, label, available)| PaneHeaderAction {
            label,
            available: *available,
        })
        .collect::<Vec<_>>();
    let regions = PaneHeaderActions { actions: &actions }.render(
        frame,
        area,
        reserved_left,
        reserved_right,
        &theme(),
    );
    for ((destination, _, _), region) in links.iter().zip(regions) {
        if let Some(region) = region {
            app.geometry
                .add_region(RegionKind::OpsLink(*destination), region);
        }
    }
}

fn render_ops_control_strip(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &TuiApp,
    groups: &[ControlGroup<'_>],
    status: Option<&str>,
) {
    for (group, choice, region) in (ControlStrip { groups, status }).render(frame, area, &theme()) {
        app.geometry
            .add_region(RegionKind::OpsControl { group, choice }, region);
    }
}

fn adaptive_ops_pane_layout(area: Rect, horizontal_at: u16, weights: &[u32]) -> PaneLayout {
    if area.width == 0 || area.height == 0 {
        return PaneLayout {
            panes: vec![area; weights.len()],
            dividers: Vec::new(),
        };
    }
    if area.width >= horizontal_at {
        PaneLayout::horizontal(area, weights)
    } else {
        let total = weights.iter().copied().sum::<u32>().max(1);
        let panes = Layout::default()
            .direction(Direction::Vertical)
            .constraints(
                weights
                    .iter()
                    .map(|weight| Constraint::Ratio((*weight).max(1), total)),
            )
            .split(area)
            .to_vec();
        PaneLayout {
            panes,
            dividers: Vec::new(),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn render_ops_selection_list(
    frame: &mut Frame<'_>,
    body: Rect,
    chrome: PaneChrome,
    app: &TuiApp,
    lines: &[Line<'static>],
    selected: usize,
    first_visible: &Cell<usize>,
    viewport_height: &Cell<u16>,
    region_kind: RegionKind,
    scrollbar_target: ScrollbarTarget,
) {
    let region = chrome.region(body, lines.len());
    viewport_height.set(region.content_area.height);
    let start = selection_window_start(
        selected,
        lines.len(),
        region.content_area.height,
        first_visible.get(),
    );
    first_visible.set(start);
    app.geometry.add_region(region_kind, region.content_area);
    ScrollableDocument {
        lines,
        offset: start,
    }
    .render(frame, region, &theme());
    if let Some(slot) = region.scrollbar {
        app.geometry
            .add_region(RegionKind::Scrollbar(scrollbar_target), slot.area);
    }
}

fn selection_window_start(
    selected: usize,
    count: usize,
    viewport_height: u16,
    current: usize,
) -> usize {
    if count == 0 {
        return 0;
    }
    let capacity = viewport_height.max(1) as usize;
    let mut first = current.min(count.saturating_sub(1));
    if selected < first {
        first = selected;
    } else if selected >= first.saturating_add(capacity) {
        first = selected.saturating_add(1).saturating_sub(capacity);
    }
    first.min(count.saturating_sub(capacity))
}

fn render_ops_overview(frame: &mut Frame<'_>, area: Rect, app: &TuiApp) {
    match app.ops.current_route() {
        OpsRoute::OverviewServer => render_overview_server(frame, area, app),
        OpsRoute::OverviewIssues => render_overview_issues(frame, area, app),
        _ => render_overview_cockpit(frame, area, app),
    }
}

fn render_overview_cockpit(frame: &mut Frame<'_>, area: Rect, app: &TuiApp) {
    let chrome = PaneChrome { area };
    chrome.render_header(frame, area, "Overview", PaneFocus::Active, &theme());
    let body = chrome.body_area(area);
    let content = chrome.content_area(body);
    if content.width == 0 || content.height == 0 {
        chrome.render_body_boundary(frame, body, &theme());
        return;
    }

    let show_status = content.height >= 9;
    let status_height = if show_status { 5 } else { 0 };
    let [status_area, links_area] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(status_height), Constraint::Min(1)])
        .areas(content);
    if show_status {
        let mut lines = vec![
            SectionHeading {
                title: "Environment",
                detail: Some("current state"),
            }
            .line(&theme()),
        ];
        lines.extend(overview_status_lines(app));
        frame.render_widget(Paragraph::new(lines).style(surface_style()), status_area);
    }

    if links_area.height > 0 {
        let title_height = 1;
        let list_area = Rect {
            y: links_area.y.saturating_add(title_height),
            height: links_area.height.saturating_sub(title_height),
            ..links_area
        };
        frame.render_widget(
            Paragraph::new(
                SectionHeading {
                    title: "Open",
                    detail: Some("enter or click"),
                }
                .line(&theme()),
            )
            .style(surface_style()),
            Rect {
                height: links_area.height.min(title_height),
                ..links_area
            },
        );
        let visible = list_area.height.max(1) as usize;
        let selected = app
            .overview
            .selected_item
            .min(OverviewItem::ALL.len().saturating_sub(1));
        let start = selected
            .saturating_sub(visible.saturating_sub(1) / 2)
            .min(OverviewItem::ALL.len().saturating_sub(visible));
        let issues = app.overview_issues();
        let lines = OverviewItem::ALL
            .iter()
            .enumerate()
            .skip(start)
            .take(visible)
            .enumerate()
            .map(|(visible_index, (index, item))| {
                let row = Rect::new(
                    list_area.x,
                    list_area.y.saturating_add(visible_index as u16),
                    list_area.width,
                    1,
                );
                app.geometry
                    .add_region(RegionKind::OverviewItem(index), row);
                let description = if *item == OverviewItem::Issues {
                    if issues.is_empty() {
                        "No actionable diagnostics".to_string()
                    } else {
                        format!(
                            "{} actionable diagnostic{}",
                            issues.len(),
                            if issues.len() == 1 { "" } else { "s" }
                        )
                    }
                } else {
                    item.description().to_string()
                };
                NavigationRow {
                    label: item.label(),
                    description: &description,
                    selected: index == selected,
                }
                .line(list_area.width as usize, &theme())
            })
            .collect::<Vec<_>>();
        frame.render_widget(Paragraph::new(lines).style(surface_style()), list_area);
    }
    chrome.render_body_boundary(frame, body, &theme());
}

fn overview_status_lines(app: &TuiApp) -> Vec<Line<'static>> {
    let deployment = if let Some(error) = &app.ops_environment.error {
        (
            StatusKind::Failed,
            format!("unavailable · {}", fit(error, 48)),
        )
    } else if let Some(deployment) = app
        .ops_environment
        .snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.deployment.as_ref())
    {
        (
            StatusKind::Running,
            format!(
                "revision {} · {}",
                deployment.deployment_revision, deployment.version
            ),
        )
    } else if app.ops_environment.loading {
        (StatusKind::Idle, "loading…".to_string())
    } else {
        (StatusKind::Idle, "not deployed".to_string())
    };
    let collector = if let Some(error) = &app.overview.observability.error {
        (
            StatusKind::Failed,
            format!("unavailable · {}", fit(error, 48)),
        )
    } else if let Some(capability) = app
        .overview
        .observability
        .value
        .as_ref()
        .and_then(|snapshot| snapshot.capability.as_ref())
    {
        let kind = if capability.status
            == crate::model::local_observability::LocalObservabilityStatus::Ready
        {
            StatusKind::Running
        } else {
            StatusKind::Failed
        };
        (kind, format!("{:?}", capability.status).to_lowercase())
    } else if app.overview.observability.loading {
        (StatusKind::Idle, "checking…".to_string())
    } else {
        (StatusKind::Idle, "unavailable".to_string())
    };
    let stream_kind = match app.stream.status {
        StreamStatus::Connected | StreamStatus::Connecting | StreamStatus::Reconnecting { .. } => {
            StatusKind::Running
        }
        StreamStatus::Failed(_) => StatusKind::Failed,
        StreamStatus::Disconnected => StatusKind::Idle,
    };
    vec![
        overview_status_line("Deployment", deployment.0, deployment.1),
        overview_status_line(
            "Local server",
            server_status_kind(app.server.run.status),
            app.server.run.status.title(),
        ),
        overview_status_line("Collector", collector.0, collector.1),
        overview_status_line(
            "Stream",
            stream_kind,
            format!(
                "{} · {} unread",
                app.stream.status.label(),
                app.stream.unread
            ),
        ),
    ]
}

fn overview_status_line(
    label: impl Into<String>,
    kind: StatusKind,
    value: impl Into<String>,
) -> Line<'static> {
    let value = value.into();
    let mut spans = vec![Span::styled(
        fit(&label.into(), 14),
        Style::default().fg(theme().text_muted),
    )];
    spans.extend(
        StatusMarker {
            kind,
            label: &value,
        }
        .spans(&theme()),
    );
    Line::from(spans)
}

fn render_overview_server(frame: &mut Frame<'_>, area: Rect, app: &TuiApp) {
    let mut lines = vec![
        overview_value_line("Nested server", app.server.run.status.title()),
        overview_value_line(
            "Connected endpoint",
            app.selected_target.context.stable_id(),
        ),
        overview_value_line("Context", app.selected_target.context.provenance()),
    ];
    if let Some(ports) = &app.server.run.bound_ports {
        lines.push(overview_value_line(
            "Observed ports",
            format!(
                "router {} · request {} · MCP {}",
                ports.router_port, ports.custom_request_port, ports.mcp_port
            ),
        ));
    }
    lines.push(Line::default());
    let state = &app.overview.observability;
    if state.loading && state.value.is_none() {
        lines.extend(overview_loading_lines(
            "Checking local observability capability",
        ));
    } else if let Some(error) = &state.error {
        lines.push(overview_error_line(error));
    } else if let Some(capability) = state
        .value
        .as_ref()
        .and_then(|snapshot| snapshot.capability.as_ref())
    {
        for (label, value) in [
            ("Collector", format!("{:?}", capability.status)),
            (
                "Message",
                capability
                    .message
                    .clone()
                    .unwrap_or_else(|| "none".to_string()),
            ),
            (
                "Exporter",
                format!(
                    "{} {}",
                    capability.exporter_name, capability.exporter_version
                ),
            ),
            (
                "Sample interval",
                format!("{} seconds", capability.sample_interval_seconds),
            ),
            ("Freshness", overview_capability_freshness(capability)),
            (
                "Retention",
                format!(
                    "{} days · {}",
                    capability.retention_days,
                    overview_bytes(capability.retention_bytes)
                ),
            ),
            ("Discarded points", capability.discarded_points.to_string()),
        ] {
            lines.push(overview_value_line(label, value));
        }
    } else {
        lines.push(overview_warning_line(
            "The selected target does not expose local observability",
        ));
    }
    render_ops_document(frame, area, app, "Overview / Server & collector", lines);
}

fn render_overview_issues(frame: &mut Frame<'_>, area: Rect, app: &TuiApp) {
    let chrome = PaneChrome { area };
    chrome.render_header(
        frame,
        area,
        "Overview / Issues",
        PaneFocus::Active,
        &theme(),
    );
    let body = chrome.body_area(area);
    let content = chrome.content_area(body);
    let issues = app.overview_issues();
    if issues.is_empty() {
        render_metrics_notice(
            frame,
            content,
            NoticeKind::Empty,
            "No actionable issues detected",
        );
        chrome.render_body_boundary(frame, body, &theme());
        return;
    }
    let detail_height = if content.height >= 6 { 3 } else { 0 };
    let [list, detail] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(detail_height)])
        .areas(content);
    let visible = list.height.max(1) as usize;
    let start = app
        .overview
        .selected_issue
        .saturating_sub(visible.saturating_sub(1) / 2)
        .min(issues.len().saturating_sub(visible));
    let lines = issues
        .iter()
        .enumerate()
        .skip(start)
        .take(visible)
        .enumerate()
        .map(|(visible_index, (index, issue))| {
            app.geometry.add_region(
                RegionKind::OverviewIssue(index),
                Rect::new(
                    list.x,
                    list.y.saturating_add(visible_index as u16),
                    list.width,
                    1,
                ),
            );
            metric_selection_line(
                issue.title.clone(),
                index == app.overview.selected_issue,
                list.width,
            )
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines).style(surface_style()), list);
    if detail.height > 0
        && let Some(issue) = issues.get(app.overview.selected_issue)
    {
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    issue.detail.clone(),
                    Style::default().fg(theme().text_secondary),
                )),
                overview_muted_line("Enter opens the owning Ops destination"),
            ])
            .style(surface_style())
            .wrap(Wrap { trim: true }),
            detail,
        );
    }
    chrome.render_body_boundary(frame, body, &theme());
}

fn overview_loading_lines(message: impl Into<String>) -> Vec<Line<'static>> {
    vec![Line::from(Span::styled(
        format!("◌ {}…", message.into()),
        Style::default().fg(theme().info),
    ))]
}

fn overview_value_line(label: impl Into<String>, value: impl Into<String>) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{}  ", label.into()),
            Style::default().fg(theme().text_muted),
        ),
        Span::styled(value.into(), Style::default().fg(theme().text)),
    ])
}

fn overview_muted_line(value: impl Into<String>) -> Line<'static> {
    Line::from(Span::styled(
        value.into(),
        Style::default().fg(theme().text_muted),
    ))
}

fn overview_warning_line(value: impl Into<String>) -> Line<'static> {
    Line::from(Span::styled(
        value.into(),
        Style::default().fg(theme().marker),
    ))
}

fn overview_error_line(value: impl Into<String>) -> Line<'static> {
    Line::from(Span::styled(
        value.into(),
        Style::default().fg(theme().error),
    ))
}

fn overview_capability_freshness(
    capability: &crate::model::local_observability::LocalObservabilityCapability,
) -> String {
    let service = capability
        .last_service_sample_unix_nanos
        .map(metric_freshness_label)
        .unwrap_or_else(|| "no Server samples".to_string());
    let agent = capability
        .last_agent_ingest_unix_nanos
        .map(metric_freshness_label)
        .unwrap_or_else(|| "no Agent samples".to_string());
    format!("Server {service} · Agent {agent}")
}

fn overview_bytes(bytes: u64) -> String {
    const GIB: u64 = 1024 * 1024 * 1024;
    const MIB: u64 = 1024 * 1024;
    if bytes >= GIB && bytes.is_multiple_of(GIB) {
        format!("{} GiB", bytes / GIB)
    } else if bytes >= MIB && bytes.is_multiple_of(MIB) {
        format!("{} MiB", bytes / MIB)
    } else {
        format!("{bytes} bytes")
    }
}

fn render_ops_stream(frame: &mut Frame<'_>, area: Rect, app: &TuiApp) {
    let chrome = PaneChrome { area };
    let target = app
        .ops
        .current_stream()
        .or_else(|| app.stream.target.as_ref());
    let title = target.map_or_else(
        || "Stream".to_string(),
        |target| format!("Stream / {}", target.agent_id),
    );
    let mut status_lines = vec![stream_status_line(app, target, area.width as usize)];
    if let StreamStatus::Reconnecting {
        attempt,
        delay,
        error,
    } = &app.stream.status
    {
        let detail = match error {
            Some(error) => format!(
                "attempt {attempt} · retry in {:.2}s · {}",
                delay.as_secs_f64(),
                plain_tui_text(error)
            ),
            None => format!(
                "attempt {attempt} · retry in {:.2}s · connection closed",
                delay.as_secs_f64()
            ),
        };
        status_lines.push(Line::from(Span::styled(
            fit(&detail, area.width as usize),
            Style::default().fg(theme().text_muted),
        )));
    } else if let StreamStatus::Failed(error) = &app.stream.status {
        status_lines.push(Line::from(Span::styled(
            fit(&plain_tui_text(error), area.width as usize),
            Style::default().fg(theme().error),
        )));
    }
    let status_height = status_lines
        .len()
        .min(area.height.saturating_sub(1) as usize) as u16;
    chrome.render_header(frame, area, &title, PaneFocus::Active, &theme());
    let target_available = target.is_some();
    let related_links = [
        (OpsDestination::Agents, "Agent", target_available),
        (OpsDestination::Oplog, "Oplog", target_available),
        (OpsDestination::Metrics, "Metrics", target_available),
    ];
    render_ops_related_links(
        frame,
        area,
        app,
        &related_links,
        (UnicodeWidthStr::width(title.as_str()) as u16).saturating_add(6),
        0,
    );
    let outer_body = chrome.body_area(area);
    let [status_row, body] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(status_height), Constraint::Min(0)])
        .areas(outer_body);
    let status = chrome.content_area(status_row);
    frame.render_widget(Paragraph::new(status_lines).style(surface_style()), status);

    let pane_layout = PaneLayout::horizontal(body, &[1]);
    let content_length = app.stream.events.len().max(1);
    let Some(region) = pane_layout.regions(&[content_length]).into_iter().next() else {
        chrome.render_body_boundary(frame, outer_body, &theme());
        return;
    };
    app.geometry.add_region(RegionKind::OpsStream, body);
    let viewport_height = region.content_area.height as usize;
    let range = app.stream.visible_range(viewport_height);
    let lines = if app.stream.events.is_empty() {
        let (kind, message) = if target.is_none() {
            (
                NoticeKind::Empty,
                "Select one exact Agent in Agents, then use ctrl+k to connect",
            )
        } else if matches!(app.stream.status, StreamStatus::Connecting) {
            (NoticeKind::Info, "Connecting to the selected Agent")
        } else {
            (
                NoticeKind::Empty,
                "No buffered events · use ctrl+k to connect",
            )
        };
        vec![Notice { kind, message }.line(&theme())]
    } else {
        app.stream
            .events
            .range(range.clone())
            .map(|item| stream_item_line(item, region.content_area.width as usize))
            .collect()
    };
    frame.render_widget(
        Paragraph::new(lines).style(surface_style()),
        region.content_area,
    );
    if let Some(slot) = region.scrollbar {
        let mut state = ScrollbarState::new(app.stream.events.len())
            .position(range.start)
            .viewport_content_length(viewport_height);
        render_scrollbar(frame, slot, &mut state, &theme());
        app.geometry
            .add_region(RegionKind::Scrollbar(ScrollbarTarget::OpsStream), slot.area);
    }
    pane_layout.render_body_boundaries(frame, &[region], &theme());
    chrome.render_body_boundary(frame, outer_body, &theme());
}

fn stream_status_line(
    app: &TuiApp,
    route_target: Option<&AgentIdentity>,
    width: usize,
) -> Line<'static> {
    let visual = theme();
    let status_color = match app.stream.status {
        StreamStatus::Connected => visual.success,
        StreamStatus::Connecting | StreamStatus::Reconnecting { .. } => visual.accent_hover,
        StreamStatus::Failed(_) => visual.error,
        StreamStatus::Disconnected => visual.text_muted,
    };
    let target = route_target
        .map(|target| format!("{}/{}", target.component, target.agent_id))
        .unwrap_or_else(|| "one exact Agent required".to_string());
    let summary = format!(
        "{} · {}/{} events · {} unread · {} server missed · {} local dropped · {} / {}",
        if app.stream.follow {
            "Follow"
        } else {
            "Paused"
        },
        app.stream.events.len(),
        stream::MAX_EVENTS,
        app.stream.unread,
        app.stream.server_missed,
        app.stream.locally_dropped,
        format_bytes(app.stream.buffered_bytes),
        format_bytes(stream::MAX_BYTES),
    );
    let prefix = format!("[{}] ", app.stream.status.label());
    let suffix = format!("{target} · {summary}");
    Line::from(vec![
        Span::styled(prefix, Style::default().fg(status_color)),
        Span::styled(
            fit(
                &suffix,
                width.saturating_sub(app.stream.status.label().len() + 3),
            ),
            Style::default().fg(visual.text_secondary),
        ),
    ])
}

fn stream_item_line(item: &AgentStreamItem, width: usize) -> Line<'static> {
    let visual = theme();
    let timestamp = item.timestamp().to_string();
    let timestamp = timestamp
        .split_once('T')
        .map(|(_, time)| time.trim_end_matches('Z'))
        .unwrap_or(timestamp.as_str());
    let (kind, message, color) = match item {
        AgentStreamItem::Stdout { message, .. } => ("OUT", message.clone(), visual.text),
        AgentStreamItem::Stderr { message, .. } => ("ERR", message.clone(), visual.error),
        AgentStreamItem::Log {
            level,
            context,
            message,
            ..
        } => {
            let color = match level {
                LogLevel::Trace | LogLevel::Debug => visual.text_muted,
                LogLevel::Info => visual.info,
                LogLevel::Warn => visual.accent_hover,
                LogLevel::Error | LogLevel::Critical => visual.error,
            };
            ("LOG", format!("{} [{}] {message}", level, context), color)
        }
        AgentStreamItem::InvocationStarted {
            function,
            idempotency_key,
            ..
        } => (
            "CALL",
            format!("started {function} · {idempotency_key}"),
            visual.accent_hover,
        ),
        AgentStreamItem::InvocationFinished {
            function,
            idempotency_key,
            ..
        } => (
            "CALL",
            format!("finished {function} · {idempotency_key}"),
            visual.success,
        ),
        AgentStreamItem::PluginError {
            plugin_name,
            message,
            ..
        } => ("PLUGIN", format!("{plugin_name} · {message}"), visual.error),
        AgentStreamItem::SnapshotRecoverySucceeded { snapshot_index, .. } => (
            "RECOVER",
            format!("loaded snapshot from oplog index {snapshot_index}"),
            visual.success,
        ),
        AgentStreamItem::SnapshotRecoveryFailed {
            snapshot_index,
            error,
            ..
        } => (
            "RECOVER",
            format!("snapshot {snapshot_index} failed · {error}"),
            visual.error,
        ),
        AgentStreamItem::ServerLagged { missed, .. } => (
            "LAG",
            format!("server reports {missed} missed events"),
            visual.accent_hover,
        ),
        AgentStreamItem::StreamClosed { .. } => {
            ("STREAM", "connection closed".to_string(), visual.text_muted)
        }
        AgentStreamItem::StreamError { error, .. } => (
            "STREAM",
            format!("connection error · {error}"),
            visual.error,
        ),
    };
    let message = message.replace(['\r', '\n'], " ↵ ");
    let prefix = format!("{timestamp:<12} {kind:<7} ");
    Line::from(vec![
        Span::styled(prefix.clone(), Style::default().fg(visual.text_muted)),
        Span::styled(
            fit(&message, width.saturating_sub(prefix.chars().count())),
            Style::default().fg(color),
        ),
    ])
}

fn format_bytes(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MiB", bytes as f64 / (1024 * 1024) as f64)
    } else if bytes >= 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

fn render_ops_activity(frame: &mut Frame<'_>, area: Rect, app: &TuiApp) {
    let chrome = PaneChrome { area };
    chrome.render_header(frame, area, "Oplog", PaneFocus::Active, &theme());
    let outer_body = chrome.body_area(area);
    let [controls_row, query_row, map_row, body] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .areas(outer_body);
    let controls = chrome.content_area(controls_row);
    let query_bar = chrome.content_area(query_row);
    let representation = app.activity.representation;
    let representation_choices = [
        ControlChoice {
            label: "Grouped",
            selected: representation == ActivityRepresentation::Grouped,
        },
        ControlChoice {
            label: "Entries",
            selected: representation == ActivityRepresentation::Entries,
        },
    ];
    let order_choices = [
        ControlChoice {
            label: "Newest",
            selected: app.activity.order == ActivityOrder::Newest,
        },
        ControlChoice {
            label: "Oldest",
            selected: app.activity.order == ActivityOrder::Oldest,
        },
    ];
    let input_choices = [
        ControlChoice {
            label: "Loaded",
            selected: app.activity.input_kind == ActivityInputKind::Loaded,
        },
        ControlChoice {
            label: "Server",
            selected: app.activity.input_kind == ActivityInputKind::Server,
        },
    ];
    let groups = [
        ControlGroup {
            label: "View",
            choices: &representation_choices,
        },
        ControlGroup {
            label: "Order",
            choices: &order_choices,
        },
        ControlGroup {
            label: "Find",
            choices: &input_choices,
        },
    ];
    let (load_label, load_tone) = if app.activity.loading {
        (Some("loading"), PaneStatusTone::Loading)
    } else if app.activity.stopped {
        (Some("stopped"), PaneStatusTone::Idle)
    } else if app.activity.paused {
        (Some("more available"), PaneStatusTone::Active)
    } else {
        (None, PaneStatusTone::Idle)
    };
    if let Some(label) = load_label {
        PaneHeaderStatus {
            label,
            tone: load_tone,
        }
        .render(frame, area, &theme());
    }
    let oplog_scope = app.ops_handoff_scope();
    let selected_agent = app.related_exact_agent().is_some();
    let related_links = [
        (OpsDestination::Agents, "Agent", selected_agent),
        (
            OpsDestination::Metrics,
            "Metrics",
            !oplog_scope.is_empty() && oplog_scope.len() <= activity::HARD_AGENT_LIMIT,
        ),
        (
            OpsDestination::Stream,
            "Stream",
            app.related_stream_available(),
        ),
    ];
    render_ops_related_links(
        frame,
        area,
        app,
        &related_links,
        12,
        u16::from(load_label.is_some()).saturating_mul(18),
    );
    let scan_progress = app
        .activity
        .progress
        .iter()
        .filter_map(|progress| {
            progress
                .scanned_through_index
                .map(|scanned| (scanned, progress.last_index))
        })
        .max_by_key(|(scanned, _)| *scanned)
        .map(|(scanned, last)| format!("Scan #{scanned}/#{last}"))
        .unwrap_or_default();
    let scan_bound = if app
        .activity
        .progress
        .iter()
        .any(|progress| progress.scan_limit_reached && progress.has_more)
    {
        "bounded"
    } else {
        ""
    };
    let time_filter = match (
        app.activity.applied_query.from,
        app.activity.applied_query.to,
    ) {
        (Some(from), Some(to)) => format!(
            "Time {}→{}",
            from.format("%H:%M:%S%.3fZ"),
            to.format("%H:%M:%S%.3fZ")
        ),
        (Some(from), None) => format!("From {}", from.format("%H:%M:%S%.3fZ")),
        (None, Some(to)) => format!("To {}", to.format("%H:%M:%S%.3fZ")),
        (None, None) => String::new(),
    };
    let scope_status = match app.activity.agents.as_slice() {
        [agent] => format!(
            "Agent {}",
            fit_middle(&format!("{}/{}", agent.component, agent.agent_id), 42)
        ),
        agents => format!("Scope {} Agents", agents.len()),
    };
    let control_status = [
        Some(scope_status),
        (!app.activity.loaded_filter.is_empty()).then(|| {
            format!(
                "Matches {}/{} loaded",
                activity::filtered_entries(&app.activity.entries, &app.activity.loaded_filter)
                    .len(),
                app.activity.entries.len()
            )
        }),
        app.activity
            .applied_query
            .active()
            .then_some("Sparse server hits".to_string()),
        (!time_filter.is_empty()).then_some(time_filter),
        (!scan_progress.is_empty()).then_some(scan_progress),
        (!scan_bound.is_empty()).then_some(scan_bound.to_string()),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("   ");
    render_ops_control_strip(frame, controls, app, &groups, Some(&control_status));
    let query_state = match app.activity.input_kind {
        ActivityInputKind::Loaded => "(loaded only)",
        ActivityInputKind::Server if app.activity.query == app.activity.applied_query.text => {
            "(applied)"
        }
        ActivityInputKind::Server => "[enter to run]",
    };
    let status_width = (UnicodeWidthStr::width(query_state) + 3)
        .min(query_bar.width.saturating_sub(1) as usize) as u16;
    let [query_input, query_status] = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(1), Constraint::Length(status_width)])
        .areas(query_bar);
    let (input, placeholder) = match app.activity.input_kind {
        ActivityInputKind::Loaded => (
            app.activity.loaded_filter.as_str(),
            "filter loaded entries (ctrl+s for server search)",
        ),
        ActivityInputKind::Server => (
            app.activity.query.as_str(),
            "search the server (ctrl+f for loaded filter)",
        ),
    };
    let search = SearchInput {
        query: input,
        placeholder,
    };
    frame.render_widget(
        Paragraph::new(search.line(query_input.width as usize, &theme())).style(surface_style()),
        query_input,
    );
    frame.render_widget(
        Paragraph::new(Span::styled(
            query_state.to_string(),
            Style::default().fg(theme().text_muted),
        ))
        .style(surface_style()),
        query_status,
    );
    if app.mode == InteractionMode::OplogFilter && query_input.width > 0 {
        frame.set_cursor_position(Position::new(
            query_input
                .x
                .saturating_add(search.cursor_column(query_input.width as usize) as u16),
            query_input.y,
        ));
    }
    render_activity_map(frame, chrome.content_area(map_row), app);

    if let Some(error) = &app.activity.error {
        render_metrics_notice(frame, chrome.content_area(body), NoticeKind::Warning, error);
        chrome.render_body_boundary(frame, outer_body, &theme());
        return;
    }
    if app.activity.loading && app.activity.entries.is_empty() {
        render_metrics_notice(
            frame,
            chrome.content_area(body),
            NoticeKind::Info,
            "Loading durable Agent activity…",
        );
        chrome.render_body_boundary(frame, outer_body, &theme());
        return;
    }
    if app.activity.entries.is_empty() {
        render_metrics_notice(
            frame,
            chrome.content_area(body),
            NoticeKind::Empty,
            "No oplog entries in this scope",
        );
        chrome.render_body_boundary(frame, outer_body, &theme());
        return;
    }

    let warning_rows = usize::from(app.activity.agents.len() > activity::SOFT_AGENT_LIMIT)
        + usize::from(!app.activity.failures.is_empty());
    let [notices, content] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(warning_rows.min(u16::MAX as usize) as u16),
            Constraint::Min(1),
        ])
        .areas(body);
    let mut notice_lines = Vec::new();
    if app.activity.agents.len() > activity::SOFT_AGENT_LIMIT {
        notice_lines.push(
            Notice {
                kind: NoticeKind::Warning,
                message: "Large Oplog scope: 100 entries are read per Agent and batch",
            }
            .line(&theme()),
        );
    }
    if !app.activity.failures.is_empty() {
        let first = &app.activity.failures[0];
        let message = format!(
            "Partial result: {} Agent(s) failed; first {}: {}",
            app.activity.failures.len(),
            first.agent_id,
            plain_tui_text(&first.message)
        );
        notice_lines.push(
            Notice {
                kind: NoticeKind::Warning,
                message: &message,
            }
            .line(&theme()),
        );
    }
    frame.render_widget(
        Paragraph::new(notice_lines).style(surface_style()),
        chrome.content_area(notices),
    );

    render_activity_explorer(frame, content, app, chrome);
    chrome.render_body_boundary(frame, outer_body, &theme());
}

fn render_activity_map(frame: &mut Frame<'_>, area: Rect, app: &TuiApp) {
    app.geometry.remove_region(RegionKind::OplogMap);
    app.geometry.add_region(RegionKind::OplogMap, area);
    if area.width == 0 || area.height == 0 {
        return;
    }
    let labels = activity_list_labels(app);
    let prefix = "loaded map ";
    let prefix_width = UnicodeWidthStr::width(prefix).min(area.width as usize);
    let available = area.width as usize - prefix_width;
    let visual = theme();
    let mut spans = vec![Span::styled(
        fit(prefix, prefix_width),
        Style::default().fg(visual.text_muted),
    )];
    let selected_cell = app
        .activity
        .selected
        .saturating_mul(available.saturating_sub(1))
        .checked_div(labels.len().saturating_sub(1).max(1))
        .unwrap_or(0)
        .min(available.saturating_sub(1));
    for cell in 0..available {
        let index = cell
            .saturating_mul(labels.len().saturating_sub(1))
            .checked_div(available.saturating_sub(1).max(1))
            .unwrap_or(0)
            .min(labels.len().saturating_sub(1));
        let label = labels.get(index);
        let selected = !labels.is_empty() && cell == selected_cell;
        let color = label
            .map(|label| match label.category {
                crate::model::agent::oplog::OplogEntryCategory::Invocation => visual.accent,
                crate::model::agent::oplog::OplogEntryCategory::HostCall => visual.info,
                crate::model::agent::oplog::OplogEntryCategory::Log => visual.success,
                crate::model::agent::oplog::OplogEntryCategory::Failure => visual.error,
                crate::model::agent::oplog::OplogEntryCategory::Trace => visual.marker,
                crate::model::agent::oplog::OplogEntryCategory::State => visual.text_secondary,
                crate::model::agent::oplog::OplogEntryCategory::Lifecycle => visual.accent_hover,
                crate::model::agent::oplog::OplogEntryCategory::Other => visual.text_muted,
            })
            .unwrap_or(visual.text_faint);
        spans.push(Span::styled(
            if selected { "┃" } else { "━" },
            Style::default().fg(color),
        ));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(surface_style()),
        area,
    );
}

fn render_activity_explorer(frame: &mut Frame<'_>, area: Rect, app: &TuiApp, chrome: PaneChrome) {
    let (list_pane, divider, details_pane) = layout::ops_pane_regions(
        area,
        app.activity.details_visible,
        app.layout.ops_details_placement,
        app.layout.ops_details_ratio,
    );
    app.geometry.remove_region(RegionKind::OpsDetails);
    app.geometry.remove_region(RegionKind::OpsDetailsSplit);
    app.geometry.add_region(RegionKind::OpsList, list_pane);
    if let Some(details_pane) = details_pane {
        app.geometry
            .add_region(RegionKind::OpsDetails, details_pane);
    }
    if let Some(divider) = divider {
        app.geometry
            .add_region(RegionKind::OpsDetailsSplit, divider);
    }

    let [list_header, list_body] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .areas(list_pane);
    let list_title = match app.activity.representation {
        ActivityRepresentation::Grouped if app.activity.applied_query.active() => {
            "Search hits · explicit links only"
        }
        ActivityRepresentation::Grouped => "Journal",
        ActivityRepresentation::Entries => "Entries",
    };
    chrome.render_header(
        frame,
        list_pane,
        list_title,
        if details_pane.is_none() || app.activity.focus == ActivityFocus::List {
            PaneFocus::Active
        } else {
            PaneFocus::Idle
        },
        &theme(),
    );
    let labels = activity_list_labels(app);
    PaneHeaderStatus {
        label: &format!("{} visible", labels.len()),
        tone: PaneStatusTone::Idle,
    }
    .render(frame, list_header, &theme());
    let list_region = chrome.region(list_body, labels.len());
    app.activity
        .list_viewport_height
        .set(list_region.content_area.height);
    app.activity.reveal_selection(labels.len());
    let start = app.activity.list_first_visible.get();
    let lines = labels
        .iter()
        .enumerate()
        .skip(start)
        .take(list_region.content_area.height as usize)
        .map(|(index, label)| {
            activity_selection_line(
                label,
                index,
                index == app.activity.selected,
                list_region.content_area.width,
            )
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(lines).style(surface_style()),
        list_region.content_area,
    );
    if let Some(slot) = list_region.scrollbar {
        let mut state = ScrollbarState::new(labels.len())
            .position(app.activity.selected.min(labels.len().saturating_sub(1)))
            .viewport_content_length(list_region.content_area.height as usize);
        render_scrollbar(frame, slot, &mut state, &theme());
        app.geometry
            .add_region(RegionKind::Scrollbar(ScrollbarTarget::OpsList), slot.area);
    }
    chrome.render_body_boundary(frame, list_body, &theme());

    if let Some(details_pane) = details_pane {
        let [details_header, details_body] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(1)])
            .areas(details_pane);
        let details_title = match app.activity.representation {
            ActivityRepresentation::Grouped => "Group details",
            ActivityRepresentation::Entries => "Entry details",
        };
        chrome.render_header(
            frame,
            details_pane,
            details_title,
            if app.activity.focus == ActivityFocus::Details {
                PaneFocus::Active
            } else {
                PaneFocus::Idle
            },
            &theme(),
        );
        let mut detail_lines =
            build_activity_detail_lines(app, chrome.content_area(details_body).width);
        let mut detail_region = chrome.region(details_body, detail_lines.len());
        if detail_region.scrollbar.is_some() {
            detail_lines = build_activity_detail_lines(app, detail_region.content_area.width);
            detail_region = chrome.region(details_body, detail_lines.len());
        }
        app.activity.details_content_height.set(detail_lines.len());
        app.activity
            .details_viewport_height
            .set(detail_region.content_area.height as usize);
        ScrollableDocument {
            lines: &detail_lines,
            offset: app.activity.details_scroll,
        }
        .render(frame, detail_region, &theme());
        if let Some(slot) = detail_region.scrollbar {
            app.geometry.add_region(
                RegionKind::Scrollbar(ScrollbarTarget::OpsDetails),
                slot.area,
            );
        }
        chrome.render_body_boundary(frame, details_body, &theme());
        debug_assert_eq!(
            details_header,
            Rect {
                height: details_pane.height.min(1),
                ..details_pane
            }
        );
    }
    if let Some(divider) = divider {
        chrome.render_dividers(frame, &[divider], &theme());
    }
}

struct ActivityListLabel {
    text: String,
    category: crate::model::agent::oplog::OplogEntryCategory,
    context_only: bool,
}

fn activity_list_labels(app: &TuiApp) -> Vec<ActivityListLabel> {
    match app.activity.representation {
        ActivityRepresentation::Grouped => app
            .activity_groups()
            .into_iter()
            .map(|group| {
                let identity = if app.activity.agents.len() > 1 {
                    fit_middle(&format!("{}/{}", group.component, group.agent_id), 32)
                } else {
                    String::new()
                };
                let anchor = format!("#{:05}", group.first_index);
                let count = if group.entry_count > 1 {
                    format!(" · {}×", group.entry_count)
                } else {
                    String::new()
                };
                let status = group
                    .status
                    .label()
                    .map(|label| format!(" · {label}"))
                    .unwrap_or_default();
                let summary = if group.summary.is_empty() {
                    group.kinds.join(" → ")
                } else {
                    group.summary
                };
                let branch = match group.depth {
                    0 => "",
                    1 => "└─ ",
                    _ => "  └─ ",
                };
                let identity = (!identity.is_empty())
                    .then(|| format!("{identity} · "))
                    .unwrap_or_default();
                ActivityListLabel {
                    text: format!(
                        "{branch}{anchor}  {}  [{:<9}] {identity}{}{}{status}",
                        group.timestamp,
                        group.category.label(),
                        summary,
                        count,
                    ),
                    category: group.category,
                    context_only: group.context_only,
                }
            })
            .collect(),
        ActivityRepresentation::Entries => app
            .activity_filtered_entries()
            .into_iter()
            .map(|entry| {
                let identity = if app.activity.agents.len() > 1 {
                    fit_middle(&format!("{}/{}", entry.component, entry.agent_id), 32)
                } else {
                    String::new()
                };
                let summary = entry.presentation.summary();
                let tags = entry.presentation.tags();
                let suffix = [
                    (!summary.is_empty()).then_some(summary),
                    (!tags.is_empty()).then_some(tags.join(" · ")),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · ");
                let identity = (!identity.is_empty())
                    .then(|| format!("{identity} · "))
                    .unwrap_or_default();
                ActivityListLabel {
                    text: format!(
                        "#{:05}  {}  [{:<9}] {identity}{}{}",
                        entry.presentation.index,
                        entry.presentation.timestamp,
                        entry.presentation.category().label(),
                        entry.presentation.kind,
                        (!suffix.is_empty())
                            .then(|| format!(" · {suffix}"))
                            .unwrap_or_default(),
                    ),
                    category: entry.presentation.category(),
                    context_only: false,
                }
            })
            .collect(),
    }
}

fn fit_middle(value: &str, width: usize) -> String {
    if UnicodeWidthStr::width(value) <= width {
        return value.to_string();
    }
    if width == 0 {
        return String::new();
    }
    if width == 1 {
        return "…".to_string();
    }
    let left_width = (width - 1) / 2;
    let right_width = width - 1 - left_width;
    let mut left = String::new();
    let mut used = 0usize;
    for character in value.chars() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if used.saturating_add(character_width) > left_width {
            break;
        }
        left.push(character);
        used = used.saturating_add(character_width);
    }
    let mut right = Vec::new();
    used = 0;
    for character in value.chars().rev() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if used.saturating_add(character_width) > right_width {
            break;
        }
        right.push(character);
        used = used.saturating_add(character_width);
    }
    right.reverse();
    format!("{left}…{}", right.into_iter().collect::<String>())
}

fn activity_selection_line(
    label: &ActivityListLabel,
    index: usize,
    selected: bool,
    width: u16,
) -> Line<'static> {
    let visual = theme();
    let background = match (selected, index % 2) {
        (true, 0) => visual.table_selected_even_background,
        (true, _) => visual.table_selected_odd_background,
        (false, 0) => visual.table_even_background,
        (false, _) => visual.table_odd_background,
    };
    let marker_width = width.min(2) as usize;
    let label_width = width as usize - marker_width;
    Line::from(vec![
        Span::styled(
            fit(if selected { "▌" } else { "" }, marker_width),
            Style::default()
                .fg(if selected {
                    visual.accent
                } else {
                    visual.text_muted
                })
                .bg(background),
        ),
        Span::styled(
            fit(&label.text, label_width),
            Style::default()
                .fg(if selected {
                    visual.table_selection_text
                } else if label.context_only {
                    visual.text_muted
                } else {
                    match label.category {
                        crate::model::agent::oplog::OplogEntryCategory::Invocation => visual.accent,
                        crate::model::agent::oplog::OplogEntryCategory::HostCall => visual.info,
                        crate::model::agent::oplog::OplogEntryCategory::Log => visual.success,
                        crate::model::agent::oplog::OplogEntryCategory::Failure => visual.error,
                        crate::model::agent::oplog::OplogEntryCategory::Trace => visual.marker,
                        crate::model::agent::oplog::OplogEntryCategory::State => visual.text,
                        crate::model::agent::oplog::OplogEntryCategory::Lifecycle => {
                            visual.accent_hover
                        }
                        crate::model::agent::oplog::OplogEntryCategory::Other => {
                            visual.text_secondary
                        }
                    }
                })
                .bg(background),
        ),
    ])
}

fn build_activity_detail_lines(app: &TuiApp, width: u16) -> Vec<Line<'static>> {
    match app.activity.representation {
        ActivityRepresentation::Grouped => {
            let groups = app.activity_groups();
            let Some(group) = groups.get(app.activity.selected) else {
                return activity_empty_detail_lines();
            };
            let entries = app
                .activity
                .entries
                .iter()
                .filter(|entry| {
                    entry.component == group.component
                        && entry.agent_id == group.agent_id
                        && group.entry_indices.contains(&entry.presentation.index)
                })
                .collect::<Vec<_>>();
            let mut lines = vec![
                SectionHeading {
                    title: "Journal group",
                    detail: Some(&format!("{} exact entries", entries.len())),
                }
                .line(&theme()),
                Line::default(),
            ];
            push_activity_identity_lines(&mut lines, &group.component, &group.agent_id, width);
            let anchor = match &group.group {
                Some(OplogEntryGroupKey::OplogIndex(index)) => format!("oplog #{index}"),
                Some(OplogEntryGroupKey::Span(span)) => format!("span {span}"),
                None => format!("entry #{}", group.first_index),
            };
            for (label, value) in [
                ("Anchor", anchor),
                ("Timestamp", group.timestamp.clone()),
                ("Category", group.category.label().to_string()),
                ("Kinds", group.kinds.join(" → ")),
                ("Summary", group.summary.clone()),
            ] {
                lines.push(
                    FieldRow {
                        label,
                        value: &value,
                        label_width: 10,
                    }
                    .line(&theme()),
                );
            }
            for entry in entries {
                push_activity_entry_lines(&mut lines, entry, width);
            }
            lines
        }
        ActivityRepresentation::Entries => {
            let visible = app.activity_filtered_entries();
            let Some(entry) = visible.get(app.activity.selected) else {
                return activity_empty_detail_lines();
            };
            let mut lines = vec![
                SectionHeading {
                    title: "Exact oplog entry",
                    detail: Some(&format!("#{}", entry.presentation.index)),
                }
                .line(&theme()),
                Line::default(),
            ];
            push_activity_identity_lines(&mut lines, &entry.component, &entry.agent_id, width);
            lines.push(
                FieldRow {
                    label: "Category",
                    value: entry.presentation.category().label(),
                    label_width: 10,
                }
                .line(&theme()),
            );
            let summary = entry.presentation.summary();
            if !summary.is_empty() {
                lines.push(
                    FieldRow {
                        label: "Summary",
                        value: &summary,
                        label_width: 10,
                    }
                    .line(&theme()),
                );
            }
            let tags = entry.presentation.tags().join(" · ");
            if !tags.is_empty() {
                lines.push(
                    FieldRow {
                        label: "Tags",
                        value: &tags,
                        label_width: 10,
                    }
                    .line(&theme()),
                );
            }
            lines.push(
                FieldRow {
                    label: "Kind",
                    value: &entry.presentation.kind,
                    label_width: 10,
                }
                .line(&theme()),
            );
            lines.push(
                FieldRow {
                    label: "Timestamp",
                    value: &entry.presentation.timestamp.to_string(),
                    label_width: 10,
                }
                .line(&theme()),
            );
            if let Some(attribution) = &entry.presentation.attribution {
                lines.push(Line::default());
                lines.push(
                    SectionHeading {
                        title: "Attribution",
                        detail: None,
                    }
                    .line(&theme()),
                );
                let attribution = serde_json::to_value(attribution).unwrap_or(Value::Null);
                lines.extend(
                    JsonDocument {
                        value: &attribution,
                    }
                    .lines(width, &theme()),
                );
            }
            lines.push(Line::default());
            lines.push(
                SectionHeading {
                    title: "Payload",
                    detail: None,
                }
                .line(&theme()),
            );
            lines.extend(
                JsonDocument {
                    value: &entry.presentation.detail,
                }
                .lines(width, &theme()),
            );
            lines
        }
    }
}

fn activity_empty_detail_lines() -> Vec<Line<'static>> {
    vec![
        Notice {
            kind: NoticeKind::Empty,
            message: "Select an oplog row to inspect it",
        }
        .line(&theme()),
    ]
}

fn push_activity_identity_lines(
    lines: &mut Vec<Line<'static>>,
    component: &str,
    agent_id: &str,
    width: u16,
) {
    lines.push(
        FieldRow {
            label: "Component",
            value: component,
            label_width: 10,
        }
        .line(&theme()),
    );
    let value_width = width.saturating_sub(12) as usize;
    let formatted = format_agent_id_for_terminal(agent_id, false, Some(value_width.max(1)));
    for (index, line) in formatted.lines().enumerate() {
        let mut spans = vec![
            Span::styled(
                if index == 0 {
                    fit("AgentID", 10)
                } else {
                    " ".repeat(10)
                },
                Style::default().fg(theme().text_muted),
            ),
            Span::raw("  "),
        ];
        spans.extend(agent_id_line_spans(line));
        lines.push(Line::from(spans));
    }
}

fn push_activity_entry_lines(
    lines: &mut Vec<Line<'static>>,
    entry: &activity::ActivityEntry,
    width: u16,
) {
    lines.push(Line::default());
    lines.push(
        SectionHeading {
            title: &format!("#{} {}", entry.presentation.index, entry.presentation.kind),
            detail: Some(&entry.presentation.timestamp.to_string()),
        }
        .line(&theme()),
    );
    let summary = entry.presentation.summary();
    if !summary.is_empty() {
        lines.push(
            FieldRow {
                label: "Summary",
                value: &summary,
                label_width: 10,
            }
            .line(&theme()),
        );
    }
    let tags = entry.presentation.tags().join(" · ");
    if !tags.is_empty() {
        lines.push(
            FieldRow {
                label: "Tags",
                value: &tags,
                label_width: 10,
            }
            .line(&theme()),
        );
    }
    if let Some(attribution) = &entry.presentation.attribution {
        lines.push(
            FieldRow {
                label: "Owner",
                value: match attribution {
                    golem_common::model::oplog::PublicOplogEntryAttribution::Agent(_) => "agent",
                    golem_common::model::oplog::PublicOplogEntryAttribution::Entity(_) => "entity",
                },
                label_width: 10,
            }
            .line(&theme()),
        );
    }
    lines.extend(
        JsonDocument {
            value: &entry.presentation.detail,
        }
        .lines(width, &theme()),
    );
}

fn render_ops_metrics(frame: &mut Frame<'_>, area: Rect, app: &TuiApp) {
    let chrome = PaneChrome { area };
    chrome.render_header(frame, area, "Metrics", PaneFocus::Active, &theme());
    if app.metrics.loading {
        PaneHeaderStatus {
            label: "refreshing",
            tone: PaneStatusTone::Loading,
        }
        .render(frame, area, &theme());
    }
    let exact_agent = app.related_exact_agent().is_some();
    let related_links = [
        (OpsDestination::Agents, "Agent", exact_agent),
        (
            OpsDestination::Oplog,
            "Oplog",
            app.metric_sample_oplog_target().is_ok(),
        ),
        (
            OpsDestination::Stream,
            "Stream",
            app.related_stream_available(),
        ),
    ];
    render_ops_related_links(
        frame,
        area,
        app,
        &related_links,
        14,
        u16::from(app.metrics.loading).saturating_mul(16),
    );
    let outer_body = chrome.body_area(area);
    let [controls_row, body] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .areas(outer_body);
    let controls = chrome.content_area(controls_row);
    let mode_choices = [
        ControlChoice {
            label: "Dashboard",
            selected: app.metrics.mode == MetricsMode::Dashboard,
        },
        ControlChoice {
            label: "Explore",
            selected: app.metrics.mode == MetricsMode::Explore,
        },
    ];
    let source_choices = [
        ControlChoice {
            label: "Agents",
            selected: app.metrics.source
                == crate::model::local_observability::LocalMetricSource::Agents,
        },
        ControlChoice {
            label: "Server",
            selected: app.metrics.source
                == crate::model::local_observability::LocalMetricSource::Services,
        },
    ];
    let state_choices = [
        ControlChoice {
            label: "Live",
            selected: app.metrics.live,
        },
        ControlChoice {
            label: "Paused",
            selected: !app.metrics.live,
        },
    ];
    let groups = [
        ControlGroup {
            label: "View",
            choices: &mode_choices,
        },
        ControlGroup {
            label: "Source",
            choices: &source_choices,
        },
        ControlGroup {
            label: "State",
            choices: &state_choices,
        },
    ];
    let freshness = app
        .metrics
        .dashboard
        .as_ref()
        .and_then(|dashboard| dashboard.freshest_sample_unix_nanos)
        .map(metric_freshness_label)
        .map(|freshness| format!("Freshness {freshness}"))
        .unwrap_or_default();
    let mut status = format!(
        "Scope {}   Time {}",
        app.metrics.scope_label(),
        app.metrics.range.label()
    );
    if !freshness.is_empty() {
        status.push_str("   ");
        status.push_str(&freshness);
    }
    render_ops_control_strip(frame, controls, app, &groups, Some(&status));

    if let Some(error) = &app.metrics.error {
        render_metrics_notice(frame, body, NoticeKind::Warning, error);
        chrome.render_body_boundary(frame, outer_body, &theme());
        return;
    }
    if let Some(capability) = &app.metrics.capability
        && capability.status != crate::model::local_observability::LocalObservabilityStatus::Ready
    {
        render_metrics_notice(
            frame,
            body,
            NoticeKind::Info,
            capability
                .message
                .as_deref()
                .unwrap_or("Local metrics collection is unavailable"),
        );
        chrome.render_body_boundary(frame, outer_body, &theme());
        return;
    }
    if app.metrics.families.is_empty() && app.metrics.mode == MetricsMode::Explore {
        let message = if app.metrics.loading {
            "Loading metric families…"
        } else {
            "No samples in the last 15 minutes. New local deployments are attached automatically; existing deployments begin exporting after their next deploy."
        };
        render_metrics_notice(frame, body, NoticeKind::Info, message);
        chrome.render_body_boundary(frame, outer_body, &theme());
        return;
    }

    if app.metrics.mode == MetricsMode::Dashboard {
        if app.metrics.dashboard.is_none() {
            render_metrics_notice(
                frame,
                body,
                NoticeKind::Info,
                if app.metrics.loading {
                    "Loading dashboard snapshot…"
                } else {
                    "No dashboard snapshot is available"
                },
            );
        } else {
            render_metrics_dashboard(frame, body, app);
        }
        chrome.render_body_boundary(frame, outer_body, &theme());
        return;
    }

    let pane_layout = adaptive_ops_pane_layout(body, 100, &[31, 31, 38]);
    render_metrics_families(frame, pane_layout.panes[0], chrome, app);
    render_metrics_series(frame, pane_layout.panes[1], chrome, app);
    render_metrics_chart(frame, pane_layout.panes[2], chrome, app);
    chrome.render_dividers(frame, &pane_layout.dividers, &theme());
    chrome.render_body_boundary(frame, outer_body, &theme());
}

fn render_ops_metric_panel_route(frame: &mut Frame<'_>, area: Rect, app: &TuiApp, panel_id: &str) {
    let chrome = PaneChrome { area };
    let panel = app
        .metrics
        .dashboard
        .as_ref()
        .and_then(|dashboard| dashboard.panels.iter().find(|panel| panel.id == panel_id));
    let title = panel
        .map(|panel| format!("Metrics / {}", panel.title))
        .unwrap_or_else(|| "Metrics / unavailable panel".to_string());
    chrome.render_header(frame, area, &title, PaneFocus::Active, &theme());
    let outer_body = chrome.body_area(area);
    let body = chrome.content_area(outer_body);
    let Some(panel) = panel else {
        render_metrics_notice(
            frame,
            body,
            NoticeKind::Warning,
            "This dashboard snapshot no longer contains the opened panel",
        );
        chrome.render_body_boundary(frame, outer_body, &theme());
        return;
    };
    if panel.series.is_empty() {
        render_metrics_notice(
            frame,
            body,
            NoticeKind::Empty,
            panel
                .empty_reason
                .as_deref()
                .unwrap_or("No samples in this range"),
        );
        chrome.render_body_boundary(frame, outer_body, &theme());
        return;
    }

    let mut timestamps = panel
        .series
        .iter()
        .flat_map(|series| series.samples.iter())
        .map(|sample| sample.timestamp_unix_nanos)
        .collect::<Vec<_>>();
    timestamps.sort_unstable();
    timestamps.dedup();
    let cursor_timestamp = metrics_cursor_timestamp(app, &timestamps);
    let value_min = panel
        .series
        .iter()
        .flat_map(|series| series.samples.iter())
        .map(|sample| sample.value)
        .fold(f64::INFINITY, f64::min);
    let value_max = panel
        .series
        .iter()
        .flat_map(|series| series.samples.iter())
        .map(|sample| sample.value)
        .fold(f64::NEG_INFINITY, f64::max);
    let (value_min, value_max) = if value_min.is_finite() && value_max.is_finite() {
        (value_min, value_max)
    } else {
        (0.0, 0.0)
    };
    let dashboard_range = app
        .metrics
        .dashboard
        .as_ref()
        .map(|dashboard| dashboard.range);
    let time_from = dashboard_range
        .map(|range| range.from_unix_nanos)
        .or(panel.observed_from_unix_nanos)
        .or_else(|| timestamps.first().copied())
        .unwrap_or(0);
    let time_to = dashboard_range
        .map(|range| range.to_unix_nanos)
        .or(panel.observed_to_unix_nanos)
        .or_else(|| timestamps.last().copied())
        .unwrap_or(time_from);
    let chart_region = metric_chart_region(body, 22, 1);
    app.geometry
        .add_region(RegionKind::MetricsChart, chart_region);
    let unit = panel
        .series
        .first()
        .map(|series| series.unit.as_str())
        .filter(|unit| *unit != "1")
        .unwrap_or("");
    let mut lines = vec![
        Line::from(Span::styled(
            format!(
                "y {}{unit} … {}{unit} · x {} → {} · gaps · shared scale{}",
                format_metric_value(value_min),
                format_metric_value(value_max),
                metric_timestamp_label(time_from),
                metric_timestamp_label(time_to),
                if panel.approximate {
                    " · ≈ bounded"
                } else {
                    " · exact"
                },
            ),
            Style::default().fg(theme().text_muted),
        )),
        Line::default(),
    ];
    let colors = [
        theme().accent,
        theme().info,
        theme().success,
        theme().marker,
        theme().accent_hover,
        theme().text_secondary,
    ];
    for (index, series) in panel.series.iter().enumerate() {
        let chart = metric_sparkline_with_bounds(
            &series.samples,
            chart_region.width.max(1) as usize,
            time_from,
            time_to,
            value_min,
            value_max,
            cursor_timestamp,
        );
        let latest = series
            .latest_value
            .map(format_metric_value)
            .unwrap_or_else(|| "—".to_string());
        lines.push(Line::from(vec![
            Span::styled(
                format!("{} ", fit(&series.label, 20)),
                Style::default().fg(colors[index % colors.len()]),
            ),
            Span::styled(chart, Style::default().fg(colors[index % colors.len()])),
            Span::styled(
                format!("  {latest}{unit}"),
                Style::default().fg(theme().text_secondary),
            ),
        ]));
    }
    if let Some(timestamp) = cursor_timestamp {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            format!("cursor {}", metric_timestamp_label(timestamp)),
            Style::default()
                .fg(theme().accent_hover)
                .add_modifier(Modifier::BOLD),
        )));
        for series in &panel.series {
            let sample = series
                .samples
                .iter()
                .find(|sample| sample.timestamp_unix_nanos == timestamp);
            let series_unit = if series.unit == "1" {
                ""
            } else {
                series.unit.as_str()
            };
            lines.push(metric_cursor_value_line(&series.label, series_unit, sample));
        }
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "Shift+←/→ or click chart to inspect · Esc back",
        Style::default().fg(theme().text_muted),
    )));
    frame.render_widget(
        Paragraph::new(lines)
            .style(surface_style())
            .wrap(Wrap { trim: false }),
        body,
    );
    chrome.render_body_boundary(frame, outer_body, &theme());
}

fn render_metrics_dashboard(frame: &mut Frame<'_>, area: Rect, app: &TuiApp) {
    let Some(dashboard) = &app.metrics.dashboard else {
        return;
    };
    let stat_height = if area.height >= 8 { 3 } else { 1 };
    let [stats_area, panels_area] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(stat_height), Constraint::Min(1)])
        .areas(area);
    render_metric_stats(frame, stats_area, &dashboard.stats);

    if panels_area.height == 0 || dashboard.panels.is_empty() {
        return;
    }
    let columns = if panels_area.width >= 96 { 2 } else { 1 };
    const MIN_PANEL_HEIGHT: usize = 6;
    let panel_count = dashboard.panels.len();
    let total_rows = panel_count.div_ceil(columns);
    let visible_rows = ((panels_area.height as usize) / MIN_PANEL_HEIGHT)
        .max(1)
        .min(total_rows);
    let selected = app.metrics.selected_panel.min(panel_count - 1);
    let start_row = selection_window_start(
        selected / columns,
        total_rows,
        visible_rows as u16,
        app.metrics.dashboard_first_visible.get() / columns,
    );
    let start = start_row.saturating_mul(columns);
    let end = start
        .saturating_add(visible_rows.saturating_mul(columns))
        .min(panel_count);
    app.metrics.dashboard_first_visible.set(start);
    app.metrics.dashboard_visible_count.set(end - start);
    app.metrics.dashboard_columns.set(columns);
    let overflowing = visible_rows < total_rows;
    let grid_area = if overflowing {
        Rect {
            width: panels_area.width.saturating_sub(1),
            ..panels_area
        }
    } else {
        panels_area
    };
    app.geometry
        .add_region(RegionKind::MetricsDashboard, grid_area);
    let row_constraints = (0..visible_rows)
        .map(|_| Constraint::Ratio(1, visible_rows as u32))
        .collect::<Vec<_>>();
    let row_areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints(row_constraints)
        .split(grid_area);
    for (visible, panel_index) in (start..end).enumerate() {
        let row = visible / columns;
        let column = visible % columns;
        let column_areas = Layout::default()
            .direction(Direction::Horizontal)
            .constraints((0..columns).map(|_| Constraint::Ratio(1, columns as u32)))
            .split(row_areas[row]);
        app.geometry
            .add_region(RegionKind::MetricsPanel(panel_index), column_areas[column]);
        render_metric_dashboard_panel(
            frame,
            column_areas[column],
            app,
            panel_index,
            &dashboard.panels[panel_index],
        );
    }
    if overflowing {
        let slot = PaneScrollbarSlot {
            area: Rect::new(
                panels_area.right().saturating_sub(1),
                panels_area.y,
                1,
                panels_area.height,
            ),
        };
        let mut scrollbar = ScrollbarState::new(total_rows)
            .position(start_row)
            .viewport_content_length(visible_rows);
        render_scrollbar(frame, slot, &mut scrollbar, &theme());
        app.geometry.add_region(
            RegionKind::Scrollbar(ScrollbarTarget::MetricsDashboard),
            slot.area,
        );
    }
}

fn render_metric_stats(
    frame: &mut Frame<'_>,
    area: Rect,
    stats: &[crate::model::local_observability::LocalMetricDashboardStat],
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    if stats.is_empty() {
        return;
    }
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints((0..stats.len()).map(|_| Constraint::Ratio(1, stats.len() as u32)))
        .split(area);
    for (index, stat) in stats.iter().enumerate() {
        let value = stat
            .value
            .map(|value| {
                let prefix = if stat.approximate { "≈" } else { "" };
                let suffix = if stat.unit == "1" {
                    String::new()
                } else {
                    format!(" {}", stat.unit)
                };
                format!("{prefix}{}{suffix}", format_metric_value(value))
            })
            .unwrap_or_else(|| "—".to_string());
        let text = if area.height >= 3 {
            format!("{}\n{value}", stat.label)
        } else {
            format!("{}: {value}", stat.label)
        };
        frame.render_widget(
            Paragraph::new(text)
                .style(Style::default().fg(theme().text).bg(theme().panel_strong))
                .alignment(Alignment::Center),
            columns[index],
        );
    }
}

fn render_metric_dashboard_panel(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &TuiApp,
    panel_index: usize,
    panel: &crate::model::local_observability::LocalMetricDashboardPanel,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let selected = panel_index == app.metrics.selected_panel;
    let border = if selected {
        theme().accent
    } else {
        theme().border_subtle
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border).bg(theme().surface))
        .title(Span::styled(
            format!(" {} ", panel.title),
            Style::default()
                .fg(if selected {
                    theme().accent_hover
                } else {
                    theme().text_secondary
                })
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let mut lines = Vec::new();
    if panel.series.is_empty() {
        lines.push(Line::from(Span::styled(
            panel
                .empty_reason
                .as_deref()
                .unwrap_or("No samples in this range"),
            Style::default().fg(theme().text_muted),
        )));
    } else {
        for series in panel.series.iter().take(inner.height.max(1) as usize) {
            if !series.samples.is_empty() && inner.width >= 24 {
                let sparkline_width = (inner.width as usize / 3).clamp(8, 20);
                let value_min = series
                    .samples
                    .iter()
                    .map(|sample| sample.value)
                    .fold(f64::INFINITY, f64::min);
                let value_max = series
                    .samples
                    .iter()
                    .map(|sample| sample.value)
                    .fold(f64::NEG_INFINITY, f64::max);
                let time_range = app
                    .metrics
                    .dashboard
                    .as_ref()
                    .map(|dashboard| dashboard.range);
                let time_from = time_range
                    .map(|range| range.from_unix_nanos)
                    .unwrap_or(series.samples[0].timestamp_unix_nanos);
                let time_to = time_range
                    .map(|range| range.to_unix_nanos)
                    .unwrap_or_else(|| {
                        series
                            .samples
                            .last()
                            .map(|sample| sample.timestamp_unix_nanos)
                            .unwrap_or(time_from)
                    });
                let chart = metric_sparkline_with_bounds(
                    &series.samples,
                    sparkline_width,
                    time_from,
                    time_to,
                    value_min,
                    value_max,
                    None,
                );
                let value = series
                    .latest_value
                    .map(format_metric_value)
                    .unwrap_or_else(|| "—".to_string());
                let approximation = if series.approximate { "≈" } else { "" };
                let unit = if series.unit == "1" {
                    String::new()
                } else {
                    format!(" {}", series.unit)
                };
                lines.push(Line::from(vec![
                    Span::styled(format!("{chart} "), Style::default().fg(theme().accent)),
                    Span::styled(
                        fit(
                            &format!("{} {approximation}{value}{unit}", series.label),
                            inner.width.saturating_sub(sparkline_width as u16 + 1) as usize,
                        ),
                        Style::default().fg(theme().text_secondary),
                    ),
                ]));
            } else {
                let value = series
                    .latest_value
                    .map(format_metric_value)
                    .unwrap_or_else(|| "—".to_string());
                lines.push(Line::from(fit(
                    &format!("{}  {value} {}", series.label, series.unit),
                    inner.width as usize,
                )));
            }
        }
        if panel.approximate {
            lines.push(Line::from(Span::styled(
                "≈ bounded/downsampled",
                Style::default().fg(theme().text_muted),
            )));
        }
    }
    frame.render_widget(
        Paragraph::new(lines)
            .style(surface_style())
            .wrap(Wrap { trim: true }),
        inner,
    );
}

fn metric_freshness_label(timestamp_unix_nanos: i64) -> String {
    let now = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .min(i64::MAX as u128) as i64;
    let age_seconds = now.saturating_sub(timestamp_unix_nanos) / 1_000_000_000;
    format!("fresh {age_seconds}s")
}

fn render_metrics_notice(frame: &mut Frame<'_>, area: Rect, kind: NoticeKind, message: &str) {
    frame.render_widget(
        Paragraph::new(Notice { kind, message }.line(&theme()))
            .style(surface_style())
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn render_metrics_families(frame: &mut Frame<'_>, area: Rect, chrome: PaneChrome, app: &TuiApp) {
    chrome.render_header(
        frame,
        area,
        "Families",
        if app.metrics.focus == MetricsFocus::Families {
            PaneFocus::Active
        } else {
            PaneFocus::Idle
        },
        &theme(),
    );
    let status = format!(
        "{} loaded{}",
        app.metrics.families.len(),
        if app.metrics.families_next_cursor.is_some() {
            " / more"
        } else {
            ""
        }
    );
    PaneHeaderStatus {
        label: &status,
        tone: if app.metrics.families_next_cursor.is_some() {
            PaneStatusTone::Active
        } else {
            PaneStatusTone::Idle
        },
    }
    .render(frame, area, &theme());
    let outer_body = chrome.body_area(area);
    let body = chrome
        .region(outer_body, app.metrics.families.len())
        .content_area;
    let lines = app
        .metrics
        .families
        .iter()
        .enumerate()
        .map(|(index, family)| {
            metric_selection_line(
                format!(
                    "{}  {} · {} series",
                    family.name, family.category, family.series_count
                ),
                index == app.metrics.selected_family,
                body.width,
            )
        })
        .collect::<Vec<_>>();
    render_ops_selection_list(
        frame,
        outer_body,
        chrome,
        app,
        &lines,
        app.metrics.selected_family,
        &app.metrics.families_first_visible,
        &app.metrics.families_viewport_height,
        RegionKind::MetricsFamilies,
        ScrollbarTarget::MetricsFamilies,
    );
    chrome.render_body_boundary(frame, outer_body, &theme());
}

fn render_metrics_series(frame: &mut Frame<'_>, area: Rect, chrome: PaneChrome, app: &TuiApp) {
    chrome.render_header(
        frame,
        area,
        "Series",
        if app.metrics.focus == MetricsFocus::Series {
            PaneFocus::Active
        } else {
            PaneFocus::Idle
        },
        &theme(),
    );
    let status = format!(
        "{} loaded / {}/{} shown{}",
        app.metrics.series.len(),
        app.metrics.selected_series_keys.len(),
        MAX_EXPLORE_SERIES,
        if app.metrics.series_next_cursor.is_some() {
            " / more"
        } else {
            ""
        }
    );
    PaneHeaderStatus {
        label: &status,
        tone: if app.metrics.series_next_cursor.is_some() {
            PaneStatusTone::Active
        } else {
            PaneStatusTone::Idle
        },
    }
    .render(frame, area, &theme());
    let outer_body = chrome.body_area(area);
    let body = chrome
        .region(outer_body, app.metrics.series.len())
        .content_area;
    let lines = app
        .metrics
        .series
        .iter()
        .enumerate()
        .map(|(index, series)| {
            let identity = series
                .agent_id
                .as_deref()
                .or(series.agent_type.as_deref())
                .unwrap_or("service");
            let identity = if series.statistic == "value" {
                identity.to_string()
            } else {
                format!("{identity}/{}", series.statistic)
            };
            let value = series
                .latest_value
                .map(format_metric_value)
                .unwrap_or_else(|| "—".to_string());
            let shown = if app
                .metrics
                .selected_series_keys
                .iter()
                .any(|selected| selected == &series.series_key)
            {
                "[x]"
            } else {
                "[ ]"
            };
            metric_selection_line(
                format!("{shown} {identity}  {value}"),
                index == app.metrics.selected_series,
                body.width,
            )
        })
        .collect::<Vec<_>>();
    render_ops_selection_list(
        frame,
        outer_body,
        chrome,
        app,
        &lines,
        app.metrics.selected_series,
        &app.metrics.series_first_visible,
        &app.metrics.series_viewport_height,
        RegionKind::MetricsSeries,
        ScrollbarTarget::MetricsSeries,
    );
    if let Some(region) = app.geometry.region(RegionKind::MetricsSeries) {
        let start = app.metrics.series_first_visible.get();
        for (line, index) in (start..app.metrics.series.len())
            .take(region.height as usize)
            .enumerate()
        {
            if region.width >= 4 {
                app.geometry.add_region(
                    RegionKind::MetricsSeriesToggle(index),
                    Rect::new(
                        region.x.saturating_add(1),
                        region.y.saturating_add(line as u16),
                        3,
                        1,
                    ),
                );
            }
        }
    }
    chrome.render_body_boundary(frame, outer_body, &theme());
}

fn render_metrics_chart(frame: &mut Frame<'_>, area: Rect, chrome: PaneChrome, app: &TuiApp) {
    chrome.render_header(
        frame,
        area,
        "Chart / Details",
        if app.metrics.focus == MetricsFocus::Chart {
            PaneFocus::Active
        } else {
            PaneFocus::Idle
        },
        &theme(),
    );
    let outer_body = chrome.body_area(area);
    let body = chrome.content_area(outer_body);
    let mut lines = Vec::new();
    if let Some(notice) = &app.metrics.notice {
        lines.push(
            Notice {
                kind: NoticeKind::Info,
                message: notice,
            }
            .line(&theme()),
        );
    }
    if let Some(family) = app.metrics.families.get(app.metrics.selected_family) {
        lines.push(Line::from(Span::styled(
            fit(&family.name, body.width as usize),
            Style::default()
                .fg(theme().text)
                .add_modifier(Modifier::BOLD),
        )));
        if !family.description.is_empty() {
            lines.extend(wrap_styled_line(
                Line::from(Span::styled(
                    family.description.clone(),
                    Style::default().fg(theme().text_secondary),
                )),
                body.width as usize,
            ));
        }
    }
    if !app.metrics.series_samples.is_empty() {
        let chart_region = metric_chart_region(body, 19, 3);
        app.geometry
            .add_region(RegionKind::MetricsChart, chart_region);
        lines.push(Line::default());
        let value_min = app
            .metrics
            .series_samples
            .iter()
            .flat_map(|samples| samples.response.samples.iter())
            .map(|sample| sample.value)
            .fold(f64::INFINITY, f64::min);
        let value_max = app
            .metrics
            .series_samples
            .iter()
            .flat_map(|samples| samples.response.samples.iter())
            .map(|sample| sample.value)
            .fold(f64::NEG_INFINITY, f64::max);
        let (value_min, value_max) = if value_min.is_finite() && value_max.is_finite() {
            (value_min, value_max)
        } else {
            (0.0, 0.0)
        };
        let time_from = app
            .metrics
            .time_range
            .map(|range| range.from_unix_nanos)
            .or_else(|| {
                app.metrics
                    .series_samples
                    .iter()
                    .filter_map(|samples| samples.response.observed_from_unix_nanos)
                    .min()
            })
            .unwrap_or(0);
        let time_to = app
            .metrics
            .time_range
            .map(|range| range.to_unix_nanos)
            .or_else(|| {
                app.metrics
                    .series_samples
                    .iter()
                    .filter_map(|samples| samples.response.observed_to_unix_nanos)
                    .max()
            })
            .unwrap_or(time_from);
        let unit = app
            .metrics
            .families
            .get(app.metrics.selected_family)
            .map(|family| family.unit.as_str())
            .unwrap_or("1");
        let unit = if unit == "1" { "" } else { unit };
        let timestamps = app.metrics_chart_timestamps();
        let cursor_timestamp = metrics_cursor_timestamp(app, &timestamps);
        let x_axis = if app.metrics.range.is_custom() {
            app.metrics.range.label()
        } else {
            format!("-{} → now", app.metrics.range.label())
        };
        lines.push(Line::from(Span::styled(
            format!(
                "y {}{unit} … {}{unit} · x {x_axis} · gaps · shared scale",
                format_metric_value(value_min),
                format_metric_value(value_max),
            ),
            Style::default().fg(theme().text_muted),
        )));
        let colors = [
            theme().accent,
            theme().info,
            theme().success,
            theme().marker,
            theme().accent_hover,
            theme().text_secondary,
        ];
        for (index, samples) in app.metrics.series_samples.iter().enumerate() {
            let label = app
                .metrics
                .series
                .iter()
                .find(|series| series.series_key == samples.series_key)
                .map(metric_series_short_label)
                .unwrap_or_else(|| samples.series_key.clone());
            let chart_width = chart_region.width.max(1) as usize;
            let chart = metric_sparkline_with_bounds(
                &samples.response.samples,
                chart_width,
                time_from,
                time_to,
                value_min,
                value_max,
                cursor_timestamp,
            );
            let approximation = if samples.response.approximate {
                "≈"
            } else {
                ""
            };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{} ", fit(&label, 18)),
                    Style::default().fg(colors[index % colors.len()]),
                ),
                Span::styled(chart, Style::default().fg(colors[index % colors.len()])),
                Span::styled(
                    format!(" {approximation}"),
                    Style::default().fg(theme().text_muted),
                ),
            ]));
        }
        let total_points = app
            .metrics
            .series_samples
            .iter()
            .map(|samples| samples.response.samples.len())
            .sum::<usize>();
        lines.push(Line::from(format!(
            "{} series · {total_points} visible points · refresh 5s",
            app.metrics.series_samples.len()
        )));
        if let Some(timestamp) = cursor_timestamp {
            lines.push(Line::from(Span::styled(
                format!("cursor {}", metric_timestamp_label(timestamp)),
                Style::default()
                    .fg(theme().accent_hover)
                    .add_modifier(Modifier::BOLD),
            )));
            for samples in &app.metrics.series_samples {
                let label = app
                    .metrics
                    .series
                    .iter()
                    .find(|series| series.series_key == samples.series_key)
                    .map(metric_series_short_label)
                    .unwrap_or_else(|| samples.series_key.clone());
                let sample = samples
                    .response
                    .samples
                    .iter()
                    .find(|sample| sample.timestamp_unix_nanos == timestamp);
                lines.push(metric_cursor_value_line(&label, unit, sample));
            }
        }
    }
    if let Some(series) = app.metrics.series.get(app.metrics.selected_series) {
        lines.push(Line::default());
        for (key, value) in &series.attributes {
            lines.push(Line::from(format!("{key}: {value}")));
        }
    }
    frame.render_widget(
        Paragraph::new(lines)
            .style(surface_style())
            .wrap(Wrap { trim: false }),
        body,
    );
    chrome.render_body_boundary(frame, outer_body, &theme());
}

fn metric_chart_region(area: Rect, leading: u16, trailing: u16) -> Rect {
    if area.width <= leading.saturating_add(trailing) {
        return area;
    }
    Rect {
        x: area.x.saturating_add(leading),
        width: area.width.saturating_sub(leading).saturating_sub(trailing),
        ..area
    }
}

fn metrics_cursor_timestamp(app: &TuiApp, timestamps: &[i64]) -> Option<i64> {
    if timestamps.is_empty() {
        return None;
    }
    Some(
        timestamps[app
            .metrics
            .chart_cursor
            .unwrap_or(timestamps.len() - 1)
            .min(timestamps.len() - 1)],
    )
}

fn metric_bucket_range(
    query_range: crate::model::local_observability::LocalMetricTimeRange,
    bucket_width_unix_nanos: i64,
    sample_timestamp_unix_nanos: i64,
) -> Option<crate::model::local_observability::LocalMetricTimeRange> {
    if bucket_width_unix_nanos <= 0
        || sample_timestamp_unix_nanos < query_range.from_unix_nanos
        || sample_timestamp_unix_nanos >= query_range.to_unix_nanos
    {
        return None;
    }
    let offset = sample_timestamp_unix_nanos.saturating_sub(query_range.from_unix_nanos);
    let bucket_offset = offset
        .div_euclid(bucket_width_unix_nanos)
        .saturating_mul(bucket_width_unix_nanos);
    let from_unix_nanos = query_range.from_unix_nanos.saturating_add(bucket_offset);
    let to_unix_nanos = from_unix_nanos
        .saturating_add(bucket_width_unix_nanos)
        .min(query_range.to_unix_nanos);
    (from_unix_nanos < to_unix_nanos).then_some(
        crate::model::local_observability::LocalMetricTimeRange {
            from_unix_nanos,
            to_unix_nanos,
        },
    )
}

fn metric_timestamp_label(timestamp_unix_nanos: i64) -> String {
    let seconds = timestamp_unix_nanos.div_euclid(1_000_000_000);
    let nanos = timestamp_unix_nanos.rem_euclid(1_000_000_000) as u32;
    chrono::DateTime::<chrono::Utc>::from_timestamp(seconds, nanos)
        .map(|timestamp| timestamp.format("%Y-%m-%d %H:%M:%S%.3fZ").to_string())
        .unwrap_or_else(|| format!("{timestamp_unix_nanos} ns"))
}

fn format_metrics_range_input(timestamp_unix_nanos: i64) -> String {
    datetime_from_unix_nanos(timestamp_unix_nanos)
        .map(|timestamp| timestamp.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .unwrap_or_else(|| timestamp_unix_nanos.to_string())
}

fn parse_metrics_range(
    from: &str,
    to: &str,
) -> Result<crate::model::local_observability::LocalMetricTimeRange, String> {
    let parse = |label: &str, value: &str| {
        chrono::DateTime::parse_from_rfc3339(value.trim())
            .map_err(|_| format!("{label} must be an RFC3339 timestamp with a timezone"))?
            .timestamp_nanos_opt()
            .ok_or_else(|| format!("{label} is outside the supported timestamp range"))
    };
    let from_unix_nanos = parse("From", from)?;
    let to_unix_nanos = parse("To", to)?;
    if from_unix_nanos >= to_unix_nanos {
        return Err("From must be earlier than To".to_string());
    }
    Ok(crate::model::local_observability::LocalMetricTimeRange {
        from_unix_nanos,
        to_unix_nanos,
    })
}

fn datetime_from_unix_nanos(timestamp_unix_nanos: i64) -> Option<chrono::DateTime<chrono::Utc>> {
    let seconds = timestamp_unix_nanos.div_euclid(1_000_000_000);
    let nanos = timestamp_unix_nanos.rem_euclid(1_000_000_000) as u32;
    chrono::DateTime::<chrono::Utc>::from_timestamp(seconds, nanos)
}

fn metric_cursor_value_line(
    label: &str,
    unit: &str,
    sample: Option<&crate::model::local_observability::LocalMetricSample>,
) -> Line<'static> {
    let value = sample
        .map(|sample| format_metric_value(sample.value))
        .unwrap_or_else(|| "—".to_string());
    let anchor = sample
        .and_then(|sample| sample.source_oplog_index)
        .map(|index| format!(" · oplog #{index}"))
        .unwrap_or_default();
    Line::from(vec![
        Span::styled(
            format!("{}  ", fit(label, 18)),
            Style::default().fg(theme().text_secondary),
        ),
        Span::styled(
            format!("{value}{unit}{anchor}"),
            Style::default().fg(theme().text),
        ),
    ])
}

fn metric_series_short_label(
    series: &crate::model::local_observability::LocalMetricSeries,
) -> String {
    let identity = series
        .agent_id
        .as_deref()
        .or(series.agent_type.as_deref())
        .or_else(|| series.attributes.values().next().map(String::as_str))
        .unwrap_or("service");
    if series.statistic == "value" {
        identity.to_string()
    } else {
        format!("{identity}/{}", series.statistic)
    }
}

fn render_ops_resources(frame: &mut Frame<'_>, area: Rect, app: &TuiApp) {
    let chrome = PaneChrome { area };
    chrome.render_header(frame, area, "Configuration", PaneFocus::Active, &theme());
    if app.resources.loading.is_some() {
        PaneHeaderStatus {
            label: "refreshing",
            tone: PaneStatusTone::Loading,
        }
        .render(frame, area, &theme());
    }
    let outer_body = chrome.body_area(area);
    let [controls_row, body] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .areas(outer_body);
    let controls = chrome.content_area(controls_row);
    let scope_choices = ResourceScope::ALL.map(|scope| ControlChoice {
        label: scope.label(),
        selected: scope == app.resources.scope,
    });
    let groups = [ControlGroup {
        label: "Scope",
        choices: &scope_choices,
    }];
    render_ops_control_strip(frame, controls, app, &groups, None);

    let pane_layout = adaptive_ops_pane_layout(body, 96, &[28, 36, 36]);
    render_resource_kinds(frame, pane_layout.panes[0], chrome, app);
    render_resource_rows(frame, pane_layout.panes[1], chrome, app);
    render_resource_detail(frame, pane_layout.panes[2], chrome, app);
    chrome.render_dividers(frame, &pane_layout.dividers, &theme());
    chrome.render_body_boundary(frame, outer_body, &theme());
}

fn render_resource_kinds(frame: &mut Frame<'_>, area: Rect, chrome: PaneChrome, app: &TuiApp) {
    chrome.render_header(
        frame,
        area,
        "Kinds",
        if app.resources.focus() == resources::ResourceFocus::Kinds {
            PaneFocus::Active
        } else {
            PaneFocus::Idle
        },
        &theme(),
    );
    let outer_body = chrome.body_area(area);
    let body = chrome
        .region(
            outer_body,
            resources::ResourceKind::for_scope(app.resources.scope).len(),
        )
        .content_area;
    let lines = resources::ResourceKind::for_scope(app.resources.scope)
        .iter()
        .enumerate()
        .map(|(index, kind)| {
            let status = if app.resources.loading == Some(*kind) {
                "…".to_string()
            } else if app.resources.kind_error(*kind).is_some() {
                "!".to_string()
            } else {
                app.resources
                    .kind_count(*kind)
                    .map(|count| count.to_string())
                    .unwrap_or_else(|| "—".to_string())
            };
            metric_selection_line(
                format!("{}  {status}", kind.label()),
                index == app.resources.selected_kind_index(),
                body.width,
            )
        })
        .collect::<Vec<_>>();
    let (first_visible, viewport_height) = app.resources.kind_viewport();
    render_ops_selection_list(
        frame,
        outer_body,
        chrome,
        app,
        &lines,
        app.resources.selected_kind_index(),
        first_visible,
        viewport_height,
        RegionKind::ResourceKinds,
        ScrollbarTarget::ResourceKinds,
    );
    chrome.render_body_boundary(frame, outer_body, &theme());
}

fn render_resource_rows(frame: &mut Frame<'_>, area: Rect, chrome: PaneChrome, app: &TuiApp) {
    chrome.render_header(
        frame,
        area,
        app.resources.selected_kind().label(),
        if app.resources.focus() == resources::ResourceFocus::Items {
            PaneFocus::Active
        } else {
            PaneFocus::Idle
        },
        &theme(),
    );
    let outer_body = chrome.body_area(area);
    let body = chrome
        .region(outer_body, app.resources.visible_rows().len())
        .content_area;
    app.geometry.add_region(RegionKind::ResourceItems, body);
    if app.resources.loading == Some(app.resources.selected_kind())
        && !app.resources.is_loaded(app.resources.selected_kind())
    {
        render_metrics_notice(frame, body, NoticeKind::Info, "Loading this resource kind…");
        chrome.render_body_boundary(frame, outer_body, &theme());
        return;
    }
    if let Some(error) = app.resources.current_error() {
        render_metrics_notice(frame, body, NoticeKind::Warning, error);
        chrome.render_body_boundary(frame, outer_body, &theme());
        return;
    }
    if app.resources.visible_rows().is_empty() {
        render_metrics_notice(
            frame,
            body,
            NoticeKind::Empty,
            "No items in this resource kind",
        );
        chrome.render_body_boundary(frame, outer_body, &theme());
        return;
    }
    let lines = app
        .resources
        .visible_rows()
        .iter()
        .enumerate()
        .map(|(index, row)| {
            metric_selection_line(
                if row.summary.is_empty() {
                    row.name.clone()
                } else {
                    format!("{}  {}", row.name, row.summary)
                },
                index == app.resources.selected_row_index(),
                body.width,
            )
        })
        .collect::<Vec<_>>();
    let (first_visible, viewport_height) = app.resources.row_viewport();
    render_ops_selection_list(
        frame,
        outer_body,
        chrome,
        app,
        &lines,
        app.resources.selected_row_index(),
        first_visible,
        viewport_height,
        RegionKind::ResourceItems,
        ScrollbarTarget::ResourceItems,
    );
    chrome.render_body_boundary(frame, outer_body, &theme());
}

fn render_resource_detail(frame: &mut Frame<'_>, area: Rect, chrome: PaneChrome, app: &TuiApp) {
    let selected = app
        .resources
        .visible_rows()
        .get(app.resources.selected_row_index());
    chrome.render_header(
        frame,
        area,
        "Details",
        if app.resources.focus() == resources::ResourceFocus::Details {
            PaneFocus::Active
        } else {
            PaneFocus::Idle
        },
        &theme(),
    );
    let outer_body = chrome.body_area(area);
    let body = chrome.content_area(outer_body);
    let build_lines = |width: u16| {
        selected.map_or_else(Vec::new, |row| {
            let mut lines = wrap_styled_line(
                Line::from(Span::styled(
                    row.name.clone(),
                    Style::default()
                        .fg(theme().text)
                        .add_modifier(Modifier::BOLD),
                )),
                width as usize,
            );
            lines.push(Line::default());
            for field in &row.fields {
                lines.extend(wrap_styled_line(
                    Line::from(vec![
                        Span::styled(
                            format!("{}  ", field.label),
                            Style::default().fg(theme().text_secondary),
                        ),
                        Span::styled(field.value.clone(), Style::default().fg(theme().text)),
                    ]),
                    width as usize,
                ));
            }
            lines
        })
    };
    let mut lines = build_lines(body.width);
    let mut region = chrome.region(outer_body, lines.len());
    if region.content_area.width != body.width {
        lines = build_lines(region.content_area.width);
        region = chrome.region(outer_body, lines.len());
    }
    app.resources
        .set_details_dimensions(lines.len(), region.content_area.height);
    app.geometry
        .add_region(RegionKind::ResourceDetails, region.content_area);
    ScrollableDocument {
        lines: &lines,
        offset: app.resources.details_scroll() as usize,
    }
    .render(frame, region, &theme());
    if let Some(slot) = region.scrollbar {
        app.geometry.add_region(
            RegionKind::Scrollbar(ScrollbarTarget::ResourceDetails),
            slot.area,
        );
    }
    chrome.render_body_boundary(frame, outer_body, &theme());
}

fn metric_selection_line(text: String, selected: bool, width: u16) -> Line<'static> {
    let marker = if selected { "│" } else { " " };
    let style = if selected {
        Style::default()
            .fg(theme().table_selection_text)
            .bg(theme().table_selected_even_background)
    } else {
        Style::default().fg(theme().text)
    };
    Line::from(Span::styled(
        fit(&format!("{marker} {text}"), width as usize),
        style,
    ))
}

fn metric_sparkline_with_bounds(
    samples: &[crate::model::local_observability::LocalMetricSample],
    width: usize,
    time_from: i64,
    time_to: i64,
    min: f64,
    max: f64,
    cursor_timestamp: Option<i64>,
) -> String {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let width = width.max(1);
    if samples.is_empty() || !min.is_finite() || !max.is_finite() {
        return "·".repeat(width);
    }
    let span = time_to.saturating_sub(time_from).max(1);
    let mut values = vec![None; width];
    for sample in samples {
        let offset = sample.timestamp_unix_nanos.saturating_sub(time_from);
        let column = ((offset as i128 * width.saturating_sub(1) as i128) / span as i128)
            .clamp(0, width.saturating_sub(1) as i128) as usize;
        values[column] = Some(sample.value);
    }
    let mut rendered = values
        .into_iter()
        .map(|value| {
            let Some(value) = value else {
                return '·';
            };
            let index = if (max - min).abs() < f64::EPSILON {
                3
            } else {
                (((value - min) / (max - min)) * 7.0).round() as usize
            };
            BARS[index.min(7)]
        })
        .collect::<Vec<_>>();
    if let Some(timestamp) = cursor_timestamp
        && timestamp >= time_from
        && timestamp <= time_to
    {
        let offset = timestamp.saturating_sub(time_from);
        let column = ((offset as i128 * width.saturating_sub(1) as i128) / span as i128)
            .clamp(0, width.saturating_sub(1) as i128) as usize;
        rendered[column] = '│';
    }
    rendered.into_iter().collect()
}

fn format_metric_value(value: f64) -> String {
    if value.abs() >= 1_000_000.0 {
        format!("{:.2}M", value / 1_000_000.0)
    } else if value.abs() >= 1_000.0 {
        format!("{:.2}k", value / 1_000.0)
    } else if value.fract().abs() < f64::EPSILON {
        format!("{value:.0}")
    } else {
        format!("{value:.3}")
    }
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
    let title = format!("Agents / {}", identity.agent_id);
    let lines = build_ops_agent_details_lines(
        agent,
        app.agents.included.len(),
        &app.agents.agent_types,
        app.agents.metadata_error.as_deref(),
        area.width,
    );
    render_ops_document(frame, area, app, &title, lines);
    let links = [
        PaneHeaderAction {
            label: "Oplog",
            available: true,
        },
        PaneHeaderAction {
            label: "Metrics",
            available: true,
        },
        PaneHeaderAction {
            label: "Stream",
            available: app.related_stream_available(),
        },
        PaneHeaderAction {
            label: "SSH",
            available: true,
        },
    ];
    let regions = PaneHeaderActions { actions: &links }.render(
        frame,
        area,
        (UnicodeWidthStr::width(title.as_str()) as u16).saturating_add(6),
        0,
        &theme(),
    );
    for (kind, region) in [
        RegionKind::OpsLink(OpsDestination::Oplog),
        RegionKind::OpsLink(OpsDestination::Metrics),
        RegionKind::OpsLink(OpsDestination::Stream),
        RegionKind::AgentSsh,
    ]
    .into_iter()
    .zip(regions)
    {
        if let Some(region) = region {
            app.geometry.add_region(kind, region);
        }
    }
}

fn render_ops_ssh(frame: &mut Frame<'_>, area: Rect, app: &TuiApp, identity: &AgentIdentity) {
    let chrome = PaneChrome { area };
    let title = format!("Agents / SSH / {}", identity.agent_id);
    chrome.render_header(frame, area, &title, PaneFocus::Active, &theme());
    PaneHeaderStatus {
        label: app.ssh.status.label(),
        tone: match app.ssh.status {
            SshStatus::Ready => PaneStatusTone::Active,
            SshStatus::Connecting | SshStatus::Running => PaneStatusTone::Loading,
            SshStatus::Failed => PaneStatusTone::Error,
            SshStatus::Disconnected => PaneStatusTone::Idle,
        },
    }
    .render(frame, area, &theme());

    let outer_body = chrome.body_area(area);
    let [context_row, output_body, input_row] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .areas(outer_body);
    let context = chrome.content_area(context_row);
    let place = if app.ssh.cwd().is_empty() {
        "agent start"
    } else {
        app.ssh.cwd()
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Agent  ", Style::default().fg(theme().text_muted)),
            Span::styled(
                fit(
                    &identity.agent_id,
                    context.width.saturating_sub(16) as usize,
                ),
                Style::default().fg(theme().info),
            ),
            Span::styled("  cwd  ", Style::default().fg(theme().text_muted)),
            Span::styled(
                place.to_string(),
                Style::default().fg(theme().text_secondary),
            ),
        ]))
        .style(surface_style()),
        context,
    );

    let output = chrome.content_area(output_body);
    app.geometry.add_region(RegionKind::OpsList, output_body);
    let output_width = output.width.saturating_sub(1) as usize;
    let mut lines = Vec::new();
    for line in &app.ssh.lines {
        let (prefix, color) = match line.kind {
            SshLineKind::Command => ("❯ ", theme().accent_hover),
            SshLineKind::Stdout => ("  ", theme().text),
            SshLineKind::Stderr => ("! ", theme().marker),
            SshLineKind::Status => ("· ", theme().text_muted),
            SshLineKind::Error => ("! ", theme().error),
        };
        lines.extend(wrap_styled_line(
            Line::from(vec![
                Span::styled(prefix, Style::default().fg(color)),
                Span::styled(line.text.clone(), Style::default().fg(color)),
            ]),
            output_width.max(1),
        ));
    }
    if lines.is_empty() {
        let message = match app.ssh.status {
            SshStatus::Connecting => "Connecting to the Agent's bound `bash` tool",
            SshStatus::Failed => "SSH could not connect; use the command palette to retry",
            _ => "Type a command below; each submission is one durable `bash run` call",
        };
        lines.push(
            Notice {
                kind: if app.ssh.status == SshStatus::Failed {
                    NoticeKind::Error
                } else {
                    NoticeKind::Info
                },
                message,
            }
            .line(&theme()),
        );
    }
    let line_count = lines.len();
    app.ssh.content_height.set(line_count);
    app.ssh.viewport_height.set(output.height as usize);
    let maximum = line_count.saturating_sub(output.height as usize);
    let scroll = app.ssh.scroll.min(maximum);
    frame.render_widget(
        Paragraph::new(lines)
            .style(surface_style())
            .scroll((scroll as u16, 0)),
        output,
    );
    if line_count > output.height as usize && output.width > 0 {
        let slot = PaneScrollbarSlot {
            area: Rect::new(output.right().saturating_sub(1), output.y, 1, output.height),
        };
        let mut state = ScrollbarState::new(line_count)
            .position(scroll)
            .viewport_content_length(output.height as usize);
        render_scrollbar(frame, slot, &mut state, &theme());
        app.geometry.add_region(
            RegionKind::Scrollbar(ScrollbarTarget::OpsDocument),
            slot.area,
        );
    }

    let input = chrome.content_area(input_row);
    app.geometry
        .add_region(RegionKind::AgentSshInput, input_row);
    let editor = TextEditState {
        draft: app.ssh.input.clone(),
        cursor: app.ssh.cursor,
    };
    let value_width = input.width.saturating_sub(4) as usize;
    let (visible, cursor_column) = editor.viewport(value_width);
    let active = app.mode == InteractionMode::SshInput && app.ssh.status == SshStatus::Ready;
    let marker = if app.ssh.status == SshStatus::Running {
        "◌ "
    } else {
        "❯ "
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                marker,
                Style::default()
                    .fg(if active {
                        theme().accent_hover
                    } else {
                        theme().text_muted
                    })
                    .bg(theme().input_background),
            ),
            Span::styled(
                if visible.is_empty() && !active {
                    "enter to type".to_string()
                } else {
                    visible
                },
                Style::default()
                    .fg(if active {
                        theme().input_text
                    } else {
                        theme().text_faint
                    })
                    .bg(theme().input_background),
            ),
        ]))
        .style(surface_style().bg(theme().input_background)),
        input,
    );
    if active && input.width > 2 && input.height > 0 {
        frame.set_cursor_position(Position::new(
            input
                .x
                .saturating_add(2)
                .saturating_add(cursor_column as u16)
                .min(input.right().saturating_sub(1)),
            input.y,
        ));
    }
    chrome.render_body_boundary(frame, outer_body, &theme());
}

fn render_ops_deployments(frame: &mut Frame<'_>, area: Rect, app: &TuiApp) {
    match app.ops.current_route() {
        OpsRoute::DeploymentHistory => render_ops_deployment_history(frame, area, app, false),
        OpsRoute::DeploymentRevision(revision) => {
            render_ops_deployment_revision(frame, area, app, *revision)
        }
        OpsRoute::DeploymentCompare(route) => render_ops_deployment_compare(
            frame,
            area,
            app,
            route.base_revision,
            route.target_revision,
            route.focus,
        ),
        _ => render_ops_deployment_history(frame, area, app, true),
    }
}

fn render_ops_deployment_history(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &TuiApp,
    recent_only: bool,
) {
    let chrome = PaneChrome { area };
    let mut lines = ops_environment_status_lines(app);
    if recent_only && let Some(snapshot) = &app.ops_environment.snapshot {
        match &snapshot.deployment {
            Some(deployment) => lines.push(Line::from(vec![
                Span::styled("Current  ", Style::default().fg(theme().text_muted)),
                Span::styled(
                    format!("revision {}", deployment.deployment_revision),
                    Style::default()
                        .fg(theme().text)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("  ·  {}  ·  {}", deployment.version, deployment.hash),
                    Style::default().fg(theme().text_secondary),
                ),
            ])),
            None => lines.push(
                Notice {
                    kind: NoticeKind::Empty,
                    message: "The selected environment has no current deployment",
                }
                .line(&theme()),
            ),
        }
    }
    if let Some(error) = &app.deployments.error {
        lines.push(
            Notice {
                kind: NoticeKind::Error,
                message: error,
            }
            .line(&theme()),
        );
    } else if app.deployments.loading && app.deployments.history.is_empty() {
        lines.push(Notice::animated_loading(
            "Loading deployment history",
            spinner_symbol(0),
            &theme(),
        ));
    }

    let title = if recent_only {
        "Deployments / Current & recent"
    } else {
        "Deployments / All History"
    };
    chrome.render_header(frame, area, title, PaneFocus::Active, &theme());
    let outer_body = chrome.body_area(area);
    let status_height = lines.len().min(outer_body.height as usize) as u16;
    let [status_row, table_body] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(status_height), Constraint::Min(1)])
        .areas(outer_body);
    let status_area = chrome.content_area(status_row);
    if status_height > 0 {
        frame.render_widget(Paragraph::new(lines).style(surface_style()), status_area);
    }

    let count = if recent_only {
        app.deployments.history.len().min(RECENT_LIMIT)
    } else {
        app.deployments.history.len()
    };
    let history = &app.deployments.history[..count];
    if history.is_empty() {
        if !app.deployments.loading && app.deployments.error.is_none() {
            frame.render_widget(
                Paragraph::new(
                    Notice {
                        kind: NoticeKind::Empty,
                        message: "No deployments are available for this environment",
                    }
                    .line(&theme()),
                )
                .style(surface_style()),
                table_body,
            );
        }
        chrome.render_body_boundary(frame, outer_body, &theme());
        return;
    }
    let owned_rows = history
        .iter()
        .map(|record| {
            vec![
                record.revision.to_string(),
                record.version.clone(),
                record.hash.clone(),
            ]
        })
        .collect::<Vec<_>>();
    let row_cells = owned_rows
        .iter()
        .map(|row| row.iter().map(String::as_str).collect::<Vec<_>>())
        .collect::<Vec<_>>();
    let rows = row_cells.iter().map(Vec::as_slice).collect::<Vec<_>>();
    render_deployment_table(
        frame,
        table_body,
        app,
        &DEPLOYMENT_COLUMNS,
        &rows,
        None,
        app.deployments.selected.min(count.saturating_sub(1)),
    );
    chrome.render_body_boundary(frame, outer_body, &theme());
}

fn render_ops_deployment_revision(frame: &mut Frame<'_>, area: Rect, app: &TuiApp, revision: u64) {
    let title = format!("Deployments / Revision {revision}");
    let mut lines = Vec::new();
    if let Some(error) = &app.deployments.error {
        lines.push(
            Notice {
                kind: NoticeKind::Error,
                message: error,
            }
            .line(&theme()),
        );
    }
    let Some(details) = app.deployments.details.get(&revision) else {
        if app.deployments.loading {
            lines.push(Notice::animated_loading(
                "Loading revision details",
                spinner_symbol(0),
                &theme(),
            ));
        } else if lines.is_empty() {
            lines.push(
                Notice {
                    kind: NoticeKind::Empty,
                    message: "Revision details are not loaded",
                }
                .line(&theme()),
            );
        }
        render_deployment_document(frame, area, app, &title, lines);
        return;
    };
    lines.push(
        FieldRow {
            label: "Hash",
            value: &details.hash,
            label_width: 8,
        }
        .line(&theme()),
    );
    let mut previous_kind = None;
    for entity in &details.entities {
        if previous_kind != Some(entity.kind) {
            lines.push(Line::default());
            lines.push(
                SectionHeading {
                    title: entity.kind.label(),
                    detail: None,
                }
                .line(&theme()),
            );
            previous_kind = Some(entity.kind);
        }
        lines.push(
            FieldRow {
                label: &entity.name,
                value: &entity.detail,
                label_width: 28,
            }
            .line(&theme()),
        );
    }
    if details.entities.is_empty() {
        lines.push(
            Notice {
                kind: NoticeKind::Empty,
                message: "This revision contains no deployable entities",
            }
            .line(&theme()),
        );
    }
    render_deployment_document(frame, area, app, &title, lines);
}

fn render_deployment_document(
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
    app.geometry.add_region(RegionKind::OpsList, body);
    let pane_layout = PaneLayout::horizontal(body, &[1]);
    let Some(region) = pane_layout.regions(&[lines.len()]).into_iter().next() else {
        return;
    };
    app.deployments.content_height.set(lines.len());
    app.deployments
        .viewport_height
        .set(region.content_area.height as usize);
    ScrollableDocument {
        lines: &lines,
        offset: app.deployments.scroll,
    }
    .render(frame, region, &theme());
    if let Some(slot) = region.scrollbar {
        app.geometry.add_region(
            RegionKind::Scrollbar(ScrollbarTarget::OpsDocument),
            slot.area,
        );
    }
    pane_layout.render_body_boundaries(frame, &[region], &theme());
}

fn render_ops_deployment_compare(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &TuiApp,
    base: u64,
    target: u64,
    focus: DeploymentCompareFocus,
) {
    let chrome = PaneChrome { area };
    chrome.render_header(
        frame,
        area,
        "Deployments / Compare",
        PaneFocus::Active,
        &theme(),
    );
    let outer_body = chrome.body_area(area);
    let [selector_row, table_body] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .areas(outer_body);
    let selectors = chrome.content_area(selector_row);
    let selector = |label: &'static str, revision: u64, active: bool| {
        Span::styled(
            format!(" {label} {revision} "),
            if active {
                Style::default()
                    .fg(theme().selection_text)
                    .bg(theme().selection_background)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme().text_secondary)
            },
        )
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            selector("Base", base, focus == DeploymentCompareFocus::Base),
            Span::styled("  →  ", Style::default().fg(theme().border_subtle)),
            selector("Target", target, focus == DeploymentCompareFocus::Target),
            Span::styled(
                "  tab field · ←/→ revision",
                Style::default().fg(theme().text_muted),
            ),
        ]))
        .style(surface_style()),
        selectors,
    );
    let base_width = UnicodeWidthStr::width(format!(" Base {base} ").as_str()) as u16;
    if base_width <= selectors.width {
        app.geometry.add_region(
            RegionKind::OpsControl {
                group: 0,
                choice: 0,
            },
            Rect::new(selectors.x, selectors.y, base_width, 1),
        );
    }
    let target_x = selectors.x.saturating_add(base_width).saturating_add(5);
    let target_width = UnicodeWidthStr::width(format!(" Target {target} ").as_str()) as u16;
    if target_x.saturating_add(target_width) <= selectors.right() {
        app.geometry.add_region(
            RegionKind::OpsControl {
                group: 0,
                choice: 1,
            },
            Rect::new(target_x, selectors.y, target_width, 1),
        );
    }
    if let Some(error) = &app.deployments.error {
        frame.render_widget(
            Paragraph::new(
                Notice {
                    kind: NoticeKind::Error,
                    message: error,
                }
                .line(&theme()),
            )
            .style(surface_style()),
            table_body,
        );
        chrome.render_body_boundary(frame, outer_body, &theme());
        return;
    }
    let Some(changes) = app.deployments.changes(base, target) else {
        frame.render_widget(
            Paragraph::new(Notice::animated_loading(
                "Loading both revisions",
                spinner_symbol(0),
                &theme(),
            ))
            .style(surface_style()),
            table_body,
        );
        chrome.render_body_boundary(frame, outer_body, &theme());
        return;
    };
    if changes.is_empty() {
        frame.render_widget(
            Paragraph::new(
                Notice {
                    kind: NoticeKind::Empty,
                    message: "No deployable entities changed between these revisions",
                }
                .line(&theme()),
            )
            .style(surface_style()),
            table_body,
        );
        chrome.render_body_boundary(frame, outer_body, &theme());
        return;
    }
    let owned_rows = changes
        .iter()
        .map(|change| {
            vec![
                change.change.label().to_string(),
                change.kind.item_label().to_string(),
                change.name.clone(),
                change.before.clone().unwrap_or_else(|| "—".to_string()),
                change.after.clone().unwrap_or_else(|| "—".to_string()),
            ]
        })
        .collect::<Vec<_>>();
    let row_cells = owned_rows
        .iter()
        .map(|row| row.iter().map(String::as_str).collect::<Vec<_>>())
        .collect::<Vec<_>>();
    let rows = row_cells.iter().map(Vec::as_slice).collect::<Vec<_>>();
    let owned_tones = changes
        .iter()
        .map(|change| {
            vec![
                Some(match change.change {
                    DeploymentChangeKind::Added => CellTone::Success,
                    DeploymentChangeKind::Changed => CellTone::Warning,
                    DeploymentChangeKind::Removed => CellTone::Error,
                }),
                None,
                None,
                None,
                None,
            ]
        })
        .collect::<Vec<_>>();
    let tones = owned_tones.iter().map(Vec::as_slice).collect::<Vec<_>>();
    render_deployment_table(
        frame,
        table_body,
        app,
        &DEPLOYMENT_CHANGE_COLUMNS,
        &rows,
        Some(&tones),
        app.deployments
            .selected_change
            .min(changes.len().saturating_sub(1)),
    );
    chrome.render_body_boundary(frame, outer_body, &theme());
}

fn render_deployment_table(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &TuiApp,
    columns: &[PaneTableColumn<'_>],
    rows: &[&[&str]],
    tones: Option<&[&[Option<CellTone>]]>,
    selected: usize,
) {
    app.geometry.add_region(RegionKind::OpsList, area);
    let mut state = PaneTableState::new(columns, selected);
    let resolved = responsive_table_columns(columns, rows, &state, area.width.saturating_sub(2));
    state = PaneTableState::new(&resolved, selected);
    let table = PaneTable {
        columns: &resolved,
        rows,
        state: &state,
        cell_tones: tones,
        rich_cells: None,
        row_markers: None,
        first_row_index: 0,
    };
    let table_height = table.height();
    let chrome = PaneChrome { area };
    let region = chrome.region(area, table_height);
    let window = table.window_from(
        region.content_area.height,
        app.deployments.table_first_visible.get(),
    );
    app.deployments.table_first_visible.set(window.start_row);
    state.selected = window.selected_row;
    PaneTable {
        columns: &resolved,
        rows: &rows[window.start_row..],
        state: &state,
        cell_tones: tones.map(|tones| &tones[window.start_row..]),
        rich_cells: None,
        row_markers: None,
        first_row_index: window.start_row,
    }
    .render(frame, region.content_area, &theme());
    if let Some(slot) = region.scrollbar {
        let mut scrollbar = ScrollbarState::new(table_height)
            .position(window.selected_line_offset)
            .viewport_content_length(region.content_area.height as usize);
        render_scrollbar(frame, slot, &mut scrollbar, &theme());
        app.geometry
            .add_region(RegionKind::Scrollbar(ScrollbarTarget::OpsList), slot.area);
    }
    chrome.render_body_boundary(frame, area, &theme());
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
    if let Some(slot) = region.scrollbar {
        app.geometry.add_region(
            RegionKind::Scrollbar(ScrollbarTarget::OpsDocument),
            slot.area,
        );
    }
    pane_layout.render_body_boundaries(frame, &[region], &theme());
}

fn render_ops_agents_overview(frame: &mut Frame<'_>, area: Rect, app: &TuiApp) {
    let chrome = PaneChrome { area };
    let list_pane = app.geometry.region(RegionKind::OpsList).unwrap_or(area);
    let details_pane = app.geometry.region(RegionKind::OpsDetails);
    let divider = app.geometry.region(RegionKind::OpsDetailsSplit);
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
    let scope = app.agent_handoff_scope();
    let scope_available = !scope.is_empty() && scope.len() <= activity::HARD_AGENT_LIMIT;
    let links = [
        (OpsDestination::Oplog, "Oplog", scope_available),
        (OpsDestination::Metrics, "Metrics", scope_available),
        (
            OpsDestination::Stream,
            "Stream",
            app.related_stream_available(),
        ),
    ];
    if let Some([details_header, _]) = details_parts {
        render_ops_related_links(frame, details_header, app, &links, 12, 0);
    } else {
        render_ops_related_links(frame, list_header, app, &links, 22, 18);
    }

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
        cell_tones: Some(&tones),
        rich_cells: Some(&rich_cells),
        row_markers: Some(&row_markers),
        first_row_index: 0,
    }
    .height();
    let query_bar = CollectionQueryBar {
        dataset: &app.agents.dataset_label(),
        mode: app.agents.mode.label(),
        scope: app.agents.query_scope.label(),
        query: &app.agents.query,
        matched: filtered.len(),
        loaded: app.agents.agents.len(),
        more_available: app.agents.has_more(),
        editing: app.mode == InteractionMode::AgentFilter,
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
    let query_cursor_column = query_bar.cursor_column();
    frame.render_widget(
        Paragraph::new(query_bar.lines(&theme())).style(surface_style()),
        query_area,
    );
    if let Some(cursor_column) = query_cursor_column
        && query_area.width > 0
        && query_area.height > 1
    {
        frame.set_cursor_position(Position::new(
            query_area
                .x
                .saturating_add((cursor_column as u16).min(query_area.width - 1)),
            query_area.y.saturating_add(1),
        ));
    }
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
        app.geometry
            .add_region(RegionKind::Scrollbar(ScrollbarTarget::OpsList), slot.area);
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
        if let Some(slot) = details_region.scrollbar {
            app.geometry.add_region(
                RegionKind::Scrollbar(ScrollbarTarget::OpsDetails),
                slot.area,
            );
        }
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

fn render_shell_header(frame: &mut Frame<'_>, area: Rect, app: &TuiApp) {
    ContextHeader {
        pairs: &[
            ContextPair {
                label: "app",
                value: &app.selected_target.context.application,
            },
            ContextPair {
                label: "env",
                value: &app.selected_target.context.environment,
            },
            ContextPair {
                label: "server",
                value: &app.selected_target.context.server,
            },
        ],
    }
    .render(frame, area, &theme());
}

#[derive(Clone, Copy)]
struct FooterSessionPresentation {
    view: DevView,
    name: &'static str,
    state: Option<&'static str>,
    kind: StatusKind,
}

impl FooterSessionPresentation {
    fn width(self) -> u16 {
        let state_width = self
            .state
            .map_or(0, |state| 1 + UnicodeWidthStr::width(state));
        (UnicodeWidthStr::width(self.name) + 2 + state_width) as u16
    }
}

fn footer_session_presentations(app: &TuiApp, width: u16) -> Vec<FooterSessionPresentation> {
    if width < 40 {
        return Vec::new();
    }
    let show_state = width >= layout::SHELL_COMPACT_WIDTH;
    vec![
        FooterSessionPresentation {
            view: DevView::Server,
            name: "Server",
            state: show_state.then_some(app.server.run.status.title()),
            kind: server_status_kind(app.server.run.status),
        },
        FooterSessionPresentation {
            view: DevView::Repl,
            name: "REPL",
            state: show_state.then_some(app.repl.run.status.title()),
            kind: repl_status_kind(app.repl.run.status),
        },
    ]
}

fn footer_session_status_widths(app: &TuiApp, width: u16) -> [u16; 2] {
    let presentations = footer_session_presentations(app, width);
    if let [server, repl] = presentations.as_slice() {
        [server.width(), repl.width()]
    } else {
        [0, 0]
    }
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
            active: app.active_workspace == Workspace::Dev,
        },
        WorkspaceItem {
            key: "",
            label: "Ops",
            active: app.active_workspace == Workspace::Ops,
        },
    ];
    WorkspaceSelector {
        items: &workspace_items,
        junctions: &[],
    }
    .render(frame, rows[0], &theme());
    let status_items = footer_session_presentations(app, area.width)
        .into_iter()
        .filter_map(|presentation| {
            app.geometry
                .region(RegionKind::DevSessionStatus(presentation.view))
                .map(|status_area| FooterStatusItem {
                    name: presentation.name,
                    state: presentation.state,
                    kind: presentation.kind,
                    area: status_area,
                })
        })
        .collect::<Vec<_>>();
    FooterStatusCluster {
        items: &status_items,
    }
    .render(frame, &theme());

    if app.mode == InteractionMode::Leader {
        let items = leader_key_hints(app);
        for (index, row) in rows.iter().enumerate().skip(1) {
            ShortcutRow {
                items: if index == 1 { &items } else { &[] },
                active: true,
                left_glyph: if index + 1 == rows.len() {
                    "└"
                } else {
                    "│"
                },
                fallback: None,
            }
            .render(frame, *row, &theme());
        }
        return;
    }

    if app.mode == InteractionMode::TerminalInput {
        let items = [
            KeyHint {
                key: "ctrl+x",
                label: "TUI control",
            },
            KeyHint {
                key: "all other keys",
                label: "Nested CLI",
            },
        ];
        for (index, row) in rows.iter().enumerate().skip(1) {
            ShortcutRow {
                items: if index == 1 { &items } else { &[] },
                active: true,
                left_glyph: if index + 1 == rows.len() {
                    "└"
                } else {
                    "│"
                },
                fallback: Some(items[0]),
            }
            .render(frame, *row, &theme());
        }
        return;
    }

    if app.mode == InteractionMode::SshInput {
        let items = [
            KeyHint {
                key: "enter",
                label: "Run command",
            },
            KeyHint {
                key: "ctrl+c",
                label: "Cancel command",
            },
            KeyHint {
                key: "↑ / ↓",
                label: "History",
            },
            KeyHint {
                key: "esc",
                label: "TUI control",
            },
            KeyHint {
                key: "ctrl+p",
                label: "Commands",
            },
        ];
        for (index, row) in rows.iter().enumerate().skip(1) {
            ShortcutRow {
                items: if index == 1 { &items } else { &[] },
                active: true,
                left_glyph: if index + 1 == rows.len() {
                    "└"
                } else {
                    "│"
                },
                fallback: Some(items[0]),
            }
            .render(frame, *row, &theme());
        }
        return;
    }

    let global = [
        KeyHint {
            key: "ctrl+x",
            label: "Menu",
        },
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
                label: "Commands",
            },
        ];
        let dev_terminal = [
            KeyHint {
                key: "enter",
                label: "CLI input",
            },
            KeyHint {
                key: "pageup",
                label: "Scrollback",
            },
            KeyHint {
                key: "tab / shift+tab",
                label: "Focus pane",
            },
            KeyHint {
                key: "ctrl+x a",
                label: "Commands",
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
                label: "Commands",
            },
        ];
        let deployment_history = [
            KeyHint {
                key: "↑/↓",
                label: "Select revision",
            },
            KeyHint {
                key: "enter",
                label: "Open revision",
            },
            KeyHint {
                key: "ctrl+a",
                label: "All history",
            },
            KeyHint {
                key: "ctrl+r",
                label: "Refresh",
            },
        ];
        let deployment_revision = [
            KeyHint {
                key: "↑/↓ / pgup/pgdn",
                label: "Scroll",
            },
            KeyHint {
                key: "enter",
                label: "Compare predecessor",
            },
            KeyHint {
                key: "esc",
                label: "Back",
            },
            KeyHint {
                key: "ctrl+r",
                label: "Refresh",
            },
        ];
        let deployment_compare = [
            KeyHint {
                key: "tab",
                label: "Base / target",
            },
            KeyHint {
                key: "←/→",
                label: "Choose revision",
            },
            KeyHint {
                key: "↑/↓",
                label: "Select change",
            },
            KeyHint {
                key: "esc",
                label: "Back",
            },
        ];
        let dev_layout_mode = [
            KeyHint {
                key: "tab / shift+tab",
                label: "Focus pane",
            },
            KeyHint {
                key: "arrows / shift+arrows",
                label: "Resize 1 / 5",
            },
            KeyHint {
                key: "b / s / r",
                label: "Toggle views",
            },
            KeyHint {
                key: "m",
                label: "Make main",
            },
            KeyHint {
                key: "f",
                label: "Right / bottom",
            },
            KeyHint {
                key: "enter / esc",
                label: "Done",
            },
        ];
        let ops_layout_mode = [
            KeyHint {
                key: "tab / shift+tab",
                label: "Focus pane",
            },
            KeyHint {
                key: "arrows / shift+arrows",
                label: "Resize 1 / 5",
            },
            KeyHint {
                key: "d",
                label: "Toggle Details",
            },
            KeyHint {
                key: "f",
                label: "Right / bottom",
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
        let items = if app.mode == InteractionMode::Layout && app.active_workspace == Workspace::Dev
        {
            dev_layout_mode.as_slice()
        } else if app.mode == InteractionMode::Layout {
            ops_layout_mode.as_slice()
        } else if app.mode == InteractionMode::TerminalScrollback {
            scrollback_mode.as_slice()
        } else if app.active_workspace == Workspace::Dev {
            if app.focused_terminal_is_running() {
                dev_terminal.as_slice()
            } else {
                dev.as_slice()
            }
        } else if app.ops.active() == OpsDestination::Deployments {
            match app.ops.current_route() {
                OpsRoute::DeploymentRevision(_) => deployment_revision.as_slice(),
                OpsRoute::DeploymentCompare(_) => deployment_compare.as_slice(),
                _ => deployment_history.as_slice(),
            }
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
    let search = SearchInput {
        query: &filter.query,
        placeholder: "type component or agent type",
    };
    let summary = format!(
        "Server-side filter · {}/{} choices",
        filtered_indices.len(),
        filter.choices.len()
    );
    let mut lines = vec![
        Notice {
            kind: NoticeKind::Info,
            message: &summary,
        }
        .line(&theme()),
        search.line(content.width as usize, &theme()),
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
    if content.width > 0 && content.height > 1 {
        frame.set_cursor_position(Position::new(
            content
                .x
                .saturating_add(search.cursor_column(content.width as usize) as u16),
            content.y.saturating_add(1),
        ));
    }
}

fn decision_table_widths(total_width: usize, first_percent: usize) -> [usize; 2] {
    let available = total_width.saturating_sub(3);
    let first = available.saturating_mul(first_percent) / 100;
    [first, available.saturating_sub(first)]
}

fn render_dev_workspace(frame: &mut Frame<'_>, area: Rect, app: &TuiApp) {
    frame.render_widget(Paragraph::new("").style(surface_style()), area);
    let chrome = PaneChrome { area };
    let input = LayoutInput {
        area,
        active_workspace: app.active_workspace,
        dev_primary: app.dev_primary,
        focused_dev_view: app.dev_focus,
        mode: app.mode.current(),
        context_picker_rows: context_picker_layout_rows(&app.context_switcher),
        context_picker_selected: app.context_switcher.selected,
        context_picker_prefix_height: context_picker_prefix_height(&app.context_switcher),
        ops_details_visible: false,
        dev_session_status_widths: [0, 0],
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
        Paragraph::new(repl_status_line(
            &app.repl,
            terminal_input_label(app, NestedCliTarget::Repl),
        ))
        .style(surface_style()),
        summary_area,
    );

    let terminal_content = terminal_content_area(terminal_area);
    let lines = if app.repl.run.status == ReplStatus::Stopped && app.repl.run.exit_code.is_none() {
        vec![prefixed_line(
            "Open Commands with ctrl+x a to start `golem repl`.",
        )]
    } else {
        repl_screen_lines(&app.repl.run, terminal_content.height as usize)
    };

    frame.render_widget(
        Paragraph::new(lines).style(surface_style()),
        terminal_content,
    );
    if let Some(slot) = render_output_scrollbar(frame, terminal_area, &app.repl.run.screen) {
        app.geometry.add_region(
            RegionKind::Scrollbar(ScrollbarTarget::Dev(DevView::Repl)),
            slot.area,
        );
    }

    render_terminal_cursor(
        frame,
        terminal_content,
        &app.repl.run.screen,
        app.terminal_input_owns(NestedCliTarget::Repl),
    );
}

fn render_server_view(frame: &mut Frame<'_>, area: Rect, app: &TuiApp) {
    frame.render_widget(Paragraph::new("").style(surface_style()), area);
    let [summary_area, output_area] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .areas(area);

    frame.render_widget(
        Paragraph::new(server_status_line(
            &app.server,
            terminal_input_label(app, NestedCliTarget::Server),
        ))
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
    let output_content = terminal_content_area(output_area);
    frame.render_widget(
        Paragraph::new(output_lines)
            .style(surface_style())
            .wrap(Wrap { trim: false }),
        output_content,
    );
    if let Some(slot) = render_output_scrollbar(frame, output_area, &app.server.run.terminal) {
        app.geometry.add_region(
            RegionKind::Scrollbar(ScrollbarTarget::Dev(DevView::Server)),
            slot.area,
        );
    }
    render_terminal_cursor(
        frame,
        output_content,
        &app.server.run.terminal,
        app.terminal_input_owns(NestedCliTarget::Server),
    );
}

fn surface_style() -> Style {
    Style::default().fg(theme().text).bg(theme().surface)
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
    if overflow {
        app.geometry.add_region(
            RegionKind::Scrollbar(ScrollbarTarget::Help),
            Rect {
                x: body.right().saturating_sub(1),
                width: 1,
                ..body
            },
        );
    }
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
            ActionId::OpenPalette,
            ActionId::OpenContextPicker,
            ActionId::ShowHelp,
            ActionId::Quit,
        ],
    );
    push_section_break(&mut lines, width, "Leader");
    push_raw_help(&mut lines, width, "ctrl+x w", "Workspaces");
    push_raw_help(&mut lines, width, "ctrl+x v", "Views");
    push_raw_help(&mut lines, width, "ctrl+x l", "Layout mode");
    push_raw_help(&mut lines, width, "ctrl+x a", "Commands · Context");
    push_raw_help(&mut lines, width, "ctrl+x j", "Jobs");
    push_raw_help(&mut lines, width, "ctrl+x e", "Context");

    push_section_break(&mut lines, width, "Palette");
    push_raw_help(&mut lines, width, "type", "Filter commands");
    push_raw_help(&mut lines, width, "up / down", "Move selection");
    push_raw_help(&mut lines, width, "left / right", "Switch All / Context");
    push_raw_help(&mut lines, width, "enter", "Execute selected action");
    push_raw_help(&mut lines, width, "esc / ctrl+c", "Close palette");

    if app.active_workspace == Workspace::Dev {
        push_section_break(&mut lines, width, "Dev panes");
        push_raw_help(&mut lines, width, "tab / shift+tab", "Focus visible pane");
        push_raw_help(
            &mut lines,
            width,
            "ctrl+x l",
            "Resize, place, show, hide, or promote panes",
        );
        push_raw_help(&mut lines, width, "pageup", "Enter terminal scrollback");
        push_raw_help(&mut lines, width, "mouse wheel", "Scroll terminal history");
        push_raw_help(&mut lines, width, "ctrl+x", "Return CLI input to the TUI");
        push_raw_help(
            &mut lines,
            width,
            "enter / click",
            "Give a running pane input to the CLI",
        );
        push_raw_help(
            &mut lines,
            width,
            "esc",
            "Return from scrollback to the prior input owner",
        );
    } else if app.ops.active() == OpsDestination::Overview {
        push_section_break(&mut lines, width, "Overview");
        if matches!(app.ops.current_route(), OpsRoute::OverviewIssues) {
            push_raw_help(&mut lines, width, "up / down", "Select an issue");
            push_raw_help(
                &mut lines,
                width,
                "enter / click",
                "Open owning destination",
            );
            push_raw_help(&mut lines, width, "esc", "Return to Overview");
        } else if matches!(app.ops.current_route(), OpsRoute::OverviewServer) {
            push_raw_help(&mut lines, width, "ctrl+r", "Refresh Overview status");
            push_raw_help(&mut lines, width, "esc", "Return to Overview");
        } else {
            push_raw_help(&mut lines, width, "up / down", "Select a destination");
            push_raw_help(&mut lines, width, "enter / click", "Open destination");
            push_raw_help(&mut lines, width, "ctrl+r", "Refresh Overview status");
        }
    } else if app.ops.active() == OpsDestination::Oplog {
        push_section_break(&mut lines, width, "Oplog");
        push_raw_help(
            &mut lines,
            width,
            "left / right",
            "Grouped or exact Entries",
        );
        push_raw_help(
            &mut lines,
            width,
            "tab / shift+tab",
            "Focus journal or details",
        );
        push_raw_help(&mut lines, width, "up / down", "Select or scroll details");
        push_raw_help(&mut lines, width, "pageup / pagedown", "Move by page");
        push_raw_help(
            &mut lines,
            width,
            "type / backspace",
            "Edit the selected Loaded filter or Server search",
        );
        push_raw_help(&mut lines, width, "ctrl+f", "Filter entries already loaded");
        push_raw_help(&mut lines, width, "ctrl+s", "Edit server-side search");
        push_raw_help(&mut lines, width, "enter", "Run selected server search");
        push_raw_help(
            &mut lines,
            width,
            "ctrl+enter",
            "Load contiguous context around the selected entry",
        );
        push_raw_help(&mut lines, width, "ctrl+n", "Load the next bounded batch");
        push_raw_help(&mut lines, width, "ctrl+g", "Stop the active search");
        push_raw_help(&mut lines, width, "ctrl+o", "Toggle newest / oldest order");
        push_raw_help(&mut lines, width, "ctrl+r", "Restart the bounded query");
        push_raw_help(&mut lines, width, "ctrl+x l", "Resize or hide details");
        push_raw_help(&mut lines, width, "ctrl+x v o", "Open Overview");
    } else if app.ops.active() == OpsDestination::Stream {
        push_action_section_for_app(
            &mut lines,
            app,
            width,
            "Agent Stream",
            &[ActionId::ToggleAgentStream],
        );
        push_raw_help(&mut lines, width, "up / down", "Scroll buffered events");
        push_raw_help(&mut lines, width, "pageup / pagedown", "Scroll by page");
        push_raw_help(&mut lines, width, "home", "Show oldest buffered events");
        push_raw_help(&mut lines, width, "end", "Follow latest events");
        push_raw_help(&mut lines, width, "ctrl+x v a", "Select an exact Agent");
    } else if app.ops.active() == OpsDestination::Metrics {
        push_section_break(&mut lines, width, "Metrics");
        if matches!(app.ops.current_route(), OpsRoute::MetricsPanel(_)) {
            push_raw_help(
                &mut lines,
                width,
                "shift+left / shift+right",
                "Inspect chart samples",
            );
            push_raw_help(&mut lines, width, "click / wheel", "Inspect chart sample");
            push_raw_help(
                &mut lines,
                width,
                "enter",
                "Open exact sample or aggregate bucket in Oplog",
            );
            push_raw_help(&mut lines, width, "esc", "Return to dashboard");
        } else {
            push_raw_help(&mut lines, width, "left / right", "Dashboard or Explore");
            if app.metrics.mode == MetricsMode::Dashboard {
                push_raw_help(
                    &mut lines,
                    width,
                    "up / down / wheel",
                    "Select dashboard panel",
                );
                push_raw_help(&mut lines, width, "enter / click", "Expand selected panel");
            } else {
                push_raw_help(
                    &mut lines,
                    width,
                    "tab / shift+tab",
                    "Focus family, series, or chart",
                );
                push_raw_help(
                    &mut lines,
                    width,
                    "up / down",
                    "Select; at the end load the next page",
                );
                push_raw_help(
                    &mut lines,
                    width,
                    "ctrl+space",
                    "Show or hide selected series (maximum six)",
                );
                push_raw_help(
                    &mut lines,
                    width,
                    "shift+left / shift+right",
                    "Inspect focused chart",
                );
                push_raw_help(
                    &mut lines,
                    width,
                    "enter",
                    "Open exact sample or aggregate bucket in Oplog",
                );
            }
        }
        push_raw_help(&mut lines, width, "ctrl+s", "Switch Agents / Server source");
        push_raw_help(
            &mut lines,
            width,
            "ctrl+t",
            "Choose preset or absolute time range",
        );
        push_raw_help(&mut lines, width, "ctrl+l", "Toggle Live / Paused");
        push_raw_help(&mut lines, width, "ctrl+r", "Refresh now");
        push_raw_help(&mut lines, width, "ctrl+x v o", "Open Overview");
    } else if app.ops.active() == OpsDestination::Configuration {
        push_section_break(&mut lines, width, "Configuration");
        push_raw_help(
            &mut lines,
            width,
            "left / right",
            "Environment, Account, or Server",
        );
        push_raw_help(
            &mut lines,
            width,
            "tab / shift+tab",
            "Focus kinds, items, or details",
        );
        push_raw_help(&mut lines, width, "up / down", "Select or scroll details");
        push_raw_help(&mut lines, width, "ctrl+r", "Reload selected kind");
        push_raw_help(&mut lines, width, "ctrl+x v o", "Open Overview");
    } else if app.ops_agents_focused() {
        push_action_section_for_app(
            &mut lines,
            app,
            width,
            "Agents Overview",
            &[
                ActionId::RefreshAgents,
                ActionId::OpenAgentDatasetFilter,
                ActionId::IncludeAgentMatches,
                ActionId::ExcludeAgentMatches,
                ActionId::LoadMoreAgents,
                ActionId::ToggleAgentAutoRefresh,
                ActionId::ToggleAgentDetails,
                ActionId::CycleAgentMode,
                ActionId::ToggleAgentStream,
                ActionId::OpenAgentScopeInOplog,
                ActionId::OpenAgentScopeInMetrics,
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
    } else if app.ops_document_focused() {
        push_action_section_for_app(
            &mut lines,
            app,
            width,
            "Agent",
            &[
                ActionId::ToggleAgentStream,
                ActionId::OpenAgentScopeInOplog,
                ActionId::OpenAgentScopeInMetrics,
            ],
        );
        push_raw_help(&mut lines, width, "up / down", "Scroll Agent details");
        push_raw_help(&mut lines, width, "esc", "Return to Agents");
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
    ids: &[ActionId],
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
    action: &ActionDefinition,
    width: usize,
) {
    let shortcut = action.shortcut.unwrap_or("-");
    let suffix = match app.action_availability(action) {
        ActionAvailability::Available => String::new(),
        ActionAvailability::Unavailable(reason) => format!(" ({reason})"),
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

fn leader_key_hints(app: &TuiApp) -> Vec<KeyHint<'static>> {
    let entries: &[(&str, &str)] = match app.leader.group {
        None => &[
            ("w", "Workspaces"),
            ("v", "Views"),
            ("l", "Layout"),
            ("a", "Commands"),
            ("j", "Jobs"),
            ("e", "Context"),
        ],
        Some(LeaderGroup::Workspaces) => {
            &[("n", "Next"), ("p", "Previous"), ("d", "Dev"), ("o", "Ops")]
        }
        Some(LeaderGroup::Views) if app.active_workspace == Workspace::Dev => {
            &[("b", "Build & Deploy"), ("s", "Server"), ("r", "REPL")]
        }
        Some(LeaderGroup::Views) => &[
            ("o", "Overview"),
            ("a", "Agents"),
            ("l", "Oplog"),
            ("s", "Stream"),
            ("m", "Metrics"),
            ("d", "Deployments"),
            ("c", "Configuration"),
            ("n/p", "Next/Previous"),
        ],
    };
    entries
        .iter()
        .map(|(key, label)| KeyHint { key, label })
        .collect()
}

fn render_output_view(frame: &mut Frame<'_>, area: Rect, app: &TuiApp) {
    frame.render_widget(Paragraph::new("").style(surface_style()), area);
    let [summary_area, output_area] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .areas(area);

    let run = app.command_run.as_ref();
    let summary = match run {
        Some(run) => command_status_line(run, terminal_input_label(app, NestedCliTarget::Command)),
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
        .map(|_| terminal_content_area(output_area))
        .unwrap_or_else(|| terminal_content_area(output_area));
    frame.render_widget(
        Paragraph::new(output_lines)
            .style(surface_style())
            .wrap(Wrap { trim: false }),
        output_content,
    );
    if let Some(run) = run {
        if let Some(slot) = render_output_scrollbar(frame, output_area, &run.terminal) {
            app.geometry.add_region(
                RegionKind::Scrollbar(ScrollbarTarget::Dev(DevView::BuildDeploy)),
                slot.area,
            );
        }
        render_terminal_cursor(
            frame,
            output_content,
            &run.terminal,
            app.terminal_input_owns(NestedCliTarget::Command),
        );
    }
}

fn render_output_scrollbar(
    frame: &mut Frame<'_>,
    area: Rect,
    output: &TerminalScreen,
) -> Option<PaneScrollbarSlot> {
    let visible_height = area.height as usize;
    let total = output.total_lines();
    if output.max_scrollback() == 0 || area.width == 0 || visible_height == 0 {
        return None;
    }

    let position = output_scrollbar_position(total, visible_height, output.scrollback());
    let mut state = ScrollbarState::new(total)
        .position(position)
        .viewport_content_length(visible_height);

    let slot = PaneScrollbarSlot {
        area: Rect::new(area.right().saturating_sub(1), area.y, 1, area.height),
    };
    render_scrollbar(frame, slot, &mut state, &theme());
    Some(slot)
}

fn terminal_content_area(area: Rect) -> Rect {
    Rect {
        width: area
            .width
            .saturating_sub(u16::from(area.width > 0 && area.height > 0)),
        ..area
    }
}

fn render_terminal_cursor(
    frame: &mut Frame<'_>,
    area: Rect,
    screen: &TerminalScreen,
    active: bool,
) {
    if active
        && let Some(cursor) = screen.cursor_position(Position::new(area.x, area.y))
        && cursor.x < area.right()
        && cursor.y < area.bottom()
    {
        frame.set_cursor_position(cursor);
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

fn scrollbar_value_from_pointer(area: Rect, y: u16, maximum: usize) -> usize {
    if maximum == 0 || area.height <= 1 {
        return 0;
    }
    let start = area.y.saturating_add(u16::from(area.height > 2));
    let end = area.bottom().saturating_sub(1 + u16::from(area.height > 2));
    if y <= start {
        return 0;
    }
    if y >= end {
        return maximum;
    }
    let length = end.saturating_sub(start) as usize;
    usize::from(y.saturating_sub(start)).saturating_mul(maximum) / length.max(1)
}

fn command_status_line(run: &CommandRun, input: &'static str) -> Line<'static> {
    status_summary_line(
        command_status_kind(run.status),
        command_status_display(run),
        &[
            ("input", input.to_string()),
            ("job", run.kind.title().to_string()),
            ("yes", flag_state(run.options.yes).to_string()),
            ("reset", flag_state(run.options.reset).to_string()),
            ("command", command_display(&run.args)),
            ("context", run.launch_context.clone()),
        ],
    )
}

fn server_status_line(server: &LocalServerService, input: &'static str) -> Line<'static> {
    let run = &server.run;
    let clean = run.clean;
    let ports = run.bound_ports.as_ref().map_or_else(
        || {
            if run.ports_file.is_some() {
                "ports: awaiting file".to_string()
            } else {
                "ports: not observed".to_string()
            }
        },
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
            ("input", input.to_string()),
            ("session", "server".to_string()),
            ("clean", flag_state(clean).to_string()),
            ("ports", ports.trim_start_matches("ports: ").to_string()),
            ("command", command_display(&run.args)),
            ("context", run.launch_context.clone()),
        ],
    )
}

fn repl_status_line(repl: &ReplState, input: &'static str) -> Line<'static> {
    let run = &repl.run;
    status_summary_line(
        repl_status_kind(run.status),
        run.status.title().to_string(),
        &[
            ("input", input.to_string()),
            ("session", "repl".to_string()),
            ("command", command_display(&run.args)),
            ("context", run.launch_context.clone()),
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

fn terminal_input_label(app: &TuiApp, target: NestedCliTarget) -> &'static str {
    if app.mode == InteractionMode::TerminalScrollback && app.dev_focus == target_dev_view(target) {
        "TUI · scrollback"
    } else if app.terminal_input_owns(target) {
        "CLI"
    } else if app.focused_nested_cli_target() == Some(target) {
        "TUI · enter → CLI"
    } else {
        "TUI"
    }
}

fn target_dev_view(target: NestedCliTarget) -> DevView {
    match target {
        NestedCliTarget::Command => DevView::BuildDeploy,
        NestedCliTarget::Server => DevView::Server,
        NestedCliTarget::Repl => DevView::Repl,
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
            format!("Current: {}", app.selected_target.context.short_label()),
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
        .unwrap_or_else(|| "Switching context will stop context-bound work.".to_string());
    let (status_kind, status) = if app.context_switcher.waiting_for_dev_stop {
        (
            NoticeKind::Loading,
            "Waiting for work to stop; enter again after five seconds to force stop.",
        )
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
        .popup_line(&theme()),
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
                    label: if app.context_switcher.waiting_for_dev_stop {
                        "Force stop when ready"
                    } else {
                        "Confirm"
                    },
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

fn render_metrics_range_form(frame: &mut Frame<'_>, app: &TuiApp) {
    let Some(form) = app.metrics_range_form.as_ref() else {
        return;
    };
    let area = content_popup_rect(frame.area(), 72, 16);
    let content = OverlayFrame::content_area(area);
    let row_width = content.width as usize;
    let label_width = 10;
    let mut lines = vec![
        ActionButton {
            label: "Apply",
            selected: form.selected_field() == MetricsRangeFormField::Apply,
        }
        .line(&theme()),
        Line::default(),
    ];
    lines.extend(
        wrap_styled_line(
            Notice {
                kind: NoticeKind::Info,
                message: "Presets end at now. Absolute RFC3339 ranges pause Live refresh.",
            }
            .popup_line(&theme()),
            row_width,
        )
        .into_iter()
        .map(|line| line.alignment(Alignment::Center)),
    );
    lines.push(Line::default());

    let preset_value = MetricsRange::PRESETS
        .into_iter()
        .map(|preset| {
            let label = preset.label();
            if preset == form.preset {
                format!("[{label}]")
            } else {
                label
            }
        })
        .collect::<Vec<_>>()
        .join("  ");
    let preset_label = if form.custom {
        format!("{preset_value}  · custom active")
    } else {
        preset_value
    };
    lines.push(
        ReviewFieldRow {
            label: "Preset",
            value: &preset_label,
            label_width,
            selected: form.selected_field() == MetricsRangeFormField::Preset,
            editing: false,
        }
        .line(row_width, &theme()),
    );

    let mut cursor = None;
    for (field, label, editor) in [
        (MetricsRangeFormField::From, "From UTC", &form.from),
        (MetricsRangeFormField::To, "To UTC", &form.to),
    ] {
        let selected = form.selected_field() == field;
        let editing = selected && form.editing;
        let template = ReviewFieldRow {
            label,
            value: &editor.draft,
            label_width,
            selected,
            editing,
        };
        let value_offset = template.value_offset(row_width);
        let row_y = content.y.saturating_add(lines.len() as u16);
        let (value, cursor_column) = if editing {
            editor.viewport(row_width.saturating_sub(value_offset))
        } else {
            (editor.draft.clone(), 0)
        };
        if editing {
            cursor = Some(Position::new(
                content
                    .x
                    .saturating_add(value_offset as u16)
                    .saturating_add(cursor_column as u16),
                row_y,
            ));
        }
        lines.push(
            ReviewFieldRow {
                label,
                value: &value,
                label_width,
                selected,
                editing,
            }
            .line(row_width, &theme()),
        );
    }

    lines.push(Line::default());
    if let Some(error) = &form.error {
        lines.extend(
            wrap_styled_line(
                Notice {
                    kind: NoticeKind::Warning,
                    message: error,
                }
                .popup_line(&theme()),
                row_width,
            )
            .into_iter()
            .map(|line| line.alignment(Alignment::Center)),
        );
    }
    let normal_hints = [
        KeyHint {
            key: "↑/↓",
            label: "Field",
        },
        KeyHint {
            key: "←/→",
            label: "Preset",
        },
        KeyHint {
            key: "enter",
            label: "Apply / edit",
        },
        KeyHint {
            key: "esc",
            label: "Cancel",
        },
    ];
    let editing_hints = [
        KeyHint {
            key: "type / arrows",
            label: "Edit",
        },
        KeyHint {
            key: "enter",
            label: "Done",
        },
        KeyHint {
            key: "esc",
            label: "Cancel edit",
        },
    ];
    let footer = shortcut_line(
        if form.editing {
            &editing_hints
        } else {
            &normal_hints
        },
        &theme(),
        Alignment::Center,
    );
    OverlayFrame {
        title: Some("Metrics time range"),
    }
    .render_aligned(
        frame,
        area,
        overlay_lines_with_footer(lines, footer, content.height),
        Alignment::Left,
        &theme(),
    );
    if let Some(cursor) = cursor
        && cursor.x < content.right()
        && cursor.y < content.bottom()
    {
        frame.set_cursor_position(cursor);
    }
}

fn render_command_form(frame: &mut Frame<'_>, app: &TuiApp) {
    let Some(form) = app.command_form.as_ref() else {
        return;
    };
    let fields = command_form_rows(form);
    let preliminary_area = content_popup_rect(frame.area(), 62, frame.area().height);
    let preliminary_content = OverlayFrame::content_area(preliminary_area);
    let notice = wrap_styled_line(
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
        .popup_line(&theme()),
        preliminary_content.width as usize,
    );
    let desired_content_height = fields.len() as u16 + 5 + notice.len() as u16;
    let area = content_popup_rect(frame.area(), 62, desired_content_height.saturating_add(4));
    let content = OverlayFrame::content_area(area);
    let mut lines = vec![
        ActionButton {
            label: "Run",
            selected: form.selected_field() == ReviewField::Run,
        }
        .line(&theme()),
        Line::default(),
    ];
    lines.extend(notice);
    lines.push(Line::default());
    let label_width = 18;
    let row_width = content.width as usize;
    let mut cursor = None;
    for row in fields {
        let selected = row.field == form.selected_field();
        let editing = selected && form.editing.is_some();
        let template = ReviewFieldRow {
            label: row.label,
            value: &row.value,
            label_width,
            selected,
            editing,
        };
        let value_offset = template.value_offset(row_width);
        let row_y = content.y.saturating_add(lines.len() as u16);
        let (value, cursor_column) = if editing {
            form.editing
                .as_ref()
                .expect("active field editor")
                .viewport(row_width.saturating_sub(value_offset))
        } else {
            (row.value, 0)
        };
        if editing {
            cursor = Some(Position::new(
                content
                    .x
                    .saturating_add(value_offset as u16)
                    .saturating_add(cursor_column as u16),
                row_y,
            ));
        }
        lines.push(
            ReviewFieldRow {
                label: row.label,
                value: &value,
                label_width,
                selected,
                editing,
            }
            .line(row_width, &theme()),
        );
    }
    lines.push(Line::default());
    let normal_hints = [
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
    ];
    let editing_hints = [
        KeyHint {
            key: "type / arrows",
            label: "Edit",
        },
        KeyHint {
            key: "enter",
            label: "Apply",
        },
        KeyHint {
            key: "esc",
            label: "Cancel edit",
        },
    ];
    let footer = shortcut_line(
        if form.editing.is_some() {
            &editing_hints
        } else {
            &normal_hints
        },
        &theme(),
        Alignment::Center,
    );
    let title = format!("Review {}", form.kind.title());
    OverlayFrame {
        title: Some(&title),
    }
    .render_aligned(
        frame,
        area,
        overlay_lines_with_footer(lines, footer, content.height),
        Alignment::Left,
        &theme(),
    );
    if let Some(cursor) = cursor
        && cursor.x < content.right()
        && cursor.y < content.bottom()
    {
        frame.set_cursor_position(cursor);
    }
}

struct ReviewRenderRow {
    field: ReviewField,
    label: &'static str,
    value: String,
}

fn toggle_value(enabled: bool) -> String {
    if enabled {
        "[x]".to_string()
    } else {
        "[ ]".to_string()
    }
}

fn shown_value(value: &str) -> String {
    if value.is_empty() {
        "<default>".to_string()
    } else {
        value.to_string()
    }
}

fn command_form_rows(form: &CommandReviewForm) -> Vec<ReviewRenderRow> {
    form.fields()
        .iter()
        .copied()
        .filter(|field| *field != ReviewField::Run)
        .map(|field| {
            let (label, value) = match field {
                ReviewField::Yes => ("--yes", toggle_value(form.values.yes)),
                ReviewField::SkipCheck => ("skip check", toggle_value(form.values.skip_check)),
                ReviewField::ForceBuild => ("force build", toggle_value(form.values.force_build)),
                ReviewField::FullDiff => ("full diff", toggle_value(form.values.full_diff)),
                ReviewField::RollbackVersion => (
                    "rollback version",
                    shown_value(&form.values.rollback_version),
                ),
                ReviewField::RollbackRevision => (
                    "rollback revision",
                    shown_value(&form.values.rollback_revision),
                ),
                ReviewField::PostDeploy => {
                    ("post deploy", form.values.post_deploy.label().to_string())
                }
                _ => unreachable!("command review field"),
            };
            ReviewRenderRow {
                field,
                label,
                value,
            }
        })
        .collect()
}

fn render_session_form(frame: &mut Frame<'_>, app: &TuiApp) {
    let Some(form) = app.session_form.as_ref() else {
        return;
    };
    let (title, notice, error) = match form {
        SessionReviewForm::Server { restart, error, .. } => (
            if *restart {
                "Review Server restart"
            } else {
                "Review Server start"
            },
            "Starts the local server in a nested CLI PTY.",
            error.as_deref(),
        ),
        SessionReviewForm::Repl { restart, .. } => (
            if *restart {
                "Review REPL restart"
            } else {
                "Review REPL start"
            },
            "Starts the interactive REPL in a persistent nested CLI PTY.",
            None,
        ),
    };
    let fields = session_form_rows(form);
    let preliminary_area = content_popup_rect(frame.area(), 68, frame.area().height);
    let preliminary_content = OverlayFrame::content_area(preliminary_area);
    let notice = wrap_styled_line(
        Notice {
            kind: NoticeKind::Info,
            message: notice,
        }
        .popup_line(&theme()),
        preliminary_content.width as usize,
    );
    let error = error.map(|message| {
        wrap_styled_line(
            Notice {
                kind: NoticeKind::Error,
                message,
            }
            .popup_line(&theme()),
            preliminary_content.width as usize,
        )
    });
    let error_height = error
        .as_ref()
        .map(|lines| lines.len() as u16 + 1)
        .unwrap_or_default();
    let desired_content_height = fields.len() as u16 + 5 + notice.len() as u16 + error_height;
    let area = content_popup_rect(frame.area(), 68, desired_content_height.saturating_add(4));
    let content = OverlayFrame::content_area(area);
    let mut lines = vec![
        ActionButton {
            label: "Run",
            selected: form.selected_field() == ReviewField::Run,
        }
        .line(&theme()),
        Line::default(),
    ];
    lines.extend(notice);
    lines.push(Line::default());
    if let Some(error) = error {
        lines.extend(error);
        lines.push(Line::default());
    }
    let label_width = 22;
    let row_width = content.width as usize;
    let mut cursor = None;
    for row in fields {
        let selected = row.field == form.selected_field();
        let editing = selected && form.editing().is_some();
        let template = ReviewFieldRow {
            label: row.label,
            value: &row.value,
            label_width,
            selected,
            editing,
        };
        let value_offset = template.value_offset(row_width);
        let row_y = content.y.saturating_add(lines.len() as u16);
        let (value, cursor_column) = if editing {
            form.editing()
                .expect("active field editor")
                .viewport(row_width.saturating_sub(value_offset))
        } else {
            (row.value, 0)
        };
        if editing {
            cursor = Some(Position::new(
                content
                    .x
                    .saturating_add(value_offset as u16)
                    .saturating_add(cursor_column as u16),
                row_y,
            ));
        }
        lines.push(
            ReviewFieldRow {
                label: row.label,
                value: &value,
                label_width,
                selected,
                editing,
            }
            .line(row_width, &theme()),
        );
    }
    lines.push(Line::default());
    let normal_hints = [
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
    ];
    let editing_hints = [
        KeyHint {
            key: "type / arrows",
            label: "Edit",
        },
        KeyHint {
            key: "enter",
            label: "Apply",
        },
        KeyHint {
            key: "esc",
            label: "Cancel edit",
        },
    ];
    let footer = shortcut_line(
        if form.editing().is_some() {
            &editing_hints
        } else {
            &normal_hints
        },
        &theme(),
        Alignment::Center,
    );
    OverlayFrame { title: Some(title) }.render_aligned(
        frame,
        area,
        overlay_lines_with_footer(lines, footer, content.height),
        Alignment::Left,
        &theme(),
    );
    if let Some(cursor) = cursor
        && cursor.x < content.right()
        && cursor.y < content.bottom()
    {
        frame.set_cursor_position(cursor);
    }
}

fn session_form_rows(form: &SessionReviewForm) -> Vec<ReviewRenderRow> {
    form.fields()
        .iter()
        .copied()
        .filter(|field| *field != ReviewField::Run)
        .map(|field| {
            let (label, value) = match form {
                SessionReviewForm::Server { values, .. } => match field {
                    ReviewField::Clean => ("clean", toggle_value(values.clean)),
                    ReviewField::Yes => ("--yes", toggle_value(values.yes)),
                    ReviewField::SystemMemory => {
                        ("system memory", shown_value(&values.system_memory_override))
                    }
                    ReviewField::RouterAddress => {
                        ("router address", shown_value(&values.router_address))
                    }
                    ReviewField::RouterPort => ("router port", shown_value(&values.router_port)),
                    ReviewField::CustomRequestPort => (
                        "custom request port",
                        shown_value(&values.custom_request_port),
                    ),
                    ReviewField::McpPort => ("MCP port", shown_value(&values.mcp_port)),
                    ReviewField::PortsFile => ("ports file", shown_value(&values.ports_file)),
                    ReviewField::DataDirectory => ("data directory", shown_value(&values.data_dir)),
                    ReviewField::AgentFilesystemRoot => (
                        "agent filesystem root",
                        shown_value(&values.agent_filesystem_root),
                    ),
                    _ => unreachable!("server review field"),
                },
                SessionReviewForm::Repl { values, .. } => match field {
                    ReviewField::Yes => ("--yes", toggle_value(values.yes)),
                    ReviewField::Language => ("language", shown_value(&values.language)),
                    ReviewField::StreamLogs => ("stream logs", toggle_value(values.stream_logs)),
                    ReviewField::AutomaticImports => {
                        ("automatic imports", toggle_value(values.auto_imports))
                    }
                    ReviewField::PostDeploy => {
                        ("post deploy", values.post_deploy.label().to_string())
                    }
                    _ => unreachable!("REPL review field"),
                },
            };
            ReviewRenderRow {
                field,
                label,
                value,
            }
        })
        .collect()
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
    let message = format!("Still running: {}.", active.join(", "));
    let mut choices = ActionButton {
        label: "Return",
        selected: app.quit_choice == QuitChoice::Return,
    }
    .spans(&theme());
    choices.push(Span::raw("   "));
    choices.extend(
        ActionButton {
            label: "Force quit",
            selected: app.quit_choice == QuitChoice::ForceQuit,
        }
        .spans(&theme()),
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
            message: "Force quit stops all TUI-owned processes.",
        }
        .popup_line(&theme()),
        Line::default(),
        Line::from(choices).alignment(Alignment::Center),
        Line::default(),
        shortcut_line(
            &[
                KeyHint {
                    key: "←/→ / tab",
                    label: "Choose",
                },
                KeyHint {
                    key: "enter",
                    label: "Confirm",
                },
                KeyHint {
                    key: "esc",
                    label: "Return",
                },
            ],
            &theme(),
            Alignment::Center,
        ),
    ];
    let area = content_popup_rect(frame.area(), 58, lines.len() as u16 + 4);
    OverlayFrame {
        title: Some("Quit with active work"),
    }
    .render_aligned(frame, area, lines, Alignment::Left, &theme());
}

fn render_palette(frame: &mut Frame<'_>, app: &TuiApp) {
    let matches = app.palette_matches();
    let area = palette_popup_rect(frame.area(), app);
    let content = OverlayFrame::content_area(area);
    let scope_width = content.width / 2;
    app.geometry.add_region(
        RegionKind::PaletteAll,
        Rect::new(content.x, content.y, scope_width, content.height.min(1)),
    );
    app.geometry.add_region(
        RegionKind::PaletteContext,
        Rect::new(
            content.x.saturating_add(scope_width),
            content.y,
            content.width.saturating_sub(scope_width),
            content.height.min(1),
        ),
    );
    let action_capacity = content.height.saturating_sub(5).max(1) as usize;
    let overflow = matches.len() > action_capacity;
    let content_width = content.width.saturating_sub(u16::from(overflow)) as usize;
    let selected = app.palette.selected.min(matches.len().saturating_sub(1));
    let visible_count = action_capacity.min(matches.len());
    let start = selected.saturating_sub(visible_count.saturating_sub(1));
    let visible_matches = matches
        .iter()
        .skip(start)
        .take(visible_count)
        .cloned()
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
    let search = SearchInput {
        query: &app.palette.query,
        placeholder: "type to search commands",
    };
    let mut lines = vec![
        scope_line,
        search.line(content_width, &theme()),
        Line::default(),
    ];

    if matches.is_empty() {
        lines.push(
            Notice {
                kind: NoticeKind::Empty,
                message: "No matching commands",
            }
            .line(&theme()),
        );
    } else {
        for (index, matched) in visible_matches.iter().enumerate() {
            let action = &matched.action;
            let availability = app.action_availability(action);
            let description = palette_action_description(app, action);
            let visual = theme();
            lines.push(
                CommandRow {
                    category: palette_category_marker(
                        action.category,
                        matched.category_matched,
                        &visual,
                    ),
                    risk: palette_risk_marker(action.risk(), &visual),
                    label: action.label,
                    shortcut: action.shortcut,
                    description: &description,
                    label_matches: &matched.label_matches,
                    shortcut_matches: &matched.shortcut_matches,
                    description_matches: &matched.description_matches,
                    selected: start + index == selected,
                    unavailable: availability.is_unavailable(),
                }
                .line(content_width, &visual),
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
                key: "←/→",
                label: "All / Context",
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
            matches.len(),
            selected,
        );
    }
    if content_width > 0 && content.height > 1 {
        frame.set_cursor_position(Position::new(
            content
                .x
                .saturating_add(search.cursor_column(content_width) as u16),
            content.y.saturating_add(1),
        ));
    }
}

fn palette_popup_rect(area: Rect, app: &TuiApp) -> Rect {
    const MINIMUM_WIDTH: usize = 64;
    const MINIMUM_HEIGHT: usize = 10;
    const FRAME_WIDTH: usize = 4;
    const FRAME_HEIGHT: usize = 9;
    const COMMAND_PREFIX_WIDTH: usize = 6;
    const MINIMUM_LABEL_WIDTH: usize = 16;
    const MAXIMUM_LABEL_WIDTH: usize = 28;
    const SHORTCUT_WIDTH: usize = 16;
    const COLUMN_GAP_WIDTH: usize = 1;
    const MAXIMUM_DESCRIPTION_WIDTH: usize = 48;

    let actions = palette_actions(PaletteScope::All, app);
    let desired_content_width = actions
        .iter()
        .map(|action| {
            let label_width = UnicodeWidthStr::width(action.label)
                .clamp(MINIMUM_LABEL_WIDTH, MAXIMUM_LABEL_WIDTH);
            let shortcut_width = usize::from(action.shortcut.is_some()) * SHORTCUT_WIDTH;
            let gap_width = COLUMN_GAP_WIDTH * (1 + usize::from(action.shortcut.is_some()));
            let description_width =
                UnicodeWidthStr::width(palette_action_description(app, action).as_str())
                    .min(MAXIMUM_DESCRIPTION_WIDTH);
            COMMAND_PREFIX_WIDTH
                + gap_width
                + (label_width * 3).max(MAXIMUM_LABEL_WIDTH + shortcut_width + description_width)
        })
        .max()
        .unwrap_or_default();
    let desired_width = (desired_content_width + FRAME_WIDTH).max(MINIMUM_WIDTH);
    let desired_height = (actions.len().max(1) + FRAME_HEIGHT).max(MINIMUM_HEIGHT);

    content_popup_rect(
        area,
        adaptive_palette_dimension(area.width, desired_width, MINIMUM_WIDTH),
        adaptive_palette_dimension(area.height, desired_height, MINIMUM_HEIGHT),
    )
}

fn adaptive_palette_dimension(available: u16, desired: usize, minimum: usize) -> u16 {
    if available <= 2 {
        return available;
    }
    let available = usize::from(available);
    let margin_cap = available.saturating_sub(2);
    let percentage_cap = available.saturating_mul(85) / 100;
    let cap = percentage_cap.max(minimum.min(margin_cap)).min(margin_cap);
    desired.max(minimum.min(cap)).min(cap) as u16
}

fn palette_category_marker(
    category: ActionCategory,
    matched: bool,
    visual: &TuiVisualStyle,
) -> CommandMarker<'static> {
    let (glyph, color) = match category {
        ActionCategory::GoTo => ("→", visual.info),
        ActionCategory::Run => ("▶", visual.accent),
        ActionCategory::View => ("▦", visual.text_secondary),
        ActionCategory::Explore => ("⌕", visual.success),
        ActionCategory::Tui => ("•", visual.text_muted),
    };
    CommandMarker {
        glyph,
        style: Style::default().fg(color),
        matched,
    }
}

fn palette_risk_marker(
    risk: ActionRisk,
    visual: &TuiVisualStyle,
) -> Option<CommandMarker<'static>> {
    let color = match risk {
        ActionRisk::Disruptive => visual.marker,
        ActionRisk::Destructive => visual.error,
        ActionRisk::ReadOnly | ActionRisk::RoutineWrite => return None,
    };
    Some(CommandMarker {
        glyph: "!",
        style: Style::default().fg(color),
        matched: false,
    })
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
enum ActionId {
    Plan,
    Build,
    Deploy,
    Clean,
    SelectDevBuildDeploy,
    SelectDevServer,
    SelectDevRepl,
    StartServer,
    StopServer,
    ForceStopServer,
    RestartServer,
    FlipPanePlacement,
    ToggleBuildDeployView,
    ToggleServerView,
    ToggleReplView,
    MoveCurrentDevViewToMain,
    RefreshAgents,
    OpenAgentDatasetFilter,
    OpenAgentColumns,
    IncludeAgentMatches,
    ExcludeAgentMatches,
    LoadMoreAgents,
    ToggleAgentAutoRefresh,
    CycleAgentMode,
    ToggleAgentDetails,
    ToggleAgentStream,
    OpenAgentScopeInOplog,
    OpenAgentScopeInMetrics,
    OpenAgentSsh,
    UseEnvironmentMetricsScope,
    NextOpsDestination,
    PreviousOpsDestination,
    CycleAgentFindScope,
    SelectDev,
    SelectOps,
    SelectOpsOverview,
    SelectOpsAgents,
    SelectOpsOplog,
    SelectOpsStream,
    SelectOpsMetrics,
    SelectOpsDeployments,
    SelectOpsConfiguration,
    StartRepl,
    StopRepl,
    ForceStopRepl,
    RestartRepl,
    OpenPalette,
    OpenContextPicker,
    ShowHelp,
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActionCategory {
    GoTo,
    Run,
    View,
    Explore,
    Tui,
}

impl ActionCategory {
    fn label(self) -> &'static str {
        match self {
            Self::GoTo => "go to",
            Self::Run => "run",
            Self::View => "view",
            Self::Explore => "explore",
            Self::Tui => "tui",
        }
    }

    fn order(self) -> u8 {
        match self {
            Self::GoTo => 0,
            Self::Run => 1,
            Self::View => 2,
            Self::Explore => 3,
            Self::Tui => 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PalettePlacement {
    Context(Workspace),
    ContextAndGlobal(Workspace),
    Global,
}

impl PalettePlacement {
    fn visible_in(self, scope: PaletteScope, workspace: Workspace) -> bool {
        match (scope, self) {
            (PaletteScope::Context, Self::Context(context))
            | (PaletteScope::Context, Self::ContextAndGlobal(context)) => context == workspace,
            (PaletteScope::Context, Self::Global) => false,
            (PaletteScope::All, Self::Context(context)) => context == workspace,
            (PaletteScope::All, Self::ContextAndGlobal(_) | Self::Global) => true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActionExecution {
    Internal,
    Direct,
    NestedCli,
    ViewNavigation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActionRisk {
    ReadOnly,
    RoutineWrite,
    Disruptive,
    Destructive,
}

#[derive(Debug, Clone, Copy)]
struct ActionDefinition {
    id: ActionId,
    label: &'static str,
    description: &'static str,
    shortcut: Option<&'static str>,
    category: ActionCategory,
    palette_placement: PalettePlacement,
    #[allow(dead_code)]
    execution_kind: ActionExecution,
    palette_visible: bool,
}

impl ActionDefinition {
    #[allow(dead_code)]
    fn leader_path(self) -> Option<&'static str> {
        match self.id {
            ActionId::SelectDev | ActionId::SelectOps => Some("ctrl+x w"),
            ActionId::NextOpsDestination
            | ActionId::PreviousOpsDestination
            | ActionId::SelectOpsOverview
            | ActionId::SelectOpsAgents
            | ActionId::SelectOpsOplog
            | ActionId::SelectOpsStream
            | ActionId::SelectOpsMetrics
            | ActionId::SelectOpsDeployments
            | ActionId::SelectOpsConfiguration
            | ActionId::SelectDevBuildDeploy
            | ActionId::SelectDevServer
            | ActionId::SelectDevRepl => Some("ctrl+x v"),
            ActionId::FlipPanePlacement
            | ActionId::ToggleBuildDeployView
            | ActionId::ToggleServerView
            | ActionId::ToggleReplView
            | ActionId::MoveCurrentDevViewToMain => Some("ctrl+x l"),
            ActionId::OpenContextPicker => Some("ctrl+x e"),
            ActionId::OpenPalette | ActionId::ShowHelp | ActionId::Quit => None,
            _ => Some("ctrl+x a"),
        }
    }

    fn risk(self) -> ActionRisk {
        match self.id {
            ActionId::Clean => ActionRisk::Destructive,
            ActionId::Deploy
            | ActionId::StopServer
            | ActionId::ForceStopServer
            | ActionId::RestartServer
            | ActionId::StopRepl
            | ActionId::ForceStopRepl => ActionRisk::Disruptive,
            ActionId::Build
            | ActionId::StartServer
            | ActionId::StartRepl
            | ActionId::RestartRepl => ActionRisk::RoutineWrite,
            _ => ActionRisk::ReadOnly,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActionAvailability {
    Available,
    Unavailable(&'static str),
}

impl ActionAvailability {
    fn is_unavailable(self) -> bool {
        matches!(self, Self::Unavailable(_))
    }
}

const ACTIONS: &[ActionDefinition] = &[
    ActionDefinition {
        id: ActionId::Plan,
        label: "Plan",
        description: "Preview deployment changes",
        shortcut: None,
        category: ActionCategory::Run,
        palette_placement: PalettePlacement::Context(Workspace::Dev),
        execution_kind: ActionExecution::NestedCli,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::Build,
        label: "Build",
        description: "Build the application",
        shortcut: None,
        category: ActionCategory::Run,
        palette_placement: PalettePlacement::Context(Workspace::Dev),
        execution_kind: ActionExecution::NestedCli,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::Deploy,
        label: "Deploy",
        description: "Deploy the application environment",
        shortcut: None,
        category: ActionCategory::Run,
        palette_placement: PalettePlacement::Context(Workspace::Dev),
        execution_kind: ActionExecution::NestedCli,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::Clean,
        label: "Clean",
        description: "Clean application build output",
        shortcut: None,
        category: ActionCategory::Run,
        palette_placement: PalettePlacement::Context(Workspace::Dev),
        execution_kind: ActionExecution::NestedCli,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::SelectDevBuildDeploy,
        label: "Dev / Build & Deploy",
        description: "Show and focus",
        shortcut: None,
        category: ActionCategory::GoTo,
        palette_placement: PalettePlacement::ContextAndGlobal(Workspace::Dev),
        execution_kind: ActionExecution::ViewNavigation,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::SelectDevServer,
        label: "Dev / Server",
        description: "Show and focus",
        shortcut: None,
        category: ActionCategory::GoTo,
        palette_placement: PalettePlacement::ContextAndGlobal(Workspace::Dev),
        execution_kind: ActionExecution::ViewNavigation,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::SelectDevRepl,
        label: "Dev / REPL",
        description: "Show and focus",
        shortcut: None,
        category: ActionCategory::GoTo,
        palette_placement: PalettePlacement::ContextAndGlobal(Workspace::Dev),
        execution_kind: ActionExecution::ViewNavigation,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::StartServer,
        label: "Start Server",
        description: "Review and start the local server",
        shortcut: None,
        category: ActionCategory::Run,
        palette_placement: PalettePlacement::Context(Workspace::Dev),
        execution_kind: ActionExecution::NestedCli,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::StopServer,
        label: "Stop Server",
        description: "Request a graceful local-server stop",
        shortcut: None,
        category: ActionCategory::Run,
        palette_placement: PalettePlacement::Context(Workspace::Dev),
        execution_kind: ActionExecution::Internal,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::ForceStopServer,
        label: "Force Stop Server",
        description: "Kill the local server after its graceful-stop window",
        shortcut: None,
        category: ActionCategory::Run,
        palette_placement: PalettePlacement::Context(Workspace::Dev),
        execution_kind: ActionExecution::Internal,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::RestartServer,
        label: "Restart Server",
        description: "Review settings and restart the local server",
        shortcut: None,
        category: ActionCategory::Run,
        palette_placement: PalettePlacement::Context(Workspace::Dev),
        execution_kind: ActionExecution::NestedCli,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::FlipPanePlacement,
        label: "Flip Pane Placement",
        description: "Place secondary panes on the right or bottom",
        shortcut: None,
        category: ActionCategory::View,
        palette_placement: PalettePlacement::Context(Workspace::Dev),
        execution_kind: ActionExecution::Internal,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::ToggleBuildDeployView,
        label: "Toggle Build & Deploy View",
        description: "Show or hide",
        shortcut: None,
        category: ActionCategory::View,
        palette_placement: PalettePlacement::Context(Workspace::Dev),
        execution_kind: ActionExecution::Internal,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::ToggleServerView,
        label: "Toggle Server View",
        description: "Show or hide",
        shortcut: None,
        category: ActionCategory::View,
        palette_placement: PalettePlacement::Context(Workspace::Dev),
        execution_kind: ActionExecution::Internal,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::ToggleReplView,
        label: "Toggle REPL View",
        description: "Show or hide",
        shortcut: None,
        category: ActionCategory::View,
        palette_placement: PalettePlacement::Context(Workspace::Dev),
        execution_kind: ActionExecution::Internal,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::MoveCurrentDevViewToMain,
        label: "Move Current View to Main",
        description: "Promote without hiding other views",
        shortcut: None,
        category: ActionCategory::View,
        palette_placement: PalettePlacement::Context(Workspace::Dev),
        execution_kind: ActionExecution::Internal,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::RefreshAgents,
        label: "Refresh Agents",
        description: "Refresh the Ops agent list",
        shortcut: Some("ctrl+r"),
        category: ActionCategory::Explore,
        palette_placement: PalettePlacement::Context(Workspace::Ops),
        execution_kind: ActionExecution::Direct,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::OpenAgentDatasetFilter,
        label: "Filter Agent Dataset",
        description: "Choose a server-side component or agent-type filter",
        shortcut: Some("ctrl+f"),
        category: ActionCategory::Explore,
        palette_placement: PalettePlacement::Context(Workspace::Ops),
        execution_kind: ActionExecution::Internal,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::OpenAgentColumns,
        label: "Configure Agent Columns",
        description: "Choose visible Agent collection columns",
        shortcut: None,
        category: ActionCategory::View,
        palette_placement: PalettePlacement::Context(Workspace::Ops),
        execution_kind: ActionExecution::Internal,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::IncludeAgentMatches,
        label: "Include Loaded Matches",
        description: "Include every loaded agent matching the local find",
        shortcut: Some("ctrl+a"),
        category: ActionCategory::Explore,
        palette_placement: PalettePlacement::Context(Workspace::Ops),
        execution_kind: ActionExecution::Internal,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::ExcludeAgentMatches,
        label: "Exclude Loaded Matches",
        description: "Exclude every loaded agent matching the local find",
        shortcut: Some("ctrl+n"),
        category: ActionCategory::Explore,
        palette_placement: PalettePlacement::Context(Workspace::Ops),
        execution_kind: ActionExecution::Internal,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::LoadMoreAgents,
        label: "Load Next 200 Agents",
        description: "Fetch one additional batch of up to 200 agents",
        shortcut: Some("ctrl+l"),
        category: ActionCategory::Explore,
        palette_placement: PalettePlacement::Context(Workspace::Ops),
        execution_kind: ActionExecution::Direct,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::ToggleAgentAutoRefresh,
        label: "Toggle Agent Auto Refresh",
        description: "Toggle automatic agent refresh",
        shortcut: None,
        category: ActionCategory::Explore,
        palette_placement: PalettePlacement::Context(Workspace::Ops),
        execution_kind: ActionExecution::Internal,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::CycleAgentMode,
        label: "Cycle Agent Mode",
        description: "Cycle durable, ephemeral, all",
        shortcut: None,
        category: ActionCategory::Explore,
        palette_placement: PalettePlacement::Context(Workspace::Ops),
        execution_kind: ActionExecution::Internal,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::ToggleAgentDetails,
        label: "Toggle Agent Details",
        description: "Show or hide selected agent details",
        shortcut: None,
        category: ActionCategory::View,
        palette_placement: PalettePlacement::Context(Workspace::Ops),
        execution_kind: ActionExecution::Internal,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::ToggleAgentStream,
        label: "Connect / Disconnect Agent Stream",
        description: "Use the exact Agent scope and retain events in the background",
        shortcut: Some("ctrl+k"),
        category: ActionCategory::Explore,
        palette_placement: PalettePlacement::Context(Workspace::Ops),
        execution_kind: ActionExecution::Direct,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::OpenAgentScopeInOplog,
        label: "Open Agent Scope in Oplog",
        description: "Use the selected Agent or included set",
        shortcut: None,
        category: ActionCategory::Explore,
        palette_placement: PalettePlacement::Context(Workspace::Ops),
        execution_kind: ActionExecution::ViewNavigation,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::OpenAgentScopeInMetrics,
        label: "Open Agent Scope in Metrics",
        description: "Use the selected Agent or included set",
        shortcut: None,
        category: ActionCategory::Explore,
        palette_placement: PalettePlacement::Context(Workspace::Ops),
        execution_kind: ActionExecution::ViewNavigation,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::OpenAgentSsh,
        label: "Open Agent SSH",
        description: "Open a native shell for one exact Agent",
        shortcut: None,
        category: ActionCategory::Run,
        palette_placement: PalettePlacement::Context(Workspace::Ops),
        execution_kind: ActionExecution::Direct,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::UseEnvironmentMetricsScope,
        label: "Use Environment Metrics Scope",
        description: "Clear the retained Agent scope",
        shortcut: None,
        category: ActionCategory::Explore,
        palette_placement: PalettePlacement::Context(Workspace::Ops),
        execution_kind: ActionExecution::Internal,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::NextOpsDestination,
        label: "Next Ops Destination",
        description: "Open the next Ops destination",
        shortcut: None,
        category: ActionCategory::GoTo,
        palette_placement: PalettePlacement::Context(Workspace::Ops),
        execution_kind: ActionExecution::ViewNavigation,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::PreviousOpsDestination,
        label: "Previous Ops Destination",
        description: "Open the previous Ops destination",
        shortcut: None,
        category: ActionCategory::GoTo,
        palette_placement: PalettePlacement::Context(Workspace::Ops),
        execution_kind: ActionExecution::ViewNavigation,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::CycleAgentFindScope,
        label: "Cycle Agent Find Field",
        description: "Search loaded agents by AgentID, component, or agent type",
        shortcut: None,
        category: ActionCategory::Explore,
        palette_placement: PalettePlacement::Context(Workspace::Ops),
        execution_kind: ActionExecution::Internal,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::SelectDev,
        label: "Dev",
        description: "Switch workspace",
        shortcut: None,
        category: ActionCategory::GoTo,
        palette_placement: PalettePlacement::Global,
        execution_kind: ActionExecution::ViewNavigation,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::SelectOps,
        label: "Ops",
        description: "Switch workspace",
        shortcut: None,
        category: ActionCategory::GoTo,
        palette_placement: PalettePlacement::Global,
        execution_kind: ActionExecution::ViewNavigation,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::SelectOpsOverview,
        label: "Ops / Overview",
        description: "Open status and routing",
        shortcut: None,
        category: ActionCategory::GoTo,
        palette_placement: PalettePlacement::ContextAndGlobal(Workspace::Ops),
        execution_kind: ActionExecution::ViewNavigation,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::SelectOpsAgents,
        label: "Ops / Agents",
        description: "Open collection",
        shortcut: None,
        category: ActionCategory::GoTo,
        palette_placement: PalettePlacement::ContextAndGlobal(Workspace::Ops),
        execution_kind: ActionExecution::ViewNavigation,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::SelectOpsOplog,
        label: "Ops / Oplog",
        description: "Open durable history",
        shortcut: None,
        category: ActionCategory::GoTo,
        palette_placement: PalettePlacement::ContextAndGlobal(Workspace::Ops),
        execution_kind: ActionExecution::ViewNavigation,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::SelectOpsStream,
        label: "Ops / Stream",
        description: "Open live Agent events",
        shortcut: None,
        category: ActionCategory::GoTo,
        palette_placement: PalettePlacement::ContextAndGlobal(Workspace::Ops),
        execution_kind: ActionExecution::ViewNavigation,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::SelectOpsMetrics,
        label: "Ops / Metrics",
        description: "Open dashboards and Explore",
        shortcut: None,
        category: ActionCategory::GoTo,
        palette_placement: PalettePlacement::ContextAndGlobal(Workspace::Ops),
        execution_kind: ActionExecution::ViewNavigation,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::SelectOpsDeployments,
        label: "Ops / Deployments",
        description: "Open revisions and compare",
        shortcut: None,
        category: ActionCategory::GoTo,
        palette_placement: PalettePlacement::ContextAndGlobal(Workspace::Ops),
        execution_kind: ActionExecution::ViewNavigation,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::SelectOpsConfiguration,
        label: "Ops / Configuration",
        description: "Open environment, account, and server configuration",
        shortcut: None,
        category: ActionCategory::GoTo,
        palette_placement: PalettePlacement::ContextAndGlobal(Workspace::Ops),
        execution_kind: ActionExecution::ViewNavigation,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::StartRepl,
        label: "Start REPL",
        description: "Review settings and start the application REPL",
        shortcut: None,
        category: ActionCategory::Run,
        palette_placement: PalettePlacement::Context(Workspace::Dev),
        execution_kind: ActionExecution::NestedCli,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::StopRepl,
        label: "Stop REPL",
        description: "Request a graceful REPL stop",
        shortcut: None,
        category: ActionCategory::Run,
        palette_placement: PalettePlacement::Context(Workspace::Dev),
        execution_kind: ActionExecution::Internal,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::ForceStopRepl,
        label: "Force Stop REPL",
        description: "Kill the REPL after its graceful-stop window",
        shortcut: None,
        category: ActionCategory::Run,
        palette_placement: PalettePlacement::Context(Workspace::Dev),
        execution_kind: ActionExecution::Internal,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::RestartRepl,
        label: "Restart REPL",
        description: "Restart the embedded REPL",
        shortcut: None,
        category: ActionCategory::Run,
        palette_placement: PalettePlacement::Context(Workspace::Dev),
        execution_kind: ActionExecution::NestedCli,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::OpenPalette,
        label: "Open Commands",
        description: "Open the searchable command overlay",
        shortcut: Some("ctrl+p"),
        category: ActionCategory::Tui,
        palette_placement: PalettePlacement::Global,
        execution_kind: ActionExecution::Internal,
        palette_visible: false,
    },
    ActionDefinition {
        id: ActionId::OpenContextPicker,
        label: "Switch Context",
        description: "Switch selected TUI context",
        shortcut: Some("ctrl+x e"),
        category: ActionCategory::GoTo,
        palette_placement: PalettePlacement::Global,
        execution_kind: ActionExecution::Internal,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::ShowHelp,
        label: "Show Help",
        description: "Show TUI shortcuts",
        shortcut: Some("ctrl+h"),
        category: ActionCategory::Tui,
        palette_placement: PalettePlacement::Global,
        execution_kind: ActionExecution::Internal,
        palette_visible: true,
    },
    ActionDefinition {
        id: ActionId::Quit,
        label: "Quit",
        description: "Exit the TUI",
        shortcut: Some("ctrl+q"),
        category: ActionCategory::Tui,
        palette_placement: PalettePlacement::Global,
        execution_kind: ActionExecution::Internal,
        palette_visible: true,
    },
];

fn action(id: ActionId) -> &'static ActionDefinition {
    ACTIONS
        .iter()
        .find(|action| action.id == id)
        .expect("registered action")
}

fn palette_actions(scope: PaletteScope, app: &TuiApp) -> Vec<ActionDefinition> {
    ACTIONS
        .iter()
        .copied()
        .filter(|action| action.palette_visible)
        .filter(|action| action_visible_for_state(action.id, app))
        .filter(|action| {
            action
                .palette_placement
                .visible_in(scope, app.active_workspace)
        })
        .collect()
}

fn action_visible_for_state(id: ActionId, app: &TuiApp) -> bool {
    match id {
        ActionId::RefreshAgents
        | ActionId::OpenAgentDatasetFilter
        | ActionId::OpenAgentColumns
        | ActionId::IncludeAgentMatches
        | ActionId::ExcludeAgentMatches
        | ActionId::LoadMoreAgents
        | ActionId::ToggleAgentAutoRefresh
        | ActionId::CycleAgentMode
        | ActionId::ToggleAgentDetails
        | ActionId::CycleAgentFindScope => app.ops_agents_focused(),
        ActionId::OpenAgentScopeInOplog | ActionId::OpenAgentScopeInMetrics => {
            app.active_workspace == Workspace::Ops && app.ops.active() == OpsDestination::Agents
        }
        ActionId::OpenAgentSsh => {
            app.active_workspace == Workspace::Ops && app.ops.active() == OpsDestination::Agents
        }
        ActionId::ToggleAgentStream => {
            app.active_workspace == Workspace::Ops
                && matches!(
                    app.ops.active(),
                    OpsDestination::Agents | OpsDestination::Oplog | OpsDestination::Stream
                )
        }
        ActionId::UseEnvironmentMetricsScope => {
            app.active_workspace == Workspace::Ops
                && app.ops.active() == OpsDestination::Metrics
                && app.metrics.source
                    == crate::model::local_observability::LocalMetricSource::Agents
                && !app.metrics.agent_scope.is_empty()
        }
        ActionId::StartServer => matches!(
            app.server.run.status,
            ServerStatus::Stopped | ServerStatus::Failed
        ),
        ActionId::StopServer => matches!(
            app.server.run.status,
            ServerStatus::Starting | ServerStatus::Running
        ),
        ActionId::ForceStopServer => app.server.run.status == ServerStatus::Stopping,
        ActionId::RestartServer => matches!(
            app.server.run.status,
            ServerStatus::Starting | ServerStatus::Running
        ),
        ActionId::StartRepl => matches!(
            app.repl.run.status,
            ReplStatus::Stopped | ReplStatus::Failed
        ),
        ActionId::StopRepl => matches!(
            app.repl.run.status,
            ReplStatus::Starting | ReplStatus::Running
        ),
        ActionId::ForceStopRepl => app.repl.run.status == ReplStatus::Stopping,
        ActionId::RestartRepl => matches!(
            app.repl.run.status,
            ReplStatus::Starting | ReplStatus::Running
        ),
        _ => true,
    }
}

const PALETTE_FUZZY_RESULT_LIMIT: usize = 5;
const PALETTE_FUZZY_EXTRA_SPAN: usize = 3;

#[derive(Debug, Clone)]
struct PaletteMatch {
    action: ActionDefinition,
    rank: u8,
    score: i64,
    registry_index: usize,
    label_matches: Vec<usize>,
    shortcut_matches: Vec<usize>,
    description_matches: Vec<usize>,
    category_matched: bool,
}

fn palette_action_description(app: &TuiApp, action: &ActionDefinition) -> String {
    match app.action_availability(action) {
        ActionAvailability::Available => action.description.to_string(),
        ActionAvailability::Unavailable(reason) => format!("Unavailable — {reason}"),
    }
}

fn ranked_palette_actions(query: &str, scope: PaletteScope, app: &TuiApp) -> Vec<PaletteMatch> {
    let query = query.trim();
    if query.is_empty() {
        let mut matches = palette_actions(scope, app)
            .into_iter()
            .map(|action| PaletteMatch {
                action,
                rank: 0,
                score: 0,
                registry_index: ACTIONS
                    .iter()
                    .position(|registered| registered.id == action.id)
                    .expect("registered palette action"),
                label_matches: Vec::new(),
                shortcut_matches: Vec::new(),
                description_matches: Vec::new(),
                category_matched: false,
            })
            .collect::<Vec<_>>();
        matches.sort_by_key(|matched| (matched.action.category.order(), matched.registry_index));
        return matches;
    }

    let actions = palette_actions(scope, app);
    let mut matches = actions
        .iter()
        .copied()
        .into_iter()
        .filter_map(|action| {
            let registry_index = ACTIONS
                .iter()
                .position(|registered| registered.id == action.id)
                .expect("registered palette action");
            structured_palette_match(action, query, registry_index, app)
        })
        .collect::<Vec<_>>();

    if matches.is_empty() && query.chars().count() >= 3 {
        matches = fuzzy_palette_matches(&actions, query, app);
    }

    matches.sort_by(|left, right| {
        left.rank
            .cmp(&right.rank)
            .then_with(|| right.score.cmp(&left.score))
            .then_with(|| {
                app.action_availability(&left.action)
                    .is_unavailable()
                    .cmp(&app.action_availability(&right.action).is_unavailable())
            })
            .then_with(|| left.registry_index.cmp(&right.registry_index))
    });
    if matches.first().is_some_and(|matched| matched.rank == 5) {
        matches.truncate(PALETTE_FUZZY_RESULT_LIMIT);
    }
    matches
}

fn structured_palette_match(
    action: ActionDefinition,
    query: &str,
    registry_index: usize,
    app: &TuiApp,
) -> Option<PaletteMatch> {
    let query = query.to_ascii_lowercase();
    let label = action.label.to_ascii_lowercase();
    let query_tokens = query.split_whitespace().collect::<Vec<_>>();
    let mut matched = PaletteMatch {
        action,
        rank: 0,
        score: 0,
        registry_index,
        label_matches: Vec::new(),
        shortcut_matches: Vec::new(),
        description_matches: Vec::new(),
        category_matched: false,
    };

    if label == query {
        matched.label_matches = (0..action.label.chars().count()).collect();
        return Some(matched);
    }
    if label.starts_with(&query) {
        matched.rank = 1;
        matched.label_matches = (0..query.chars().count()).collect();
        return Some(matched);
    }
    if let Some(indices) = word_prefix_match_indices(action.label, &query_tokens) {
        matched.rank = 2;
        matched.label_matches = indices;
        return Some(matched);
    }
    if let Some(indices) = substring_match_indices(action.label, &query) {
        matched.rank = 3;
        matched.label_matches = indices;
        return Some(matched);
    }

    let searchable_description = matches!(
        app.action_availability(&action),
        ActionAvailability::Available
    )
    .then_some(action.description);
    for token in query_tokens {
        if let Some(indices) = word_prefix_match_indices(action.label, &[token]) {
            matched.label_matches.extend(indices);
        } else if word_prefix_match_indices(action.category.label(), &[token]).is_some() {
            matched.category_matched = true;
        } else if let Some(indices) = searchable_description
            .and_then(|description| word_prefix_match_indices(description, &[token]))
        {
            matched.description_matches.extend(indices);
        } else if let Some(indices) = action
            .shortcut
            .and_then(|shortcut| substring_match_indices(shortcut, token))
        {
            matched.shortcut_matches.extend(indices);
        } else {
            return None;
        }
    }

    matched.rank = 4;
    normalize_match_indices(&mut matched.label_matches);
    normalize_match_indices(&mut matched.shortcut_matches);
    normalize_match_indices(&mut matched.description_matches);
    Some(matched)
}

fn fuzzy_palette_matches(
    actions: &[ActionDefinition],
    query: &str,
    app: &TuiApp,
) -> Vec<PaletteMatch> {
    let matcher = SkimMatcherV2::default();
    let query = query.to_ascii_lowercase();
    let query_length = query.chars().count();
    let mut matches = actions
        .iter()
        .copied()
        .filter_map(|action| {
            let (score, indices) =
                matcher.fuzzy_indices(&action.label.to_ascii_lowercase(), &query)?;
            let span = indices
                .last()
                .zip(indices.first())
                .map(|(last, first)| last.saturating_sub(*first) + 1)
                .unwrap_or_default();
            if span > query_length + PALETTE_FUZZY_EXTRA_SPAN {
                return None;
            }
            Some(PaletteMatch {
                action,
                rank: 5,
                score,
                registry_index: ACTIONS
                    .iter()
                    .position(|registered| registered.id == action.id)
                    .expect("registered palette action"),
                label_matches: indices,
                shortcut_matches: Vec::new(),
                description_matches: Vec::new(),
                category_matched: false,
            })
        })
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| {
                app.action_availability(&left.action)
                    .is_unavailable()
                    .cmp(&app.action_availability(&right.action).is_unavailable())
            })
            .then_with(|| left.registry_index.cmp(&right.registry_index))
    });
    matches.truncate(PALETTE_FUZZY_RESULT_LIMIT);
    matches
}

fn substring_match_indices(text: &str, query: &str) -> Option<Vec<usize>> {
    let text = text.to_ascii_lowercase().chars().collect::<Vec<_>>();
    let query = query.to_ascii_lowercase().chars().collect::<Vec<_>>();
    if query.is_empty() || query.len() > text.len() {
        return None;
    }
    let start = text
        .windows(query.len())
        .position(|candidate| candidate == query)?;
    Some((start..start + query.len()).collect())
}

fn word_prefix_match_indices(text: &str, query_tokens: &[&str]) -> Option<Vec<usize>> {
    if query_tokens.is_empty() {
        return None;
    }
    let words = indexed_words(text);
    let mut matches = Vec::new();
    for query in query_tokens {
        let query = query.to_ascii_lowercase();
        let (_, indices) = words.iter().find(|(word, _)| word.starts_with(&query))?;
        matches.extend(indices.iter().take(query.chars().count()).copied());
    }
    normalize_match_indices(&mut matches);
    Some(matches)
}

fn indexed_words(text: &str) -> Vec<(String, Vec<usize>)> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut indices = Vec::new();
    for (index, character) in text.chars().enumerate() {
        if character.is_alphanumeric() {
            word.extend(character.to_lowercase());
            indices.push(index);
        } else if !word.is_empty() {
            words.push((std::mem::take(&mut word), std::mem::take(&mut indices)));
        }
    }
    if !word.is_empty() {
        words.push((word, indices));
    }
    words
}

fn normalize_match_indices(indices: &mut Vec<usize>) {
    indices.sort_unstable();
    indices.dedup();
}

fn filtered_actions(query: &str, scope: PaletteScope, app: &TuiApp) -> Vec<ActionDefinition> {
    ranked_palette_actions(query, scope, app)
        .into_iter()
        .map(|matched| matched.action)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::GolemCliCommand;
    use crate::config::{AuthenticationConfig, Profile, ProfileName};
    use crate::log::{LogContext, Output, logln};
    use crate::model::local_observability::{
        LocalMetricDashboardPanel, LocalMetricDashboardPanelKind, LocalMetricDashboardResponse,
        LocalMetricDashboardSeries, LocalMetricDashboardStat, LocalMetricFamily, LocalMetricKind,
        LocalMetricSample, LocalMetricSamplesResponse, LocalMetricSeries, LocalMetricSource,
        LocalMetricTemporality, LocalMetricTimeRange, LocalObservabilityCapability,
        LocalObservabilityStatus,
    };
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
        dev.active_workspace = Workspace::Dev;
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
        app.active_workspace = Workspace::Dev;
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
                app.active_workspace = Workspace::Dev;
                app.dev_primary = DevView::BuildDeploy;
                app.dev_focus = DevView::BuildDeploy;
                app.layout.dev_visible = [false, true, true];
                app.layout.dev_placement = placement;

                let _ = render_app_text_at(&app, width, height);
            }
        }
    }

    #[test]
    fn dev_navigation_adds_and_focuses_without_replacing_the_main_view() {
        let mut app = test_app();
        app.dev_primary = DevView::BuildDeploy;
        app.dev_focus = DevView::BuildDeploy;
        app.layout.dev_visible = [true, false, false];

        app.open_dev_workspace(DevView::Server);

        assert_eq!(app.active_workspace, Workspace::Dev);
        assert_eq!(app.dev_primary, DevView::BuildDeploy);
        assert_eq!(app.dev_focus, DevView::Server);
        assert_eq!(
            app.layout.visible_dev_views(app.dev_primary),
            vec![DevView::BuildDeploy, DevView::Server]
        );
    }

    #[test]
    fn dev_view_toggles_keep_one_view_and_promote_the_focused_replacement() {
        let mut app = test_app();
        app.active_workspace = Workspace::Dev;
        app.dev_primary = DevView::BuildDeploy;
        app.dev_focus = DevView::BuildDeploy;
        app.layout.dev_visible = [true, false, false];

        assert!(!app.toggle_dev_view(DevView::BuildDeploy));
        app.open_dev_workspace(DevView::Server);
        assert!(app.toggle_dev_view(DevView::BuildDeploy));

        assert_eq!(app.dev_primary, DevView::Server);
        assert_eq!(app.dev_focus, DevView::Server);
        assert_eq!(
            app.layout.visible_dev_views(app.dev_primary),
            vec![DevView::Server]
        );
    }

    #[test]
    fn promoting_the_focused_dev_view_preserves_every_visible_view() {
        let mut app = test_app();
        app.dev_primary = DevView::BuildDeploy;
        app.dev_focus = DevView::BuildDeploy;
        app.layout.dev_visible = [true, false, false];
        app.open_dev_workspace(DevView::Server);
        app.open_dev_workspace(DevView::Repl);

        app.make_focused_dev_view_primary();

        assert_eq!(app.dev_primary, DevView::Repl);
        assert_eq!(app.dev_focus, DevView::Repl);
        assert_eq!(
            app.layout.visible_dev_views(app.dev_primary),
            vec![DevView::Repl, DevView::BuildDeploy, DevView::Server]
        );
    }

    #[test]
    fn stacked_title_row_is_the_split_and_supports_click_or_drag() {
        let mut click_app = test_app();
        click_app.active_workspace = Workspace::Dev;
        click_app.dev_primary = DevView::BuildDeploy;
        click_app.dev_focus = DevView::BuildDeploy;
        click_app.layout.dev_visible = [true, false, false];
        click_app.layout.dev_placement = layout::PanePlacement::Bottom;
        click_app.layout.set_dev_visible(DevView::Server, true);
        let frame = render_app_text_at(&click_app, 120, 32);
        let divider = snapshot_region(&click_app, RegionKind::DevPrimarySplit);
        let server_title = snapshot_region(&click_app, RegionKind::DevViewTitle(DevView::Server));
        assert_eq!(divider.y, server_title.y);
        assert_eq!(divider.height, 1);
        assert!(
            !frame
                .lines()
                .nth(usize::from(divider.y))
                .unwrap_or_default()
                .contains('━'),
            "stacked title row must not add a second resize-grip language: {frame}"
        );

        let title_x = server_title.x.saturating_add(2);
        click_app.handle_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                title_x,
                server_title.y,
            ),
            None,
        );
        assert_eq!(click_app.dev_focus, DevView::BuildDeploy);
        click_app.handle_mouse(
            mouse(
                MouseEventKind::Up(MouseButton::Left),
                title_x,
                server_title.y,
            ),
            None,
        );
        assert_eq!(click_app.dev_focus, DevView::Server);

        let mut drag_app = test_app();
        drag_app.active_workspace = Workspace::Dev;
        drag_app.dev_primary = DevView::BuildDeploy;
        drag_app.dev_focus = DevView::BuildDeploy;
        drag_app.layout.dev_visible = [true, false, false];
        drag_app.layout.dev_placement = layout::PanePlacement::Bottom;
        drag_app.layout.set_dev_visible(DevView::Server, true);
        render_app_text_at(&drag_app, 120, 32);
        let divider = snapshot_region(&drag_app, RegionKind::DevPrimarySplit);
        let before = drag_app.layout.dev_primary_ratio;
        drag_app.handle_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                divider.x + 2,
                divider.y,
            ),
            None,
        );
        drag_app.handle_mouse(
            mouse(
                MouseEventKind::Drag(MouseButton::Left),
                divider.x + 2,
                divider.y.saturating_add(2),
            ),
            None,
        );
        drag_app.handle_mouse(
            mouse(
                MouseEventKind::Up(MouseButton::Left),
                divider.x + 2,
                divider.y.saturating_add(2),
            ),
            None,
        );
        assert_ne!(drag_app.layout.dev_primary_ratio, before);
        assert_eq!(drag_app.dev_focus, DevView::BuildDeploy);
    }

    #[test]
    fn recursive_pane_junctions_match_right_and_bottom_layouts() {
        for (placement, expected) in [
            (layout::PanePlacement::Right, "├"),
            (layout::PanePlacement::Bottom, "┬"),
        ] {
            let mut app = test_app();
            app.active_workspace = Workspace::Dev;
            app.dev_primary = DevView::BuildDeploy;
            app.dev_focus = DevView::BuildDeploy;
            app.layout.dev_visible = [false, true, true];
            app.layout.dev_placement = placement;

            let buffer = render_app_buffer_at(&app, 120, 30);
            let primary = snapshot_region(&app, RegionKind::DevPrimarySplit);
            let secondary = snapshot_region(&app, RegionKind::DevSecondarySplit);
            let (x, y) = match placement {
                layout::PanePlacement::Right => (primary.x, secondary.y),
                layout::PanePlacement::Bottom => (secondary.x, secondary.y),
            };

            assert_eq!(buffer[(x, y)].symbol(), expected);
            assert_eq!(buffer[(x, y)].fg, theme().border_subtle);
            assert_eq!(buffer[(x, y)].bg, theme().surface);
        }
    }

    #[test]
    fn dev_terminal_scrollbars_use_the_shared_trailing_slot() {
        let mut app = test_app();
        app.active_workspace = Workspace::Dev;
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
    fn terminal_scrollbar_dragging_scrolls_without_resizing_the_pane() {
        let mut app = test_app();
        app.active_workspace = Workspace::Dev;
        app.dev_primary = DevView::BuildDeploy;
        app.dev_focus = DevView::BuildDeploy;
        app.layout.dev_visible = [true, true, false];
        app.server.run.status = ServerStatus::Running;
        for index in 0..80 {
            app.server
                .run
                .terminal
                .feed(format!("server line {index}\r\n").as_bytes());
        }

        render_app_text_at(&app, 120, 30);
        let scrollbar = snapshot_region(
            &app,
            RegionKind::Scrollbar(ScrollbarTarget::Dev(DevView::Server)),
        );
        let ratio = app.layout.dev_primary_ratio;
        app.handle_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                scrollbar.x,
                scrollbar.y,
            ),
            None,
        );

        assert!(app.server.run.terminal.scrollback() > 0);
        assert_eq!(app.dev_focus, DevView::Server);
        assert_eq!(app.layout.dev_primary_ratio, ratio);

        app.handle_mouse(
            mouse(
                MouseEventKind::Drag(MouseButton::Left),
                scrollbar.x,
                scrollbar.bottom().saturating_sub(1),
            ),
            None,
        );
        app.handle_mouse(
            mouse(
                MouseEventKind::Up(MouseButton::Left),
                scrollbar.x,
                scrollbar.bottom().saturating_sub(1),
            ),
            None,
        );

        assert_eq!(app.server.run.terminal.scrollback(), 0);
        assert_eq!(app.layout.dev_primary_ratio, ratio);
        assert!(app.scrollbar_dragging.is_none());
    }

    #[test]
    fn agent_list_scrollbar_click_selects_the_corresponding_loaded_row() {
        let mut app = test_app();
        app.agents.agents = (0..60)
            .map(|index| {
                let mut agent = sample_agents()[0].clone();
                agent.agent_id = format!("cart-{index:02}");
                agent
            })
            .collect();

        render_app_text_at(&app, 100, 24);
        let scrollbar = snapshot_region(&app, RegionKind::Scrollbar(ScrollbarTarget::OpsList));
        app.handle_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                scrollbar.x,
                scrollbar.bottom().saturating_sub(1),
            ),
            None,
        );

        assert_eq!(app.agents.selected, 59);
        assert_eq!(app.agents.table.selected, 59);
        assert_eq!(app.agents.focus, AgentOverviewFocus::List);
    }

    #[test]
    fn hidden_dev_sessions_publish_status_without_leaking_terminal_content_into_ops() {
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
        assert!(frame.contains("Server ● running"), "{frame}");
        assert!(frame.contains("REPL ● running"), "{frame}");
        assert!(
            frame.contains("─(Server ● running · REPL ● running)─"),
            "{frame}"
        );
        assert!(frame.contains("[ Ops ]"), "{frame}");
    }

    #[test]
    fn footer_session_status_click_reveals_then_toggles_server_and_repl() {
        let mut app = test_app();
        app.dev_primary = DevView::BuildDeploy;
        app.dev_focus = DevView::BuildDeploy;
        app.layout.dev_visible = [true, false, false];
        app.server.run.status = ServerStatus::Running;
        app.repl.run.status = ReplStatus::Running;
        render_app_text_at(&app, 120, 32);
        let server_status = snapshot_region(&app, RegionKind::DevSessionStatus(DevView::Server));

        app.handle_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                server_status.x,
                server_status.y,
            ),
            None,
        );

        assert_eq!(app.active_workspace, Workspace::Dev);
        assert_eq!(app.dev_primary, DevView::BuildDeploy);
        assert_eq!(app.dev_focus, DevView::Server);
        assert!(app.dev_view_visible(DevView::Server));
        assert_eq!(app.mode, InteractionMode::Normal);

        render_app_text_at(&app, 120, 32);
        let server_status = snapshot_region(&app, RegionKind::DevSessionStatus(DevView::Server));
        app.handle_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                server_status.x,
                server_status.y,
            ),
            None,
        );

        assert!(!app.dev_view_visible(DevView::Server));
        assert_eq!(app.dev_focus, DevView::BuildDeploy);

        render_app_text_at(&app, 120, 32);
        let repl_status = snapshot_region(&app, RegionKind::DevSessionStatus(DevView::Repl));
        app.handle_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                repl_status.x,
                repl_status.y,
            ),
            None,
        );

        assert_eq!(app.dev_primary, DevView::BuildDeploy);
        assert_eq!(app.dev_focus, DevView::Repl);
        assert!(app.dev_view_visible(DevView::Repl));

        render_app_text_at(&app, 120, 32);
        let repl_status = snapshot_region(&app, RegionKind::DevSessionStatus(DevView::Repl));
        app.handle_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                repl_status.x,
                repl_status.y,
            ),
            None,
        );

        assert!(!app.dev_view_visible(DevView::Repl));
        assert_eq!(app.dev_focus, DevView::BuildDeploy);
    }

    #[test]
    fn compact_footer_keeps_named_session_glyphs_before_omitting_the_cluster() {
        let mut app = test_app();
        app.server.run.status = ServerStatus::Running;
        app.repl.run.status = ReplStatus::Stopped;

        let compact = render_app_text_at(&app, 64, 18);
        assert!(compact.contains("Server ●"), "{compact}");
        assert!(compact.contains("REPL ○"), "{compact}");
        assert!(!compact.contains("Server ● running"), "{compact}");

        let narrow = render_app_text_at(&app, 32, 12);
        assert!(!narrow.contains("Server ●"), "{narrow}");
        assert!(!narrow.contains("REPL ○"), "{narrow}");
    }

    #[test]
    fn clicking_ops_while_cli_input_is_active_returns_control_to_the_tui() {
        let mut app = test_app();
        app.active_workspace = Workspace::Dev;
        app.dev_primary = DevView::BuildDeploy;
        app.dev_focus = DevView::BuildDeploy;
        app.layout.dev_visible = [true, false, false];
        app.command_run = Some(CommandRun::new(
            1,
            CommandKind::Build,
            vec!["build".to_string()],
            CommandOptions::default(),
            "sample-app:local".to_string(),
        ));
        app.mode.replace(InteractionMode::TerminalInput);
        render_app_text_at(&app, 120, 32);
        let ops = snapshot_region(&app, RegionKind::Workspace(Workspace::Ops));

        app.handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), ops.x + 1, ops.y),
            None,
        );
        let frame = render_app_text_at(&app, 120, 32);

        assert_eq!(app.active_workspace, Workspace::Ops);
        assert_eq!(app.mode, InteractionMode::Normal);
        assert!(!frame.contains("TUI control"), "{frame}");
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
        assert_eq!(
            buffer
                .cell((0, 2))
                .expect("missing pane title cell")
                .symbol(),
            "├"
        );
        for y in 3..21 {
            let symbol = buffer.cell((0, y)).expect("missing cell").symbol();
            assert_eq!(symbol, "│", "missing rail at row {y}");
        }
    }

    #[test]
    fn leader_hint_renders_navigation_groups() {
        let mut app = test_app();

        app.handle_key(modified_key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        let frame = render_app_text(&app);

        assert_eq!(app.mode, InteractionMode::Leader);
        assert!(frame.contains("Workspaces"), "{frame}");
        assert!(frame.contains("Views"), "{frame}");
        assert!(frame.contains("Layout"), "{frame}");
        assert!(frame.contains("Context"), "{frame}");
        assert!(frame.contains("Commands"), "{frame}");
        assert!(frame.contains("Jobs"), "{frame}");
    }

    #[test]
    fn leader_shortcut_switches_ops_destinations() {
        let mut app = test_app();

        app.handle_key(modified_key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        app.handle_key(key(KeyCode::Char('v')));
        app.handle_key(key(KeyCode::Char('m')));
        let frame = render_app_text(&app);
        assert_eq!(app.ops.active(), OpsDestination::Metrics);
        assert!(frame.contains("View  [Dashboard] Explore"), "{frame}");

        app.handle_key(modified_key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        app.handle_key(key(KeyCode::Char('v')));
        app.handle_key(key(KeyCode::Char('o')));
        let frame = render_app_text(&app);
        assert_eq!(app.ops.active(), OpsDestination::Overview);
        assert!(frame.contains("[ Overview ]"), "{frame}");
    }

    #[test]
    fn ops_tabs_are_stable_clickable_and_keep_the_active_tab_visible() {
        let mut app = test_app();
        let wide = render_app_text_at(&app, 140, 28);
        for label in [
            "Overview",
            "Agents",
            "Oplog",
            "Stream",
            "Metrics",
            "Deployments",
            "Configuration",
        ] {
            assert!(wide.contains(label), "missing {label}: {wide}");
        }

        app.ops.select(OpsDestination::Configuration);
        let narrow = render_app_text_at(&app, 42, 20);
        assert!(narrow.contains("[Configuration]"), "{narrow}");
        assert!(narrow.contains('‹'), "{narrow}");

        app.ops.select(OpsDestination::Overview);
        render_app_text_at(&app, 100, 24);
        let metrics = app
            .geometry
            .region(RegionKind::OpsTab(OpsDestination::Metrics))
            .expect("metrics tab");
        app.handle_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                metrics.x,
                metrics.y,
            ),
            None,
        );
        assert_eq!(app.ops.active(), OpsDestination::Metrics);
    }

    #[test]
    fn alt_number_is_not_a_workspace_shortcut() {
        let mut app = test_app();

        app.handle_key(modified_key(KeyCode::Char('1'), KeyModifiers::ALT));

        assert_eq!(app.active_workspace, Workspace::Ops);
        assert_eq!(app.mode, InteractionMode::Normal);
    }

    #[test]
    fn bare_workspace_number_starts_agent_find() {
        let mut app = test_app();
        app.handle_key(key(KeyCode::Char('2')));

        assert_eq!(app.active_workspace, Workspace::Ops);
        assert_eq!(app.dev_focus, DevView::Repl);
        assert_eq!(app.mode, InteractionMode::AgentFilter);
        assert_eq!(app.agents.query, "2");
    }

    #[test]
    fn opens_palette() {
        let mut app = test_app();

        app.handle_key(modified_key(KeyCode::Char('p'), KeyModifiers::CONTROL));
        for character in "refresh".chars() {
            app.handle_key(key(KeyCode::Char(character)));
        }
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
    fn command_palette_size_stays_stable_while_filtering_and_uses_available_space() {
        let mut app = test_app();
        app.active_workspace = Workspace::Dev;
        let terminal = Rect::new(0, 0, 140, 40);
        let all = palette_actions(PaletteScope::All, &app);
        let all_area = palette_popup_rect(terminal, &app);

        app.palette.query = "plan".to_string();
        let filtered_area = palette_popup_rect(terminal, &app);
        app.palette.query =
            "an intentionally very long query that still cannot resize the popup".to_string();
        app.palette.scope = PaletteScope::Context;
        let context_area = palette_popup_rect(terminal, &app);

        assert_eq!(filtered_area, all_area);
        assert_eq!(context_area, all_area);
        assert_eq!(
            all_area.height,
            ((all.len() + 9) as u16).min(terminal.height * 85 / 100)
        );
        assert!(all_area.width <= terminal.width * 85 / 100);
        assert!(all_area.height <= terminal.height * 85 / 100);
        assert_eq!(all_area.x, (terminal.width - all_area.width) / 2);
        assert_eq!(all_area.y, (terminal.height - all_area.height) / 2);

        let short = palette_popup_rect(Rect::new(0, 0, 100, 16), &app);
        assert_eq!(short.height, 13);

        let compact = palette_popup_rect(Rect::new(0, 0, 40, 12), &app);
        assert_eq!(compact, Rect::new(1, 1, 38, 10));

        let minimal = palette_popup_rect(Rect::new(0, 0, 1, 1), &app);
        assert_eq!(minimal, Rect::new(0, 0, 1, 1));
    }

    #[test]
    fn palette_category_markers_are_distinct_single_cell_glyphs() {
        let visual = theme();
        for (category, glyph, color) in [
            (ActionCategory::GoTo, "→", visual.info),
            (ActionCategory::Run, "▶", visual.accent),
            (ActionCategory::View, "▦", visual.text_secondary),
            (ActionCategory::Explore, "⌕", visual.success),
            (ActionCategory::Tui, "•", visual.text_muted),
        ] {
            let marker = palette_category_marker(category, false, &visual);
            assert_eq!(marker.glyph, glyph);
            assert_eq!(UnicodeWidthStr::width(marker.glyph), 1);
            assert_eq!(marker.style.fg, Some(color));
        }
    }

    #[test]
    fn palette_uses_five_intent_categories() {
        let visible_count = |category| {
            ACTIONS
                .iter()
                .filter(|action| action.palette_visible && action.category == category)
                .count()
        };

        assert_eq!(visible_count(ActionCategory::GoTo), 15);
        assert_eq!(visible_count(ActionCategory::Run), 13);
        assert_eq!(visible_count(ActionCategory::View), 7);
        assert_eq!(visible_count(ActionCategory::Explore), 12);
        assert_eq!(visible_count(ActionCategory::Tui), 2);
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
    fn action_metadata_captures_path_risk_and_execution() {
        let clean = *action(ActionId::Clean);
        assert_eq!(clean.leader_path(), Some("ctrl+x a"));
        assert_eq!(clean.risk(), ActionRisk::Destructive);
        assert_eq!(clean.execution_kind, ActionExecution::NestedCli);

        let refresh = *action(ActionId::RefreshAgents);
        assert_eq!(refresh.risk(), ActionRisk::ReadOnly);
        assert_eq!(refresh.execution_kind, ActionExecution::Direct);
    }

    #[test]
    fn remote_dev_mutations_are_discoverable_but_unavailable() {
        let mut app = test_app();
        app.active_workspace = Workspace::Dev;
        app.selected_target.context.uses_local_server = false;

        assert_eq!(
            app.action_availability(action(ActionId::Build)),
            ActionAvailability::Unavailable("remote Dev mutations are not enabled in this slice")
        );
        assert!(
            filtered_actions("build", PaletteScope::All, &app)
                .iter()
                .any(|action| action.id == ActionId::Build)
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
            action(ActionId::RefreshAgents).execution_kind,
            ActionExecution::Direct
        );
        assert!(
            filtered_actions("direct", PaletteScope::All, &app)
                .iter()
                .all(|action| action.id != ActionId::RefreshAgents)
        );
        assert!(
            filtered_actions("refresh", PaletteScope::All, &app)
                .iter()
                .any(|action| action.id == ActionId::RefreshAgents)
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
                .all(|action| action.id != ActionId::OpenPalette)
        );

        let explore_actions = filtered_actions("explore", PaletteScope::All, &app);
        assert!(
            explore_actions
                .iter()
                .any(|action| action.id == ActionId::RefreshAgents),
            "{explore_actions:?}"
        );

        let go_to_actions = ranked_palette_actions("go to", PaletteScope::All, &app);
        assert!(!go_to_actions.is_empty());
        assert!(go_to_actions.iter().all(|matched| {
            matched.action.category == ActionCategory::GoTo && matched.category_matched
        }));

        assert!(
            filtered_actions("sample-app/local/local", PaletteScope::All, &app).is_empty(),
            "dynamic targets must not affect palette matching"
        );
        assert!(filtered_actions("read-only", PaletteScope::All, &app).is_empty());
        assert!(filtered_actions("interface", PaletteScope::All, &app).is_empty());
    }

    #[test]
    fn all_palette_scope_combines_global_and_current_workspace_actions() {
        let mut app = test_app();
        app.active_workspace = Workspace::Ops;
        let ops_all = palette_actions(PaletteScope::All, &app);
        let ops_context = palette_actions(PaletteScope::Context, &app);

        assert!(
            ops_context
                .iter()
                .all(|context| ops_all.iter().any(|action| action.id == context.id))
        );
        assert!(
            ops_all
                .iter()
                .any(|action| action.id == ActionId::RefreshAgents)
        );
        assert!(
            ops_all
                .iter()
                .any(|action| action.id == ActionId::SelectDevServer)
        );
        assert!(ops_all.iter().any(|action| action.id == ActionId::ShowHelp));
        assert!(
            ops_context
                .iter()
                .any(|action| action.id == ActionId::SelectOpsAgents)
        );
        assert!(
            ops_context
                .iter()
                .all(|action| action.id != ActionId::ShowHelp)
        );
        assert!(
            ops_all
                .iter()
                .all(|action| action.id != ActionId::ToggleServerView)
        );
        assert!(ops_all.iter().all(|action| action.id != ActionId::Build));

        app.active_workspace = Workspace::Dev;
        let dev_all = palette_actions(PaletteScope::All, &app);
        assert!(
            dev_all
                .iter()
                .any(|action| action.id == ActionId::ToggleServerView)
        );
        assert!(
            dev_all
                .iter()
                .any(|action| action.id == ActionId::SelectOpsAgents)
        );
        assert!(
            dev_all
                .iter()
                .all(|action| action.id != ActionId::RefreshAgents)
        );

        let dev_context = palette_actions(PaletteScope::Context, &app);
        assert!(
            dev_context
                .iter()
                .any(|action| action.id == ActionId::SelectDevServer)
        );
        assert!(
            dev_context
                .iter()
                .all(|action| action.id != ActionId::SelectOpsAgents)
        );
        assert!(
            dev_context
                .iter()
                .all(|action| action.id != ActionId::ShowHelp)
        );
    }

    #[test]
    fn empty_palette_groups_actions_by_intent_category() {
        let app = test_app();
        let matches = ranked_palette_actions("", PaletteScope::All, &app);

        assert!(
            matches
                .windows(2)
                .all(|pair| { pair[0].action.category.order() <= pair[1].action.category.order() })
        );
    }

    #[test]
    fn palette_prefers_structured_matches_before_bounded_label_fallback() {
        let mut app = test_app();
        app.active_workspace = Workspace::Dev;
        let plan = ranked_palette_actions("plan", PaletteScope::All, &app);

        assert_eq!(plan.len(), 1, "{plan:?}");
        assert_eq!(plan[0].action.id, ActionId::Plan);
        assert_eq!(plan[0].rank, 0);

        let typo = ranked_palette_actions("pln", PaletteScope::All, &app);
        assert!(!typo.is_empty(), "{typo:?}");
        assert_eq!(typo[0].action.id, ActionId::Plan);
        assert!(typo.iter().all(|matched| matched.rank == 5));
        assert!(typo.len() <= PALETTE_FUZZY_RESULT_LIMIT);
    }

    #[test]
    fn palette_tokens_can_match_across_visible_fields() {
        let app = test_app();
        let matches = filtered_actions("explore agent", PaletteScope::All, &app);

        assert!(
            matches
                .iter()
                .any(|action| action.id == ActionId::RefreshAgents),
            "{matches:?}"
        );
    }

    #[test]
    fn footer_exposes_only_the_shared_shell_shortcuts() {
        let app = test_app();
        let footer = render_app_text_at(&app, 120, 30);

        assert!(footer.contains("ctrl+x Menu"), "{footer}");
        assert!(footer.contains("ctrl+p Commands"), "{footer}");
        assert!(footer.contains("ctrl+h Help"), "{footer}");
        assert!(!footer.contains("ctrl+x b"), "{footer}");
    }

    #[test]
    fn layout_footer_shows_only_controls_for_the_active_workspace() {
        let mut app = test_app();
        app.mode.replace(InteractionMode::Layout);

        let ops = render_app_text_at(&app, 180, 30);
        assert!(ops.contains("d Toggle Details"), "{ops}");
        assert!(!ops.contains("b / s / r Toggle views"), "{ops}");
        assert!(!ops.contains("m Make main"), "{ops}");

        app.active_workspace = Workspace::Dev;
        let dev = render_app_text_at(&app, 180, 30);
        assert!(dev.contains("b / s / r Toggle views"), "{dev}");
        assert!(dev.contains("m Make main"), "{dev}");
        assert!(!dev.contains("d Toggle Details"), "{dev}");
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
        driver.assert_visible(&app, "Commands");

        app.handle_key(key(KeyCode::Char('a')));
        let labels = app
            .filtered_actions()
            .into_iter()
            .map(|action| action.label)
            .collect::<HashSet<_>>();
        for label in [
            "Toggle Agent Auto Refresh",
            "Cycle Agent Mode",
            "Configure Agent Columns",
        ] {
            assert!(labels.contains(label), "expected registered `{label}`");
        }
    }

    #[test]
    fn leader_opens_context_picker() {
        let mut app = test_app();

        app.handle_key(modified_key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        app.handle_key(key(KeyCode::Char('e')));

        assert_eq!(app.mode, InteractionMode::ContextPicker);
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

        assert_eq!(app.mode, InteractionMode::ContextPicker);
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
    fn active_work_quit_defaults_to_return_and_requires_explicit_force() {
        let mut app = test_app();
        app.active_workspace = Workspace::Dev;
        app.dev_primary = DevView::BuildDeploy;
        app.dev_focus = DevView::BuildDeploy;
        app.start_command(CommandKind::Build, None);

        app.handle_key(modified_key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        app.handle_key(modified_key(KeyCode::Char('q'), KeyModifiers::CONTROL));

        assert_eq!(app.mode, InteractionMode::QuitBlocked);
        assert_eq!(app.quit_choice, QuitChoice::Return);
        assert!(!app.should_quit);
        let frame = render_app_text_at(&app, 100, 24);
        assert!(
            frame
                .lines()
                .any(|line| line.contains("Return") && line.contains("Force quit")),
            "{frame}"
        );

        app.handle_key(key(KeyCode::Right));
        assert_eq!(app.quit_choice, QuitChoice::ForceQuit);
        app.handle_key(key(KeyCode::Enter));
        assert!(app.should_quit);
    }

    #[test]
    fn nested_terminal_keeps_all_keys_except_ctrl_x() {
        let mut app = test_app();
        app.active_workspace = Workspace::Dev;
        app.dev_primary = DevView::BuildDeploy;
        app.dev_focus = DevView::BuildDeploy;
        app.start_command(CommandKind::Build, None);

        app.handle_key(key(KeyCode::Tab));
        app.handle_key(modified_key(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert_eq!(app.mode, InteractionMode::TerminalInput);
        assert_eq!(
            app.command_run.as_ref().map(|run| run.status),
            Some(CommandStatus::Running)
        );

        app.handle_key(modified_key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        assert_eq!(app.mode, InteractionMode::Normal);
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.mode, InteractionMode::TerminalInput);
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
                server_identity: ServerIdentity::Named("profile:prod".to_string()),
                server_label: "Profile production with long descriptive name".to_string(),
                server_flags: GolemCliGlobalFlags::default(),
                environment_reference: None,
                app: Some("large-application".to_string()),
                environment: Some("prod".to_string()),
                dev_eligible: false,
                error: None,
                is_current: false,
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

        assert_eq!(app.mode, InteractionMode::ContextSwitchConfirm);
        assert!(app.context_switcher.pending_action.is_some());
        assert!(app.context_switcher.last_error.is_none());
        assert!(!app.context_switcher.switch_running);

        let frame = render_app_text(&app);
        assert_eq!(
            frame
                .matches("Switching context will stop context-bound work")
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
    fn context_switch_confirmation_stops_a_server_bound_to_another_target() {
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
        assert_eq!(app.server.run.status, ServerStatus::Stopping);
        assert!(app.context_switcher.waiting_for_dev_stop);
    }

    #[test]
    fn context_switch_confirmation_can_force_stop_a_different_server_after_five_seconds() {
        let mut app = test_app();
        app.server.run.status = ServerStatus::Running;
        app.server.run.server_identity = Some(ServerIdentity::Local);
        let (tx, _rx) = test_event_channel();

        app.open_context_picker();
        app.context_switcher
            .targets
            .push(test_manifest_context_target("prod"));
        app.context_switcher.show_targets();
        app.handle_key(key(KeyCode::Down));
        app.handle_key_with_events(key(KeyCode::Enter), Some(&tx));
        app.handle_key_with_events(key(KeyCode::Enter), Some(&tx));

        assert_eq!(app.server.run.status, ServerStatus::Stopping);
        app.server.run.stop_requested_at = Some(Instant::now() - std::time::Duration::from_secs(5));
        app.handle_key_with_events(key(KeyCode::Enter), Some(&tx));

        assert_eq!(app.server.run.status, ServerStatus::Stopped);
        assert!(!app.context_switcher.waiting_for_dev_stop);
        assert!(app.context_switcher.pending_action.is_none());
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
        assert_ne!(app.mode, InteractionMode::ContextSwitchConfirm);
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
        app.selected_target.context.uses_local_server = false;
        app.selected_target.context.dev_eligible = false;

        assert_eq!(
            app.action_availability(action(ActionId::StartServer)),
            ActionAvailability::Unavailable("requires an available local server target")
        );

        app.handle_key(key(KeyCode::Char('s')));

        assert_eq!(app.server.run.status, ServerStatus::Stopped);
        assert_eq!(app.active_workspace, Workspace::Ops);
    }

    #[test]
    fn context_picker_requires_confirmation_for_a_different_server() {
        let mut app = test_app();
        app.context_switcher = ContextSwitcherState::new(vec![
            test_current_context_target(),
            test_manifest_context_target("prod"),
        ]);
        app.server.run.status = ServerStatus::Running;

        app.open_context_picker();
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Enter));

        assert_eq!(app.mode, InteractionMode::ContextSwitchConfirm);
        assert_eq!(app.server.run.status, ServerStatus::Running);
        assert!(app.context_switcher.pending_action.is_some());
        let frame = render_app_text(&app);
        assert!(frame.contains("server"), "{frame}");
    }

    #[test]
    fn context_picker_keeps_a_server_bound_to_the_same_target() {
        let mut app = test_app();
        let mut other_local = test_manifest_context_target("other-local");
        other_local.server_identity = ServerIdentity::Local;
        other_local.detail = "server: local".to_string();
        app.context_switcher =
            ContextSwitcherState::new(vec![test_current_context_target(), other_local]);
        app.server.run.status = ServerStatus::Running;

        app.open_context_picker();
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Enter));

        assert_ne!(app.mode, InteractionMode::ContextSwitchConfirm);
        assert_eq!(app.server.run.status, ServerStatus::Running);
        assert!(app.context_switcher.pending_action.is_none());
    }

    #[test]
    fn app_environment_details_map_to_picker_targets() {
        let app = test_app();
        let server_target = TuiContextTarget {
            key: "server:local".to_string(),
            server_identity: ServerIdentity::Local,
            label: "Built-in local server".to_string(),
            detail: "local".to_string(),
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
            &app.selected_target.context,
            &app.selected_target.server_identity,
        );

        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].label, "sample-app/local");
        assert!(targets[0].dev_eligible);
        assert!(targets[0].is_current);
        assert!(matches!(
            targets[0].environment_reference,
            Some(EnvironmentReference::ApplicationEnvironment { .. })
        ));
    }

    #[test]
    fn context_markers_follow_application_environment_and_server() {
        let mut app = test_app();
        let local = test_current_context_target();
        let mut cloud = test_current_context_target();
        cloud.key = "manifest:local:cloud".to_string();
        cloud.server_identity = ServerIdentity::Named("cloud".to_string());
        cloud.detail = "server: cloud".to_string();
        cloud.is_current = false;
        app.context_switcher.targets = vec![local, cloud];
        app.context_switcher.app_environments = vec![
            TuiAppEnvironmentTarget {
                label: "sample-app/local".to_string(),
                detail: "local".to_string(),
                server_identity: ServerIdentity::Local,
                server_label: "Built-in local server".to_string(),
                server_flags: GolemCliGlobalFlags::default(),
                environment_reference: None,
                app: Some("sample-app".to_string()),
                environment: Some("local".to_string()),
                dev_eligible: true,
                error: None,
                is_current: true,
            },
            TuiAppEnvironmentTarget {
                label: "sample-app/local".to_string(),
                detail: "cloud".to_string(),
                server_identity: ServerIdentity::Named("cloud".to_string()),
                server_label: "Built-in cloud server".to_string(),
                server_flags: GolemCliGlobalFlags::default(),
                environment_reference: None,
                app: Some("sample-app".to_string()),
                environment: Some("local".to_string()),
                dev_eligible: false,
                error: None,
                is_current: false,
            },
        ];

        app.selected_target.context.server = "cloud".to_string();
        app.selected_target.context.uses_local_server = false;
        app.selected_target.server_identity = ServerIdentity::Named("cloud".to_string());
        app.refresh_context_target_markers();

        assert!(!app.context_switcher.targets[0].is_current);
        assert!(app.context_switcher.targets[1].is_current);
        assert!(!app.context_switcher.app_environments[0].is_current);
        assert!(app.context_switcher.app_environments[1].is_current);
        assert_eq!(
            app_environment_marker(&app.context_switcher.app_environments[1]).as_deref(),
            Some("current")
        );
    }

    #[test]
    fn ops_only_context_disables_dev_actions() {
        let mut app = test_app();
        app.selected_target.context.dev_eligible = false;

        assert_eq!(
            app.action_availability(action(ActionId::Build)),
            ActionAvailability::Unavailable("requires a manifest-backed application")
        );

        app.handle_key(key(KeyCode::Char('b')));

        assert_eq!(app.active_workspace, Workspace::Ops);
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
                server_identity: ServerIdentity::Local,
                server_label: "Current launch context".to_string(),
                server_flags: GolemCliGlobalFlags::default(),
                environment_reference: None,
                app: Some("sample-app".to_string()),
                environment: Some("local".to_string()),
                dev_eligible: true,
                error: None,
                is_current: true,
            }],
        );

        app.handle_key(key(KeyCode::Esc));

        assert_eq!(app.mode, InteractionMode::ContextPicker);
        assert_eq!(app.context_switcher.mode, ContextPickerStep::Targets);
    }

    #[test]
    fn selected_context_args_prefix_nested_cli_specs() {
        let mut app = test_app();
        app.selected_target.cli_args = vec![
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
        app.selected_target.cli_args = vec![
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
        app.mode.replace(InteractionMode::AgentFilter);
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
    fn included_agent_scope_opens_a_sorted_retained_oplog_route() {
        let mut app = test_app();
        app.agents.agents = sample_agents();
        app.agents.included = [
            AgentIdentity {
                component: "orders".to_string(),
                agent_id: "order-1".to_string(),
            },
            AgentIdentity {
                component: "cart".to_string(),
                agent_id: "cart-2".to_string(),
            },
        ]
        .into_iter()
        .collect();

        app.execute_action(ActionId::OpenAgentScopeInOplog, None);

        assert_eq!(app.ops.active(), OpsDestination::Oplog);
        assert_eq!(
            app.ops.current_oplog(),
            Some(&OplogRoute {
                agents: vec![
                    AgentIdentity {
                        component: "cart".to_string(),
                        agent_id: "cart-2".to_string(),
                    },
                    AgentIdentity {
                        component: "orders".to_string(),
                        agent_id: "order-1".to_string(),
                    },
                ],
                focus: None,
                from_unix_nanos: None,
                to_unix_nanos: None,
            })
        );
    }

    #[test]
    fn visible_agent_handoff_links_open_the_explicit_scope() {
        let mut app = test_app();
        app.agents.agents = sample_agents();
        app.agents.included = [
            AgentIdentity {
                component: "orders".to_string(),
                agent_id: "order-1".to_string(),
            },
            AgentIdentity {
                component: "cart".to_string(),
                agent_id: "cart-2".to_string(),
            },
        ]
        .into_iter()
        .collect();

        let frame = render_app_text_at(&app, 140, 30);
        assert!(frame.contains("open  Oplog  Metrics  Stream"), "{frame}");
        let oplog = snapshot_region(&app, RegionKind::OpsLink(OpsDestination::Oplog));
        assert!(
            app.geometry
                .region(RegionKind::OpsLink(OpsDestination::Metrics))
                .is_some()
        );
        assert!(
            app.geometry
                .region(RegionKind::OpsLink(OpsDestination::Stream))
                .is_none()
        );

        app.handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), oplog.x, oplog.y),
            None,
        );

        assert_eq!(app.ops.active(), OpsDestination::Oplog);
        assert_eq!(
            app.ops
                .current_oplog()
                .expect("visible Oplog link should retain scope")
                .agents
                .iter()
                .map(|agent| agent.agent_id.as_str())
                .collect::<Vec<_>>(),
            vec!["cart-2", "order-1"]
        );
    }

    #[test]
    fn oplog_and_stream_headers_expose_exact_agent_transitions() {
        let mut app = test_app();
        let cart = AgentIdentity {
            component: "cart".to_string(),
            agent_id: "cart-1".to_string(),
        };
        let order = AgentIdentity {
            component: "orders".to_string(),
            agent_id: "order-1".to_string(),
        };
        app.ops.open_oplog(OplogRoute {
            agents: vec![cart.clone(), order.clone()],
            focus: None,
            from_unix_nanos: None,
            to_unix_nanos: None,
        });
        app.activity.representation = ActivityRepresentation::Entries;
        app.activity.entries = vec![activity::ActivityEntry {
            component: order.component.clone(),
            agent_id: order.agent_id.clone(),
            presentation: crate::model::agent::oplog::OplogEntryPresentation {
                index: 7,
                timestamp: golem_common::model::Timestamp::from(7),
                kind: "Log".to_string(),
                group: None,
                detail: serde_json::json!({"message": "ready"}),
                attribution: None,
                entry: None,
                search_text: std::sync::OnceLock::new(),
            },
        }];

        let oplog_frame = render_app_text_at(&app, 140, 30);
        assert!(
            oplog_frame.contains("open  Agent  Metrics  Stream"),
            "{oplog_frame}"
        );
        let agent = snapshot_region(&app, RegionKind::OpsLink(OpsDestination::Agents));
        app.handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), agent.x, agent.y),
            None,
        );
        assert_eq!(app.ops.current_agent(), Some(&order));

        app.ops.open_stream(cart.clone());
        let stream_frame = render_app_text_at(&app, 140, 30);
        assert!(
            stream_frame.contains("open  Agent  Oplog  Metrics"),
            "{stream_frame}"
        );
        let metrics = snapshot_region(&app, RegionKind::OpsLink(OpsDestination::Metrics));
        app.handle_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                metrics.x,
                metrics.y,
            ),
            None,
        );
        assert_eq!(app.ops.active(), OpsDestination::Metrics);
        assert_eq!(app.metrics.agent_scope, vec![cart]);
    }

    #[test]
    fn passive_stream_handoff_never_retargets_a_live_session() {
        let mut app = test_app();
        app.agents.agents = sample_agents();
        app.agents.selected = 1;
        let running = AgentIdentity {
            component: "cart".to_string(),
            agent_id: "cart-1".to_string(),
        };
        let selected = AgentIdentity {
            component: "cart".to_string(),
            agent_id: "cart-2".to_string(),
        };
        app.stream.target = Some(running.clone());
        app.stream.status = StreamStatus::Connected;

        let frame = render_app_text_at(&app, 140, 30);
        assert!(frame.contains("Stream"), "{frame}");
        assert!(
            app.geometry
                .region(RegionKind::OpsLink(OpsDestination::Stream))
                .is_none()
        );

        app.open_related_destination(OpsDestination::Stream, None);
        assert_eq!(app.ops.active(), OpsDestination::Agents);
        assert_eq!(app.stream.target, Some(running));

        app.stream.disconnect();
        render_app_text_at(&app, 140, 30);
        let stream = snapshot_region(&app, RegionKind::OpsLink(OpsDestination::Stream));
        app.handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), stream.x, stream.y),
            None,
        );
        assert_eq!(app.ops.current_stream(), Some(&selected));
        assert_eq!(app.stream.target, None);
        assert_eq!(app.stream.status, StreamStatus::Disconnected);
    }

    #[test]
    fn agent_scope_is_retained_by_metrics_until_explicitly_cleared() {
        let mut app = test_app();
        app.agents.agents = sample_agents();
        app.agents.included = [
            AgentIdentity {
                component: "orders".to_string(),
                agent_id: "order-1".to_string(),
            },
            AgentIdentity {
                component: "cart".to_string(),
                agent_id: "cart-1".to_string(),
            },
        ]
        .into_iter()
        .collect();

        app.execute_action(ActionId::OpenAgentScopeInMetrics, None);

        assert_eq!(app.ops.active(), OpsDestination::Metrics);
        assert_eq!(
            app.metrics
                .agent_scope
                .iter()
                .map(|agent| agent.agent_id.as_str())
                .collect::<Vec<_>>(),
            vec!["cart-1", "order-1"]
        );
        assert_eq!(
            app.metrics_query(MetricsLoad::Replace)
                .expect("metrics query")
                .agent_ids,
            vec!["cart-1", "order-1"]
        );
        assert!(
            palette_actions(PaletteScope::Context, &app)
                .iter()
                .any(|action| action.id == ActionId::UseEnvironmentMetricsScope)
        );
        let frame = render_app_text_at(&app, 160, 28);
        assert!(frame.contains("Scope 2 Agents"), "{frame}");

        app.execute_action(ActionId::UseEnvironmentMetricsScope, None);

        assert!(app.metrics.agent_scope.is_empty());
        assert!(
            app.metrics_query(MetricsLoad::Replace)
                .expect("metrics query")
                .agent_ids
                .is_empty()
        );
        assert!(
            palette_actions(PaletteScope::Context, &app)
                .iter()
                .all(|action| action.id != ActionId::UseEnvironmentMetricsScope)
        );
    }

    #[test]
    fn full_agent_route_handoffs_only_that_exact_agent() {
        let mut app = test_app();
        app.agents.agents = sample_agents();
        app.agents.included = app
            .agents
            .agents
            .iter()
            .map(AgentListItem::identity)
            .collect();
        let exact = AgentIdentity {
            component: "cart".to_string(),
            agent_id: "cart-2".to_string(),
        };
        app.ops.open_agent(exact.clone());

        let actions = palette_actions(PaletteScope::Context, &app);
        assert!(
            actions
                .iter()
                .any(|action| action.id == ActionId::OpenAgentScopeInOplog)
        );
        assert!(
            actions
                .iter()
                .any(|action| action.id == ActionId::OpenAgentScopeInMetrics)
        );

        app.execute_action(ActionId::OpenAgentScopeInMetrics, None);

        assert_eq!(app.metrics.agent_scope, vec![exact]);
        assert_eq!(app.metrics.scope_label(), "Agent cart-2");
    }

    #[test]
    fn agent_handoff_actions_enforce_the_oplog_scope_limit() {
        let mut app = test_app();
        app.agents.included = (0..=activity::HARD_AGENT_LIMIT)
            .map(|index| AgentIdentity {
                component: "cart".to_string(),
                agent_id: format!("cart-{index}"),
            })
            .collect();

        assert_eq!(
            app.action_availability(action(ActionId::OpenAgentScopeInOplog)),
            ActionAvailability::Unavailable("include at most 20 Agents")
        );
        assert_eq!(
            app.action_availability(action(ActionId::OpenAgentScopeInMetrics)),
            ActionAvailability::Unavailable("include at most 20 Agents")
        );
    }

    #[test]
    fn stream_uses_one_included_agent_and_rejects_a_mixed_set() {
        let mut app = test_app();
        app.agents.agents = sample_agents();
        let included = AgentIdentity {
            component: "orders".to_string(),
            agent_id: "order-1".to_string(),
        };
        app.agents.included.insert(included.clone());

        assert_eq!(app.stream_target_candidate(), Some(included));

        app.agents.included.insert(AgentIdentity {
            component: "cart".to_string(),
            agent_id: "cart-1".to_string(),
        });
        assert!(app.stream_target_candidate().is_none());
        assert_eq!(
            app.action_availability(action(ActionId::ToggleAgentStream)),
            ActionAvailability::Unavailable("select one exact Agent")
        );
    }

    #[test]
    fn plain_oplog_navigation_does_not_adopt_hidden_agent_selection() {
        let mut app = test_app();
        app.agents.agents = sample_agents();
        app.ops.select(OpsDestination::Oplog);

        app.refresh_activity(None);

        assert!(app.activity_scope().is_none());
        assert!(
            app.activity
                .error
                .as_deref()
                .is_some_and(|error| error.contains("has no Agent scope"))
        );
    }

    #[test]
    fn agent_dataset_picker_uses_loaded_component_and_type_choices() {
        let mut app = test_app();
        app.agents.agents = sample_agents();

        app.handle_key(modified_key(KeyCode::Char('f'), KeyModifiers::CONTROL));
        let frame = render_app_text_at(&app, 100, 30);

        assert_eq!(app.mode, InteractionMode::AgentDatasetFilter);
        assert!(frame.contains("Agent dataset"), "{frame}");
        assert!(
            frame.contains("Server-side filter · 5/5 choices"),
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

        app.execute_action(ActionId::OpenAgentColumns, None);
        let frame = render_app_text_at(&app, 100, 30);

        assert_eq!(app.mode, InteractionMode::AgentColumns);
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
        assert_eq!(app.mode, InteractionMode::AgentFilter);

        let old_ratio = app.layout.ops_details_ratio;
        app.handle_key(modified_key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        app.handle_key(key(KeyCode::Char('l')));
        app.handle_key(key(KeyCode::Right));
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.mode, InteractionMode::AgentFilter);
        assert!(app.layout.ops_details_ratio > old_ratio);

        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.mode, InteractionMode::Normal);
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
        assert_eq!(app.mode, InteractionMode::Normal);
    }

    #[test]
    fn leader_shortcuts_remain_available_while_find_is_editing() {
        let mut app = test_app();
        app.agents.agents = sample_agents();
        app.handle_key(key(KeyCode::Char('c')));
        assert_eq!(app.mode, InteractionMode::AgentFilter);

        app.handle_key(modified_key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        app.handle_key(key(KeyCode::Char('a')));

        assert_eq!(app.mode, InteractionMode::Palette);
        assert_eq!(app.palette.scope, PaletteScope::Context);
        assert_eq!(app.agents.query, "c");
    }

    #[test]
    fn modified_agent_shortcuts_remain_available_while_find_is_editing() {
        let mut app = test_app();
        app.agents.agents = sample_agents();

        app.handle_key(key(KeyCode::Char('c')));
        assert_eq!(app.mode, InteractionMode::AgentFilter);
        assert_eq!(app.agents.query, "c");

        let selected = app.filtered_agents()[app.agents.selected].identity();
        app.handle_key(modified_key(KeyCode::Char(' '), KeyModifiers::CONTROL));
        assert!(app.agents.included.contains(&selected));
        assert_eq!(app.mode, InteractionMode::AgentFilter);

        app.handle_key(modified_key(KeyCode::Char('p'), KeyModifiers::CONTROL));
        assert_eq!(app.mode, InteractionMode::Palette);
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
        app.mode.replace(InteractionMode::AgentFilter);

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
    fn production_metrics_view_waits_for_real_local_samples() {
        let mut app = test_app();

        app.handle_key(modified_key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        app.handle_key(key(KeyCode::Char('v')));
        app.handle_key(key(KeyCode::Char('m')));
        let frame = render_app_text_at(&app, 120, 28);

        assert_eq!(app.ops.active(), OpsDestination::Metrics);
        assert!(frame.contains("[ Metrics ]"), "{frame}");
        assert!(frame.contains("View  [Dashboard] Explore"), "{frame}");
        assert!(
            frame.contains("No dashboard snapshot is available"),
            "{frame}"
        );
        assert!(!frame.contains("golem.agent.invocations"), "{frame}");
    }

    #[test]
    fn overview_cockpit_is_lightweight_responsive_and_routes_explicitly() {
        let mut app = test_app();
        app.ops.select(OpsDestination::Overview);
        app.metrics.agent_scope = vec![AgentIdentity {
            component: "cart".to_string(),
            agent_id: "cart-1".to_string(),
        }];
        let mut environment = sample_ops_environment_snapshot("12");
        environment.deployment = Some(OpsDeploymentSnapshot {
            deployment_revision: "12".to_string(),
            version: "demo-12".to_string(),
            hash: "d933d8d24db984b94aa1".to_string(),
        });
        app.ops_environment.snapshot = Some(environment);
        let capability = sample_overview_capability(1_000_000_000_000);
        app.overview.observability.value = Some(OverviewObservability {
            capability: Some(capability),
        });

        let frame = render_app_text_at(&app, 120, 30);
        assert!(frame.contains("Environment  current state"), "{frame}");
        assert!(frame.contains("revision 12 · demo-12"), "{frame}");
        assert!(
            frame.contains("Find Agents and choose investigation scope"),
            "{frame}"
        );
        assert!(
            frame.contains("Open curated dashboards or metric Explore"),
            "{frame}"
        );
        assert!(!frame.contains("Invocations started"), "{frame}");
        for (width, height) in [(100, 18), (60, 18), (24, 8), (1, 1)] {
            render_app_text_at(&app, width, height);
        }

        app.overview.selected_item = 3;
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.ops.active(), OpsDestination::Metrics);
        assert_eq!(app.metrics.source, LocalMetricSource::Agents);
        assert_eq!(app.metrics.mode, MetricsMode::Dashboard);
        assert_eq!(app.metrics.range, metrics::MetricsRange::Minutes15);
        assert!(app.metrics.agent_scope.is_empty());
    }

    #[test]
    fn overview_server_and_issue_routes_are_retained_and_actionable() {
        let mut app = test_app();
        app.ops.select(OpsDestination::Overview);
        app.overview.selected_item = 6;
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.ops.current_route(), &OpsRoute::OverviewServer);
        let frame = render_app_text_at(&app, 80, 20);
        assert!(frame.contains("Overview / Server & collector"), "{frame}");

        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.ops.current_route(), &OpsRoute::Root);
        app.overview.observability.value = Some(OverviewObservability { capability: None });
        app.overview.selected_item = 7;
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.ops.current_route(), &OpsRoute::OverviewIssues);
        let frame = render_app_text_at(&app, 80, 20);
        assert!(
            frame.contains("Local observability is unavailable"),
            "{frame}"
        );

        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.ops.current_route(), &OpsRoute::OverviewServer);
    }

    #[test]
    fn production_metrics_dashboard_renders_typed_stats_and_series() {
        let mut app = test_app();
        app.ops.select(OpsDestination::Metrics);
        app.metrics.dashboard = Some(LocalMetricDashboardResponse {
            source: LocalMetricSource::Agents,
            range: LocalMetricTimeRange {
                from_unix_nanos: 10,
                to_unix_nanos: 40,
            },
            generated_at_unix_nanos: 40,
            freshest_sample_unix_nanos: Some(30),
            stats: vec![LocalMetricDashboardStat {
                id: "invocations".to_string(),
                label: "Invocations started".to_string(),
                value: Some(42.0),
                unit: "1".to_string(),
                approximate: false,
            }],
            panels: vec![LocalMetricDashboardPanel {
                id: "invocation-rate".to_string(),
                title: "Invocation rate".to_string(),
                kind: LocalMetricDashboardPanelKind::TimeSeries,
                series: vec![LocalMetricDashboardSeries {
                    key: "all".to_string(),
                    label: "All invocations".to_string(),
                    unit: "invocations/s".to_string(),
                    samples: vec![
                        LocalMetricSample {
                            timestamp_unix_nanos: 10,
                            value: 1.0,
                            statistic: "rate".to_string(),
                            source_oplog_index: None,
                        },
                        LocalMetricSample {
                            timestamp_unix_nanos: 20,
                            value: 2.0,
                            statistic: "rate".to_string(),
                            source_oplog_index: None,
                        },
                    ],
                    latest_value: Some(2.0),
                    approximate: false,
                }],
                empty_reason: None,
                observed_from_unix_nanos: Some(10),
                observed_to_unix_nanos: Some(20),
                approximate: false,
            }],
        });

        let frame = render_app_text_at(&app, 120, 28);

        assert!(frame.contains("Invocations started"), "{frame}");
        assert!(frame.contains("42"), "{frame}");
        assert!(frame.contains("Invocation rate"), "{frame}");
        assert!(frame.contains("All invocations"), "{frame}");
    }

    #[test]
    fn metrics_dashboard_windows_panels_without_squeezing_and_supports_pointer_scrolling() {
        let mut app = test_app();
        app.ops.select(OpsDestination::Metrics);
        app.metrics.dashboard = Some(sample_metrics_dashboard(6));

        render_app_text_at(&app, 60, 24);

        let visible = app.metrics.dashboard_visible_count.get();
        assert!((1..6).contains(&visible));
        assert_eq!(app.metrics.dashboard_first_visible.get(), 0);
        assert_eq!(app.metrics.dashboard_columns.get(), 1);
        let first_panel = snapshot_region(&app, RegionKind::MetricsPanel(0));
        assert!(first_panel.height >= 6, "{first_panel:?}");
        let scrollbar = snapshot_region(
            &app,
            RegionKind::Scrollbar(ScrollbarTarget::MetricsDashboard),
        );
        assert_eq!(first_panel.right(), scrollbar.x);

        app.handle_mouse(
            mouse(
                MouseEventKind::ScrollDown,
                first_panel.x.saturating_add(1),
                first_panel.y.saturating_add(1),
            ),
            None,
        );
        assert_eq!(app.metrics.selected_panel, 3);

        render_app_text_at(&app, 60, 24);
        let first_visible = app.metrics.dashboard_first_visible.get();
        assert!(first_visible > 0);
        assert!(
            (first_visible..first_visible + app.metrics.dashboard_visible_count.get()).contains(&3)
        );
        snapshot_region(&app, RegionKind::MetricsPanel(3));

        let scrollbar = snapshot_region(
            &app,
            RegionKind::Scrollbar(ScrollbarTarget::MetricsDashboard),
        );
        app.handle_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                scrollbar.x,
                scrollbar.bottom().saturating_sub(1),
            ),
            None,
        );
        assert_eq!(app.metrics.selected_panel, 5);

        render_app_text_at(&app, 60, 24);
        snapshot_region(&app, RegionKind::MetricsPanel(5));

        app.metrics.selected_panel = 1;
        app.metrics.dashboard_first_visible.set(0);
        render_app_text_at(&app, 140, 20);
        assert_eq!(app.metrics.dashboard_columns.get(), 2);
        let left_panel = snapshot_region(&app, RegionKind::MetricsPanel(0));
        let right_panel = snapshot_region(&app, RegionKind::MetricsPanel(1));
        assert_eq!(left_panel.y, right_panel.y);
        assert!(left_panel.height >= 6, "{left_panel:?}");
        let scrollbar = snapshot_region(
            &app,
            RegionKind::Scrollbar(ScrollbarTarget::MetricsDashboard),
        );
        assert_eq!(right_panel.right(), scrollbar.x);
        app.handle_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                scrollbar.x,
                scrollbar.bottom().saturating_sub(1),
            ),
            None,
        );
        assert_eq!(app.metrics.selected_panel, 5);
    }

    #[test]
    fn metrics_dashboard_panel_expands_and_inspects_exact_samples() {
        let mut app = test_app();
        app.ops.select(OpsDestination::Metrics);
        app.metrics.dashboard = Some(LocalMetricDashboardResponse {
            source: LocalMetricSource::Agents,
            range: LocalMetricTimeRange {
                from_unix_nanos: 10,
                to_unix_nanos: 20,
            },
            generated_at_unix_nanos: 20,
            freshest_sample_unix_nanos: Some(20),
            stats: Vec::new(),
            panels: vec![LocalMetricDashboardPanel {
                id: "invocation-rate".to_string(),
                title: "Invocation rate".to_string(),
                kind: LocalMetricDashboardPanelKind::TimeSeries,
                series: vec![LocalMetricDashboardSeries {
                    key: "all".to_string(),
                    label: "All invocations".to_string(),
                    unit: "invocations/s".to_string(),
                    samples: vec![
                        LocalMetricSample {
                            timestamp_unix_nanos: 10,
                            value: 1.0,
                            statistic: "rate".to_string(),
                            source_oplog_index: Some(7),
                        },
                        LocalMetricSample {
                            timestamp_unix_nanos: 20,
                            value: 2.0,
                            statistic: "rate".to_string(),
                            source_oplog_index: Some(8),
                        },
                    ],
                    latest_value: Some(2.0),
                    approximate: false,
                }],
                empty_reason: None,
                observed_from_unix_nanos: Some(10),
                observed_to_unix_nanos: Some(20),
                approximate: false,
            }],
        });

        render_app_text_at(&app, 120, 28);
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(
            app.ops.current_route(),
            &OpsRoute::MetricsPanel("invocation-rate".to_string())
        );
        let frame = render_app_text_at(&app, 120, 28);
        assert!(frame.contains("Metrics / Invocation rate"), "{frame}");
        assert!(frame.contains("oplog #8"), "{frame}");

        app.handle_key(modified_key(KeyCode::Left, KeyModifiers::SHIFT));
        let frame = render_app_text_at(&app, 120, 28);
        assert!(frame.contains("oplog #7"), "{frame}");

        let chart = snapshot_region(&app, RegionKind::MetricsChart);
        app.handle_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                chart.x + chart.width.saturating_sub(1),
                chart.y,
            ),
            None,
        );
        assert_eq!(app.metrics.chart_cursor, Some(1));

        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.ops.current_route(), &OpsRoute::Root);
        render_app_text_at(&app, 120, 28);
        let panel = snapshot_region(&app, RegionKind::MetricsPanel(0));
        app.handle_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                panel.x + 1,
                panel.y + 1,
            ),
            None,
        );
        assert!(matches!(
            app.ops.current_route(),
            OpsRoute::MetricsPanel(panel_id) if panel_id == "invocation-rate"
        ));
    }

    #[test]
    fn metrics_live_mode_is_visible_and_modifier_toggleable() {
        let mut app = test_app();
        app.ops.select(OpsDestination::Metrics);

        app.handle_key(modified_key(KeyCode::Char('l'), KeyModifiers::CONTROL));

        assert!(!app.metrics.live);
        let frame = render_app_text_at(&app, 120, 28);
        assert!(frame.contains("[Paused]"), "{frame}");

        app.handle_key(modified_key(KeyCode::Char('l'), KeyModifiers::CONTROL));

        assert!(app.metrics.live);
        let frame = render_app_text_at(&app, 120, 28);
        assert!(frame.contains("[Live]"), "{frame}");
    }

    #[test]
    fn metrics_range_form_applies_absolute_range_and_pauses_live() {
        let mut app = test_app();
        app.ops.select(OpsDestination::Metrics);
        app.metrics.live = true;

        app.handle_key(modified_key(KeyCode::Char('t'), KeyModifiers::CONTROL));

        assert_eq!(app.mode, InteractionMode::MetricsRange);
        let frame = render_app_text_at(&app, 100, 28);
        assert!(frame.contains("Metrics time range"), "{frame}");
        assert!(frame.contains("Preset"), "{frame}");
        assert!(frame.contains("From UTC"), "{frame}");
        assert!(frame.contains("To UTC"), "{frame}");
        assert!(frame.contains("Apply / edit"), "{frame}");
        render_app_text_at(&app, 1, 1);

        let form = app.metrics_range_form.as_mut().unwrap();
        form.custom = true;
        form.from = TextEditState::new("2026-10-09T10:00:00.000Z");
        form.to = TextEditState::new("2026-10-09T11:30:00.000Z");
        app.handle_key(key(KeyCode::Enter));

        let expected =
            parse_metrics_range("2026-10-09T10:00:00.000Z", "2026-10-09T11:30:00.000Z").unwrap();
        assert_eq!(app.metrics.range, MetricsRange::Custom(expected));
        assert!(!app.metrics.live);
        assert_eq!(app.mode, InteractionMode::Normal);
        assert!(app.metrics_range_form.is_none());

        app.handle_key(modified_key(KeyCode::Char('l'), KeyModifiers::CONTROL));
        assert_eq!(app.mode, InteractionMode::MetricsRange);
        assert!(
            app.metrics_range_form
                .as_ref()
                .and_then(|form| form.error.as_deref())
                .is_some_and(|error| error.contains("requires a relative preset"))
        );
    }

    #[test]
    fn metrics_range_form_keeps_invalid_absolute_input_open() {
        let mut app = test_app();
        app.ops.select(OpsDestination::Metrics);
        app.handle_key(modified_key(KeyCode::Char('t'), KeyModifiers::CONTROL));
        let form = app.metrics_range_form.as_mut().unwrap();
        form.custom = true;
        form.from = TextEditState::new("2026-10-09T12:00:00Z");
        form.to = TextEditState::new("2026-10-09T11:00:00Z");

        app.handle_key(key(KeyCode::Enter));

        assert_eq!(app.mode, InteractionMode::MetricsRange);
        assert!(
            app.metrics_range_form
                .as_ref()
                .and_then(|form| form.error.as_deref())
                .is_some_and(|error| error.contains("earlier than To"))
        );
        let frame = render_app_text_at(&app, 100, 28);
        assert!(frame.contains("From must be earlier than To"), "{frame}");
    }

    #[test]
    fn metrics_range_editor_shows_cursor_and_editing_help() {
        let mut app = test_app();
        app.ops.select(OpsDestination::Metrics);
        app.handle_key(modified_key(KeyCode::Char('t'), KeyModifiers::CONTROL));
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Enter));

        assert!(app.metrics_range_form.as_ref().unwrap().editing);
        let (_, cursor) = render_app_text_and_cursor(&app);
        let buffer = render_app_buffer_at(&app, 100, 24);
        assert_ne!((cursor.x, cursor.y), (0, 0));
        assert_eq!(buffer[(cursor.x, cursor.y)].bg, theme().input_background);
        let frame = render_buffer_text(&buffer);
        assert!(frame.contains("enter Done"), "{frame}");
        assert!(frame.contains("esc Cancel edit"), "{frame}");
    }

    #[test]
    fn metrics_range_form_applies_presets_and_custom_range_is_exact() {
        let mut app = test_app();
        app.ops.select(OpsDestination::Metrics);
        app.handle_key(modified_key(KeyCode::Char('t'), KeyModifiers::CONTROL));
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Right));
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.metrics.range, MetricsRange::Hour1);

        let range = LocalMetricTimeRange {
            from_unix_nanos: 123,
            to_unix_nanos: 456,
        };
        assert_eq!(metrics::current_range(MetricsRange::Custom(range)), range);
    }

    #[test]
    fn metrics_explore_marks_and_limits_compatible_series_overlays() {
        let mut app = test_app();
        app.ops.select(OpsDestination::Metrics);
        app.metrics.mode = MetricsMode::Explore;
        app.metrics.focus = MetricsFocus::Series;
        app.metrics.families = vec![LocalMetricFamily {
            name: "golem.agent.counter".to_string(),
            description: "Counter events".to_string(),
            unit: "1".to_string(),
            kind: LocalMetricKind::Counter,
            temporality: LocalMetricTemporality::Delta,
            category: "agent".to_string(),
            series_count: 2,
            latest_value: Some(2.0),
            latest_unix_nanos: Some(20),
        }];
        let series = |key: &str, statistic: &str| LocalMetricSeries {
            series_key: key.to_string(),
            statistic: statistic.to_string(),
            temporality: LocalMetricTemporality::Delta,
            attributes: BTreeMap::new(),
            environment_id: None,
            component_id: None,
            agent_id: Some(key.to_string()),
            agent_type: Some("Counter".to_string()),
            latest_value: Some(2.0),
            latest_unix_nanos: Some(20),
        };
        app.metrics.series = vec![series("agent-a", "value"), series("agent-b", "value")];
        app.metrics.selected_series = 1;
        app.metrics.selected_series_keys = vec!["agent-a".to_string()];
        app.metrics.series_samples = vec![metrics::MetricsSeriesSamples {
            series_key: "agent-a".to_string(),
            response: LocalMetricSamplesResponse {
                samples: vec![LocalMetricSample {
                    timestamp_unix_nanos: 10,
                    value: 1.0,
                    statistic: "value".to_string(),
                    source_oplog_index: Some(7),
                }],
                observed_from_unix_nanos: Some(10),
                observed_to_unix_nanos: Some(10),
                original_point_count: 1,
                approximate: false,
                bucket_width_unix_nanos: None,
            },
        }];

        app.handle_key(modified_key(KeyCode::Char(' '), KeyModifiers::CONTROL));

        assert_eq!(
            app.metrics.selected_series_keys,
            vec!["agent-a".to_string(), "agent-b".to_string()]
        );
        let frame = render_app_text_at(&app, 120, 28);
        assert!(frame.contains("2/6 shown"), "{frame}");
        assert_eq!(frame.matches("[x]").count(), 2, "{frame}");

        app.metrics.series.push(series("sum-series", "sum"));
        app.metrics.selected_series = 2;
        app.handle_key(modified_key(KeyCode::Char(' '), KeyModifiers::CONTROL));

        assert_eq!(app.metrics.selected_series_keys.len(), 2);
        assert!(
            app.metrics
                .notice
                .as_deref()
                .is_some_and(|notice| notice.contains("same statistic and temporality"))
        );
    }

    #[test]
    fn exact_explore_sample_opens_a_retained_oplog_route() {
        let mut app = test_app();
        app.agents.agents = sample_agents();
        app.ops.select(OpsDestination::Metrics);
        app.metrics.mode = MetricsMode::Explore;
        app.metrics.focus = MetricsFocus::Chart;
        app.metrics.series = vec![LocalMetricSeries {
            series_key: "cart-invocations".to_string(),
            statistic: "value".to_string(),
            temporality: LocalMetricTemporality::Delta,
            attributes: BTreeMap::new(),
            environment_id: None,
            component_id: None,
            agent_id: Some("cart-1".to_string()),
            agent_type: Some("CartAgent".to_string()),
            latest_value: Some(1.0),
            latest_unix_nanos: Some(20),
        }];
        app.metrics.series_samples = vec![metrics::MetricsSeriesSamples {
            series_key: "cart-invocations".to_string(),
            response: LocalMetricSamplesResponse {
                samples: vec![LocalMetricSample {
                    timestamp_unix_nanos: 20,
                    value: 1.0,
                    statistic: "value".to_string(),
                    source_oplog_index: Some(73),
                }],
                observed_from_unix_nanos: Some(20),
                observed_to_unix_nanos: Some(20),
                original_point_count: 1,
                approximate: false,
                bucket_width_unix_nanos: None,
            },
        }];

        let frame = render_app_text_at(&app, 140, 32);
        assert!(frame.contains("open  Agent  Oplog  Stream"), "{frame}");
        assert!(
            app.geometry
                .region(RegionKind::OpsLink(OpsDestination::Agents))
                .is_some()
        );
        assert!(
            app.geometry
                .region(RegionKind::OpsLink(OpsDestination::Stream))
                .is_some()
        );
        let oplog = snapshot_region(&app, RegionKind::OpsLink(OpsDestination::Oplog));
        app.handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), oplog.x, oplog.y),
            None,
        );

        assert_eq!(app.ops.active(), OpsDestination::Oplog);
        assert_eq!(
            app.ops.current_route(),
            &OpsRoute::Oplog(OplogRoute {
                agents: vec![AgentIdentity {
                    component: "cart".to_string(),
                    agent_id: "cart-1".to_string(),
                }],
                focus: Some(OplogEntryIdentity {
                    agent: AgentIdentity {
                        component: "cart".to_string(),
                        agent_id: "cart-1".to_string(),
                    },
                    index: 73,
                }),
                from_unix_nanos: None,
                to_unix_nanos: None,
            })
        );
        assert_eq!(app.activity.representation, ActivityRepresentation::Entries);
        assert_eq!(
            app.activity.pending_focus,
            Some((
                ActivityAgent {
                    component: "cart".to_string(),
                    agent_id: "cart-1".to_string(),
                },
                73,
            ))
        );
    }

    #[test]
    fn aggregate_explore_sample_opens_its_scoped_oplog_bucket() {
        let mut app = test_app();
        app.agents.agents = sample_agents();
        app.ops.select(OpsDestination::Metrics);
        app.metrics.mode = MetricsMode::Explore;
        app.metrics.focus = MetricsFocus::Chart;
        app.metrics.time_range = Some(LocalMetricTimeRange {
            from_unix_nanos: 0,
            to_unix_nanos: 100,
        });
        app.metrics.series = vec![LocalMetricSeries {
            series_key: "cart-invocations".to_string(),
            statistic: "value".to_string(),
            temporality: LocalMetricTemporality::Delta,
            attributes: BTreeMap::new(),
            environment_id: None,
            component_id: None,
            agent_id: Some("cart-1".to_string()),
            agent_type: Some("CartAgent".to_string()),
            latest_value: Some(4.0),
            latest_unix_nanos: Some(40),
        }];
        app.metrics.series_samples = vec![metrics::MetricsSeriesSamples {
            series_key: "cart-invocations".to_string(),
            response: LocalMetricSamplesResponse {
                samples: vec![LocalMetricSample {
                    timestamp_unix_nanos: 40,
                    value: 4.0,
                    statistic: "value".to_string(),
                    source_oplog_index: None,
                }],
                observed_from_unix_nanos: Some(30),
                observed_to_unix_nanos: Some(45),
                original_point_count: 12,
                approximate: true,
                bucket_width_unix_nanos: Some(25),
            },
        }];

        app.handle_key(key(KeyCode::Enter));

        assert_eq!(app.ops.active(), OpsDestination::Oplog);
        assert_eq!(
            app.ops.current_route(),
            &OpsRoute::Oplog(OplogRoute {
                agents: vec![AgentIdentity {
                    component: "cart".to_string(),
                    agent_id: "cart-1".to_string(),
                }],
                focus: None,
                from_unix_nanos: Some(25),
                to_unix_nanos: Some(50),
            })
        );
        assert_eq!(app.activity.representation, ActivityRepresentation::Grouped);
        assert_eq!(
            app.activity.applied_query.from,
            datetime_from_unix_nanos(25)
        );
        assert_eq!(app.activity.applied_query.to, datetime_from_unix_nanos(50));
    }

    #[test]
    fn aggregate_metric_bucket_is_clamped_to_the_loaded_range() {
        assert_eq!(
            metric_bucket_range(
                LocalMetricTimeRange {
                    from_unix_nanos: 10,
                    to_unix_nanos: 50,
                },
                16,
                49,
            ),
            Some(LocalMetricTimeRange {
                from_unix_nanos: 42,
                to_unix_nanos: 50,
            })
        );
        assert_eq!(
            metric_bucket_range(
                LocalMetricTimeRange {
                    from_unix_nanos: 10,
                    to_unix_nanos: 50,
                },
                0,
                20,
            ),
            None
        );
    }

    #[test]
    fn metrics_append_pages_preserve_selection_and_deduplicate_rows() {
        let family = |name: &str| LocalMetricFamily {
            name: name.to_string(),
            description: String::new(),
            unit: "1".to_string(),
            kind: LocalMetricKind::Counter,
            temporality: LocalMetricTemporality::Delta,
            category: "agent".to_string(),
            series_count: 1,
            latest_value: Some(1.0),
            latest_unix_nanos: Some(10),
        };
        let series = |key: &str| LocalMetricSeries {
            series_key: key.to_string(),
            statistic: "value".to_string(),
            temporality: LocalMetricTemporality::Delta,
            attributes: BTreeMap::new(),
            environment_id: None,
            component_id: None,
            agent_id: Some(key.to_string()),
            agent_type: None,
            latest_value: Some(1.0),
            latest_unix_nanos: Some(10),
        };
        let capability = || LocalObservabilityCapability {
            status: LocalObservabilityStatus::Ready,
            message: None,
            exporter_name: "test".to_string(),
            exporter_version: "1".to_string(),
            sample_interval_seconds: 5,
            retention_days: 7,
            retention_bytes: 1024,
            discarded_points: 0,
            last_service_sample_unix_nanos: Some(10),
            last_agent_ingest_unix_nanos: Some(10),
        };
        let mut app = test_app();
        app.metrics.generation = 3;
        app.metrics.context_id = Some(TuiContextId::new(9));
        app.metrics.loading = true;
        app.metrics.families = vec![family("a")];
        app.metrics.series = vec![series("one")];

        app.finish_metrics_refresh(
            3,
            TuiContextTaskResult::new(
                TuiContextId::new(9),
                Ok(MetricsRefreshPayload {
                    capability: capability(),
                    source: LocalMetricSource::Agents,
                    mode: MetricsMode::Explore,
                    load: MetricsLoad::AppendFamilies,
                    time_range: LocalMetricTimeRange {
                        from_unix_nanos: 0,
                        to_unix_nanos: 10,
                    },
                    dashboard: None,
                    families: vec![family("a"), family("b")],
                    series: Vec::new(),
                    samples: Vec::new(),
                    series_samples: Vec::new(),
                    selected_series_keys: Vec::new(),
                    families_next_cursor: Some("family-next".to_string()),
                    series_next_cursor: None,
                    selected_family: 0,
                    selected_series: 0,
                }),
                Vec::new(),
            ),
        );

        assert_eq!(
            app.metrics
                .families
                .iter()
                .map(|family| family.name.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "b"]
        );
        assert_eq!(
            app.metrics.families_next_cursor.as_deref(),
            Some("family-next")
        );
        assert_eq!(app.metrics.selected_family, 0);

        app.finish_metrics_refresh(
            3,
            TuiContextTaskResult::new(
                TuiContextId::new(9),
                Ok(MetricsRefreshPayload {
                    capability: capability(),
                    source: LocalMetricSource::Agents,
                    mode: MetricsMode::Explore,
                    load: MetricsLoad::AppendSeries,
                    time_range: LocalMetricTimeRange {
                        from_unix_nanos: 0,
                        to_unix_nanos: 10,
                    },
                    dashboard: None,
                    families: Vec::new(),
                    series: vec![series("one"), series("two")],
                    samples: Vec::new(),
                    series_samples: Vec::new(),
                    selected_series_keys: Vec::new(),
                    families_next_cursor: None,
                    series_next_cursor: Some("series-next".to_string()),
                    selected_family: 0,
                    selected_series: 0,
                }),
                Vec::new(),
            ),
        );

        assert_eq!(
            app.metrics
                .series
                .iter()
                .map(|series| series.series_key.as_str())
                .collect::<Vec<_>>(),
            vec!["one", "two"]
        );
        assert_eq!(
            app.metrics.series_next_cursor.as_deref(),
            Some("series-next")
        );
        assert_eq!(app.metrics.selected_series, 1);
    }

    #[test]
    fn empty_metrics_view_does_not_invent_focusable_series() {
        let mut app = test_app();
        app.ops.select(OpsDestination::Metrics);
        render_app_text_at(&app, 120, 16);

        app.handle_key(key(KeyCode::Down));

        assert_eq!(app.ops.active(), OpsDestination::Metrics);
        let frame = render_app_text_at(&app, 120, 16);
        assert!(!frame.contains("golem.agent.invocations"), "{frame}");
    }

    #[test]
    fn ops_control_strips_and_compare_fields_are_pointer_addressable() {
        let mut app = test_app();
        app.ops.select(OpsDestination::Oplog);
        render_app_text_at(&app, 160, 28);
        let entries = snapshot_region(
            &app,
            RegionKind::OpsControl {
                group: 0,
                choice: 1,
            },
        );
        app.handle_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                entries.x,
                entries.y,
            ),
            None,
        );
        assert_eq!(app.activity.representation, ActivityRepresentation::Entries);

        app.ops.select(OpsDestination::Metrics);
        render_app_text_at(&app, 160, 28);
        let explore = snapshot_region(
            &app,
            RegionKind::OpsControl {
                group: 0,
                choice: 1,
            },
        );
        app.handle_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                explore.x,
                explore.y,
            ),
            None,
        );
        assert_eq!(app.metrics.mode, MetricsMode::Explore);
        render_app_text_at(&app, 160, 28);
        let server = snapshot_region(
            &app,
            RegionKind::OpsControl {
                group: 1,
                choice: 1,
            },
        );
        app.handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), server.x, server.y),
            None,
        );
        assert_eq!(app.metrics.source, LocalMetricSource::Services);

        app.ops.select(OpsDestination::Configuration);
        render_app_text_at(&app, 160, 28);
        let account = snapshot_region(
            &app,
            RegionKind::OpsControl {
                group: 0,
                choice: 1,
            },
        );
        app.handle_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                account.x,
                account.y,
            ),
            None,
        );
        assert_eq!(app.resources.scope, ResourceScope::Account);

        app.ops.open_deployment_compare(1, 2);
        render_app_text_at(&app, 160, 28);
        let target = snapshot_region(
            &app,
            RegionKind::OpsControl {
                group: 0,
                choice: 1,
            },
        );
        app.handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), target.x, target.y),
            None,
        );
        assert!(matches!(
            app.ops.current_route(),
            OpsRoute::DeploymentCompare(route)
                if route.focus == DeploymentCompareFocus::Target
        ));
    }

    #[test]
    fn metrics_explore_lists_support_click_wheel_and_scrollbar_selection() {
        let mut app = test_app();
        app.ops.select(OpsDestination::Metrics);
        app.metrics.mode = MetricsMode::Explore;
        app.metrics.families = (0..20)
            .map(|index| LocalMetricFamily {
                name: format!("family-{index:02}"),
                description: String::new(),
                unit: "1".to_string(),
                kind: LocalMetricKind::Counter,
                temporality: LocalMetricTemporality::Delta,
                category: "agent".to_string(),
                series_count: 20,
                latest_value: Some(index as f64),
                latest_unix_nanos: Some(index),
            })
            .collect();
        let series = || {
            (0..20)
                .map(|index| LocalMetricSeries {
                    series_key: format!("series-{index:02}"),
                    statistic: "value".to_string(),
                    temporality: LocalMetricTemporality::Delta,
                    attributes: BTreeMap::new(),
                    environment_id: None,
                    component_id: None,
                    agent_id: Some(format!("agent-{index:02}")),
                    agent_type: Some("Counter".to_string()),
                    latest_value: Some(index as f64),
                    latest_unix_nanos: Some(index),
                })
                .collect::<Vec<_>>()
        };
        app.metrics.series = series();

        render_app_text_at(&app, 140, 14);
        let families = snapshot_region(&app, RegionKind::MetricsFamilies);
        app.handle_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                families.x,
                families.y.saturating_add(1),
            ),
            None,
        );
        assert_eq!(app.metrics.focus, MetricsFocus::Families);
        assert_eq!(app.metrics.selected_family, 1);

        app.metrics.series = series();
        render_app_text_at(&app, 140, 14);
        let series_area = snapshot_region(&app, RegionKind::MetricsSeries);
        let toggle = snapshot_region(&app, RegionKind::MetricsSeriesToggle(1));
        app.handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), toggle.x, toggle.y),
            None,
        );
        assert_eq!(app.metrics.focus, MetricsFocus::Series);
        assert_eq!(app.metrics.selected_series, 1);
        assert_eq!(
            app.metrics.selected_series_keys,
            vec!["series-01".to_string()]
        );
        app.handle_mouse(
            mouse(MouseEventKind::ScrollDown, series_area.x, series_area.y),
            None,
        );
        assert_eq!(app.metrics.selected_series, 4);

        let scrollbar =
            snapshot_region(&app, RegionKind::Scrollbar(ScrollbarTarget::MetricsSeries));
        app.handle_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                scrollbar.x,
                scrollbar.bottom().saturating_sub(1),
            ),
            None,
        );
        assert!(app.metrics.selected_series >= 18);
    }

    #[test]
    fn ops_destination_cycle_is_stable() {
        let mut app = test_app();
        assert_eq!(app.ops.active().next(), OpsDestination::Oplog);
        app.ops.select(OpsDestination::Configuration);
        assert_eq!(app.ops.active().next(), OpsDestination::Overview);
        assert_eq!(app.ops.active().previous(), OpsDestination::Deployments);
    }

    #[test]
    fn resources_render_as_kind_items_and_safe_details_at_all_sizes() {
        let mut app = test_app();
        app.ops.select(OpsDestination::Configuration);
        let context_id = TuiContextId::new(7);
        let (generation, scope, kind) = app.resources.begin_load(context_id);
        app.resources.finish(
            generation,
            context_id,
            Ok(ResourcesRefreshPayload {
                scope,
                kind,
                rows: vec![resources::ResourceRow {
                    name: "/payments/token".to_string(),
                    summary: "revision 7".to_string(),
                    fields: vec![resources::ResourceField {
                        label: "Value".to_string(),
                        value: "Masked".to_string(),
                    }],
                }],
            }),
        );

        let frame = render_app_text_at(&app, 120, 30);
        assert!(
            frame.contains("Scope  [Environment] Account Server"),
            "{frame}"
        );
        assert!(frame.contains("Secrets"), "{frame}");
        assert!(frame.contains("/payments/token"), "{frame}");
        assert!(frame.contains("revision 7"), "{frame}");

        for (scope, expected) in [
            (ResourceScope::Environment, "[Environment]"),
            (ResourceScope::Account, "[Account]"),
            (ResourceScope::Server, "[Server]"),
        ] {
            app.resources.select_scope(scope);
            if !app.resources.is_loaded(app.resources.selected_kind()) {
                let (generation, scope, kind) = app.resources.begin_load(context_id);
                app.resources.finish(
                    generation,
                    context_id,
                    Ok(ResourcesRefreshPayload {
                        scope,
                        kind,
                        rows: vec![resources::ResourceRow {
                            name: expected.to_string(),
                            summary: "typed projection".to_string(),
                            fields: vec![resources::ResourceField {
                                label: "Status".to_string(),
                                value: "available".to_string(),
                            }],
                        }],
                    }),
                );
            }
            let frame = render_app_text_at(&app, 120, 30);
            assert!(frame.contains(expected), "{frame}");
            for (width, height) in [(100, 12), (60, 18), (24, 8), (1, 1)] {
                render_app_text_at(&app, width, height);
            }
        }
    }

    #[test]
    fn configuration_keys_switch_scopes_and_cycle_all_three_panes() {
        let mut app = test_app();
        app.active_workspace = Workspace::Ops;
        app.ops.select(OpsDestination::Configuration);

        app.handle_key(key(KeyCode::Right));
        assert_eq!(app.resources.scope, ResourceScope::Account);
        app.handle_key(key(KeyCode::Left));
        assert_eq!(app.resources.scope, ResourceScope::Environment);

        assert_eq!(app.resources.focus(), resources::ResourceFocus::Kinds);
        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.resources.focus(), resources::ResourceFocus::Items);
        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.resources.focus(), resources::ResourceFocus::Details);
        app.handle_key(key(KeyCode::BackTab));
        assert_eq!(app.resources.focus(), resources::ResourceFocus::Items);
    }

    #[test]
    fn configuration_panes_support_click_wheel_and_scrollbar_interaction() {
        let mut app = test_app();
        app.active_workspace = Workspace::Ops;
        app.ops.select(OpsDestination::Configuration);
        let context_id = TuiContextId::new(7);
        let (generation, scope, kind) = app.resources.begin_load(context_id);
        app.resources.finish(
            generation,
            context_id,
            Ok(ResourcesRefreshPayload {
                scope,
                kind,
                rows: (0..20)
                    .map(|index| resources::ResourceRow {
                        name: format!("secret-{index:02}"),
                        summary: "masked".to_string(),
                        fields: (0..20)
                            .map(|field| resources::ResourceField {
                                label: format!("Field {field:02}"),
                                value: format!("value-{index:02}-{field:02}"),
                            })
                            .collect(),
                    })
                    .collect(),
            }),
        );

        render_app_text_at(&app, 140, 14);
        let items = snapshot_region(&app, RegionKind::ResourceItems);
        app.handle_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                items.x,
                items.y.saturating_add(1),
            ),
            None,
        );
        assert_eq!(app.resources.focus(), resources::ResourceFocus::Items);
        assert_eq!(app.resources.selected_row_index(), 1);
        app.handle_mouse(mouse(MouseEventKind::ScrollDown, items.x, items.y), None);
        assert_eq!(app.resources.selected_row_index(), 4);

        let item_scrollbar =
            snapshot_region(&app, RegionKind::Scrollbar(ScrollbarTarget::ResourceItems));
        app.handle_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                item_scrollbar.x,
                item_scrollbar.bottom().saturating_sub(1),
            ),
            None,
        );
        assert!(app.resources.selected_row_index() >= 18);

        render_app_text_at(&app, 140, 14);
        let details = snapshot_region(&app, RegionKind::ResourceDetails);
        app.handle_mouse(
            mouse(MouseEventKind::ScrollDown, details.x, details.y),
            None,
        );
        assert_eq!(app.resources.focus(), resources::ResourceFocus::Details);
        assert!(app.resources.details_scroll() > 0);
        let details_scrollbar = snapshot_region(
            &app,
            RegionKind::Scrollbar(ScrollbarTarget::ResourceDetails),
        );
        app.handle_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                details_scrollbar.x,
                details_scrollbar.bottom().saturating_sub(1),
            ),
            None,
        );
        assert_eq!(
            app.resources.details_scroll() as usize,
            app.resources.details_max_scroll()
        );

        render_app_text_at(&app, 140, 14);
        let kinds = snapshot_region(&app, RegionKind::ResourceKinds);
        app.handle_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                kinds.x,
                kinds.y.saturating_add(1),
            ),
            None,
        );
        assert_eq!(app.resources.focus(), resources::ResourceFocus::Kinds);
        assert_eq!(app.resources.selected_kind_index(), 1);
    }

    #[test]
    fn stream_renders_typed_events_and_background_unread_state_responsively() {
        let mut app = test_app();
        let target = AgentIdentity {
            component: "cart".to_string(),
            agent_id: "CartAgent(\"one\")".to_string(),
        };
        app.ops.open_stream(target.clone());
        app.stream.target = Some(target);
        app.stream.status = StreamStatus::Connected;
        app.stream.apply_message(
            0,
            stream::StreamMessage::Item(AgentStreamItem::PluginError {
                timestamp: golem_common::model::Timestamp::now_utc(),
                plugin_name: "demo-plugin".to_string(),
                message: "plugin failed".to_string(),
            }),
            true,
        );
        app.stream.apply_message(
            0,
            stream::StreamMessage::Item(AgentStreamItem::SnapshotRecoverySucceeded {
                timestamp: golem_common::model::Timestamp::now_utc(),
                snapshot_index: 42,
            }),
            false,
        );

        let frame = render_app_text_at(&app, 120, 28);
        assert!(frame.contains("[Connected]"), "{frame}");
        assert!(frame.contains("1 unread"), "{frame}");
        assert!(frame.contains("PLUGIN"), "{frame}");
        assert!(frame.contains("demo-plugin · plugin failed"), "{frame}");
        assert!(frame.contains("RECOVER"), "{frame}");
        assert!(frame.contains("oplog index 42"), "{frame}");

        for (width, height) in [(60, 18), (24, 8), (1, 1)] {
            render_app_text_at(&app, width, height);
        }
    }

    #[test]
    fn stream_scrollbar_and_destination_switch_preserve_the_session() {
        let mut app = test_app();
        let target = AgentIdentity {
            component: "cart".to_string(),
            agent_id: "CartAgent(\"one\")".to_string(),
        };
        app.ops.open_stream(target.clone());
        app.stream.target = Some(target.clone());
        app.stream.status = StreamStatus::Connected;
        for index in 0..40 {
            app.stream.apply_message(
                0,
                stream::StreamMessage::Item(AgentStreamItem::Stdout {
                    timestamp: golem_common::model::Timestamp::from(index),
                    message: format!("event {index}"),
                }),
                true,
            );
        }
        render_app_text_at(&app, 80, 14);
        let scrollbar = snapshot_region(&app, RegionKind::Scrollbar(ScrollbarTarget::OpsStream));
        app.handle_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                scrollbar.x,
                scrollbar.y,
            ),
            None,
        );
        assert!(!app.stream.follow);
        assert!(app.stream.scroll_from_end > 0);

        app.select_ops_destination(OpsDestination::Agents, None);
        assert_eq!(app.stream.status, StreamStatus::Connected);
        assert_eq!(app.stream.target, Some(target));
    }

    #[test]
    fn oplog_representation_switches_between_grouped_and_entries() {
        let mut app = test_app();
        app.agents.agents = sample_agents();
        app.ops.select(OpsDestination::Oplog);

        app.set_activity_representation(ActivityRepresentation::Entries, None);

        assert_eq!(app.activity.representation, ActivityRepresentation::Entries);
        assert!(!app.activity.loading);
    }

    #[test]
    fn oplog_typed_groups_progress_and_details_render_responsively() {
        let mut app = test_app();
        app.ops.select(OpsDestination::Oplog);
        app.activity.agents = vec![ActivityAgent {
            component: "cart".to_string(),
            agent_id: "cart-1".to_string(),
        }];
        let entry = |index, kind: &str| activity::ActivityEntry {
            component: "cart".to_string(),
            agent_id: "cart-1".to_string(),
            presentation: crate::model::agent::oplog::OplogEntryPresentation {
                index,
                timestamp: golem_common::model::Timestamp::from(index),
                kind: kind.to_string(),
                group: Some(crate::model::agent::oplog::OplogEntryGroupKey::OplogIndex(
                    7,
                )),
                detail: serde_json::json!({"type": kind, "index": index}),
                attribution: None,
                entry: None,
                search_text: std::sync::OnceLock::new(),
            },
        };
        app.activity.entries = vec![entry(9, "End"), entry(7, "Start")];
        app.activity.progress = vec![activity::ActivityAgentProgress {
            agent: app.activity.agents[0].clone(),
            next: Some(golem_common::model::oplog::OplogCursor {
                next_oplog_index: 10,
                current_component_revision: 1,
            }),
            scanned_through_index: Some(9),
            last_index: 20,
            oldest_loaded_index: Some(7),
            has_more: true,
            scan_limit_reached: true,
        }];
        app.activity.paused = true;

        let grouped = render_app_text_at(&app, 120, 28);
        assert!(grouped.contains("Scan #9/#20   bounded"), "{grouped}");
        assert!(grouped.contains("#00007"), "{grouped}");
        assert!(grouped.contains("End → Start · 2×"), "{grouped}");

        app.activity.representation = ActivityRepresentation::Entries;
        let entries = render_app_text_at(&app, 120, 28);
        assert!(entries.contains("#00009"), "{entries}");
        assert!(entries.contains("\"type\": \"End\""), "{entries}");

        app.handle_key(modified_key(KeyCode::Char('o'), KeyModifiers::CONTROL));
        assert_eq!(app.activity.order, ActivityOrder::Oldest);
        assert_eq!(app.activity.entries[0].presentation.index, 7);
        assert_eq!(
            app.activity.entries[app.activity.selected]
                .presentation
                .index,
            9
        );

        for (width, height) in [(60, 18), (24, 8), (1, 1)] {
            render_app_text_at(&app, width, height);
        }
    }

    #[test]
    fn oplog_master_details_share_focus_scrolling_and_pointer_ownership() {
        let mut app = test_app();
        app.ops.select(OpsDestination::Oplog);
        app.activity.representation = ActivityRepresentation::Entries;
        app.activity.entries = sample_activity_entries(40);

        let frame = render_app_text_at(&app, 120, 20);
        assert!(frame.contains("Entries"), "{frame}");
        assert!(frame.contains("Entry details"), "{frame}");
        assert!(app.geometry.region(RegionKind::OpsDetailsSplit).is_some());

        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.activity.focus, ActivityFocus::Details);
        app.handle_key(key(KeyCode::PageDown));
        assert!(app.activity.details_scroll > 0);

        let details = snapshot_region(&app, RegionKind::OpsDetails);
        let before_wheel = app.activity.details_scroll;
        app.handle_mouse(
            mouse(
                MouseEventKind::ScrollDown,
                details.x.saturating_add(2),
                details.y.saturating_add(2),
            ),
            None,
        );
        assert!(app.activity.details_scroll >= before_wheel);

        let scrollbar = snapshot_region(&app, RegionKind::Scrollbar(ScrollbarTarget::OpsDetails));
        app.handle_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                scrollbar.x,
                scrollbar.bottom().saturating_sub(1),
            ),
            None,
        );
        assert_eq!(app.activity.focus, ActivityFocus::Details);
        assert!(app.activity.details_scroll > 0);
    }

    #[test]
    fn oplog_row_click_preserves_the_visible_window() {
        let mut app = test_app();
        app.ops.select(OpsDestination::Oplog);
        app.activity.representation = ActivityRepresentation::Entries;
        app.activity.entries = sample_activity_entries(40);
        render_app_text_at(&app, 120, 18);
        app.move_activity_selection_by(15, true);
        render_app_text_at(&app, 120, 18);
        let first = app.activity.list_first_visible.get();
        assert!(first > 0);
        let list = snapshot_region(&app, RegionKind::OpsList);

        app.handle_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                list.x.saturating_add(3),
                list.y.saturating_add(2),
            ),
            None,
        );
        render_app_text_at(&app, 120, 18);

        assert_eq!(app.activity.selected, first + 1);
        assert_eq!(app.activity.list_first_visible.get(), first);
        assert_eq!(app.activity.focus, ActivityFocus::List);
    }

    #[test]
    fn oplog_details_collapse_and_layout_controls_follow_shared_ops_rules() {
        let mut app = test_app();
        app.ops.select(OpsDestination::Oplog);
        app.activity.entries = sample_activity_entries(3);

        let narrow = render_app_text_at(&app, 55, 22);
        assert!(narrow.contains("Journal"), "{narrow}");
        assert!(app.geometry.region(RegionKind::OpsDetails).is_none());
        assert!(app.geometry.region(RegionKind::OpsDetailsSplit).is_none());

        render_app_text_at(&app, 120, 30);
        let old_ratio = app.layout.ops_details_ratio;
        app.mode.replace(InteractionMode::Layout);
        app.handle_key(key(KeyCode::Right));
        assert!(app.layout.ops_details_ratio > old_ratio);
        app.handle_key(key(KeyCode::Char('d')));
        assert!(!app.activity.details_visible);
        render_app_text_at(&app, 120, 30);
        assert!(app.geometry.region(RegionKind::OpsDetails).is_none());
    }

    #[test]
    fn oplog_server_search_edits_a_draft_and_enter_applies_it() {
        let mut app = test_app();
        app.agents.agents = sample_agents();
        let agent = AgentIdentity {
            component: "cart".to_string(),
            agent_id: "cart-1".to_string(),
        };
        app.ops.open_oplog(OplogRoute {
            agents: vec![agent.clone()],
            focus: Some(OplogEntryIdentity { agent, index: 73 }),
            from_unix_nanos: None,
            to_unix_nanos: None,
        });
        app.handle_key(modified_key(KeyCode::Char('s'), KeyModifiers::CONTROL));

        for character in "failed invocation".chars() {
            app.handle_key(key(KeyCode::Char(character)));
        }
        assert_eq!(app.mode, InteractionMode::OplogFilter);
        assert_eq!(app.activity.query, "failed invocation");
        assert!(app.activity.applied_query.text.is_empty());

        app.handle_key(key(KeyCode::Enter));

        assert_eq!(app.mode, InteractionMode::Normal);
        assert_eq!(app.activity.applied_query.text, "failed invocation");
        assert!(
            app.ops
                .current_oplog()
                .is_some_and(|route| route.focus.is_none())
        );
        let frame = render_app_text_at(&app, 120, 28);
        assert!(frame.contains("failed invocation"), "{frame}");
        assert!(frame.contains("applied"), "{frame}");
    }

    #[test]
    fn active_search_surfaces_show_a_native_cursor() {
        let assert_cursor = |app: &TuiApp| {
            let (_, cursor) = render_app_text_and_cursor(app);
            let buffer = render_app_buffer_at(app, 100, 24);
            assert_ne!((cursor.x, cursor.y), (0, 0));
            assert_eq!(buffer[(cursor.x, cursor.y)].bg, theme().input_background);
        };

        let mut palette = test_app();
        palette.handle_key(modified_key(KeyCode::Char('p'), KeyModifiers::CONTROL));
        assert_cursor(&palette);

        let mut agents = test_app();
        agents.agents.agents = sample_agents();
        agents.handle_key(key(KeyCode::Char('c')));
        assert_cursor(&agents);

        let mut dataset = test_app();
        dataset.agents.agents = sample_agents();
        dataset.handle_key(modified_key(KeyCode::Char('f'), KeyModifiers::CONTROL));
        assert_cursor(&dataset);

        let mut oplog = test_app();
        oplog.ops.select(OpsDestination::Oplog);
        oplog.handle_key(key(KeyCode::Char('f')));
        assert_cursor(&oplog);
    }

    #[test]
    fn escape_discards_the_oplog_search_draft() {
        let mut app = test_app();
        app.ops.select(OpsDestination::Oplog);
        app.activity.applied_query.text = "applied".to_string();
        app.activity.query = "applied".to_string();
        app.activity.input_kind = ActivityInputKind::Server;

        app.handle_key(key(KeyCode::Char('!')));
        assert_eq!(app.mode, InteractionMode::OplogFilter);
        assert_eq!(app.activity.query, "applied!");

        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.mode, InteractionMode::Normal);
        assert_eq!(app.activity.query, "applied");
    }

    #[test]
    fn oplog_plain_text_filters_only_the_loaded_entries_by_default() {
        let mut app = test_app();
        app.ops.select(OpsDestination::Oplog);
        app.activity.entries = sample_activity_entries(3);

        for character in "cart-01".chars() {
            app.handle_key(key(KeyCode::Char(character)));
        }

        assert_eq!(app.activity.input_kind, ActivityInputKind::Loaded);
        assert_eq!(app.activity.loaded_filter, "cart-01");
        assert!(app.activity.applied_query.text.is_empty());
        assert_eq!(app.activity_visible_len(), 1);
        let frame = render_app_text_at(&app, 120, 28);
        assert!(frame.contains("Matches 1/3 loaded"), "{frame}");
        assert!(frame.contains("loaded only"), "{frame}");
    }

    #[test]
    fn oplog_identity_density_adapts_to_single_and_multi_agent_scopes() {
        let mut app = test_app();
        app.ops.select(OpsDestination::Oplog);
        app.activity.details_visible = false;
        let long_id =
            "CheckoutAgent(\"tenant-with-a-very-long-prefix/order-with-a-distinct-tail-9472\")";
        let mut first = sample_activity_entries(1).remove(0);
        first.component = "checkout".to_string();
        first.agent_id = long_id.to_string();
        app.activity.agents = vec![ActivityAgent {
            component: first.component.clone(),
            agent_id: first.agent_id.clone(),
        }];
        app.activity.entries = vec![first.clone()];

        let single = render_app_text_at(&app, 140, 24);
        assert!(!single.contains(long_id), "{single}");
        assert!(single.contains("…"), "{single}");

        let mut second = first;
        second.agent_id =
            "CheckoutAgent(\"tenant-with-a-very-long-prefix/order-with-a-distinct-tail-1188\")"
                .to_string();
        second.presentation.index = 2;
        app.activity.agents.push(ActivityAgent {
            component: second.component.clone(),
            agent_id: second.agent_id.clone(),
        });
        app.activity.entries.push(second);

        let multi = render_app_text_at(&app, 140, 24);
        assert!(multi.contains("2 Agents"), "{multi}");
        assert!(multi.contains("…"), "{multi}");
        assert!(multi.contains("9472\")"), "{multi}");
        assert!(multi.contains("1188\")"), "{multi}");
    }

    #[test]
    fn oplog_loaded_map_moves_the_visible_selection() {
        let mut app = test_app();
        app.ops.select(OpsDestination::Oplog);
        app.activity.entries = sample_activity_entries(12);

        render_app_text_at(&app, 120, 24);
        let map = snapshot_region(&app, RegionKind::OplogMap);
        app.handle_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                map.right().saturating_sub(1),
                map.y,
            ),
            None,
        );

        assert_eq!(app.activity.selected, 11);
        assert_eq!(app.activity.focus, ActivityFocus::List);
    }

    #[test]
    async fn stopping_oplog_aborts_the_active_query_task() {
        struct DropFlag(Arc<AtomicBool>);

        impl Drop for DropFlag {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let mut app = test_app();
        let dropped = Arc::new(AtomicBool::new(false));
        let guard = DropFlag(dropped.clone());
        app.activity.task = Some(tokio::spawn(async move {
            let _guard = guard;
            std::future::pending::<()>().await;
        }));
        app.activity.loading = true;

        app.stop_activity();
        tokio::time::timeout(Duration::from_secs(1), async {
            while !dropped.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("aborted query task should be dropped");

        assert!(!app.activity.loading);
        assert!(app.activity.stopped);
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

        app.mode.replace(InteractionMode::AgentFilter);
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

        app.execute_action(ActionId::OpenAgentColumns, None);
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Char(' ')));
        app.handle_key(key(KeyCode::Esc));
        assert!(app.agents.table.column_visible("type"));

        app.execute_action(ActionId::OpenAgentColumns, None);
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
        app.active_workspace = Workspace::Ops;
        app.agents.query = "cart".to_string();
        app.agents.selected = 3;

        app.execute_action(ActionId::CycleAgentMode, None);

        assert_eq!(app.agents.mode, AgentModeFilter::Ephemeral);
        assert_eq!(app.agents.query, "cart");
        assert_eq!(app.agents.selected, 0);
        let frame = render_app_text_at(&app, 120, 30);
        assert!(frame.contains("Mode  ephemeral"), "{frame}");
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
        app.active_workspace = Workspace::Ops;
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
    fn enter_opens_agent_route_and_escape_unwinds() {
        let mut app = test_app();
        app.active_workspace = Workspace::Ops;
        app.agents.agents = sample_agents();

        app.handle_key(key(KeyCode::Enter));
        assert!(matches!(app.ops.current_route(), OpsRoute::Agent(_)));

        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.ops.current_route(), &OpsRoute::Root);
    }

    #[test]
    fn narrow_agent_view_keeps_keyboard_focus_on_the_visible_list() {
        let mut app = test_app();
        app.active_workspace = Workspace::Ops;
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
                .deployment
                .as_ref()
                .expect("retained deployment")
                .deployment_revision,
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
                .deployment
                .as_ref()
                .expect("retained deployment")
                .deployment_revision,
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

        app.handle_palette_key(key(KeyCode::Right), None);

        assert_eq!(app.mode, InteractionMode::Palette);
        assert_eq!(app.palette.scope, PaletteScope::Context);
        assert_eq!(app.palette.query, "agent");

        app.handle_palette_key(key(KeyCode::Left), None);
        assert_eq!(app.palette.scope, PaletteScope::All);
        assert_eq!(app.palette.query, "agent");
    }

    #[test]
    fn palette_scope_tabs_are_mouse_selectable() {
        let mut app = test_app();
        app.open_palette(PaletteScope::All);
        render_app_text_at(&app, 120, 32);
        let context = snapshot_region(&app, RegionKind::PaletteContext);

        app.handle_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                context.x,
                context.y,
            ),
            None,
        );

        assert_eq!(app.palette.scope, PaletteScope::Context);
        assert_eq!(app.mode, InteractionMode::Palette);
    }

    #[test]
    fn command_palette_remembers_scope_for_the_tui_session() {
        let mut app = test_app();
        app.open_palette(PaletteScope::Context);
        app.handle_palette_key(key(KeyCode::Esc), None);

        app.handle_key(modified_key(KeyCode::Char('p'), KeyModifiers::CONTROL));

        assert_eq!(app.mode, InteractionMode::Palette);
        assert_eq!(app.palette.scope, PaletteScope::Context);

        app.handle_palette_key(key(KeyCode::Esc), None);
        app.palette.scope = PaletteScope::All;
        app.handle_key(modified_key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        app.handle_key(key(KeyCode::Char('a')));

        assert_eq!(app.mode, InteractionMode::Palette);
        assert_eq!(app.palette.scope, PaletteScope::Context);
    }

    #[test]
    fn palette_ranking_prefers_exact_and_path_labels_in_registry_order() {
        let app = test_app();
        let matches = ranked_palette_actions("ops", PaletteScope::All, &app);
        let ids = matches
            .iter()
            .map(|matched| matched.action.id)
            .collect::<Vec<_>>();

        assert_eq!(ids.first(), Some(&ActionId::SelectOps));
        assert_eq!(
            &ids[1..5],
            &[
                ActionId::SelectOpsOverview,
                ActionId::SelectOpsAgents,
                ActionId::SelectOpsOplog,
                ActionId::SelectOpsStream,
            ]
        );
        assert_eq!(matches[0].rank, 0);
        assert!(matches[1..5].iter().all(|matched| matched.rank == 1));
    }

    #[test]
    fn typed_palette_matches_are_visually_highlighted() {
        let mut app = test_app();
        app.open_palette(PaletteScope::All);
        for character in "ops".chars() {
            app.handle_palette_key(key(KeyCode::Char(character)), None);
        }

        let buffer = render_app_buffer_at(&app, 120, 32);
        let visual = theme();
        assert!(
            (0..buffer.area.height).any(|y| (0..buffer.area.width)
                .any(|x| buffer[(x, y)].bg == visual.search_match_background)),
            "expected visible palette match highlighting"
        );
    }

    #[test]
    fn command_palette_uses_compact_visible_metadata() {
        let mut app = test_app();
        app.active_workspace = Workspace::Dev;
        app.dev_focus = DevView::Server;
        app.open_palette(PaletteScope::All);

        let frame = render_app_text_at(&app, 140, 40);

        assert!(frame.contains("Move Current View to Main"), "{frame}");
        assert!(
            frame.contains("Promote without hiding other views"),
            "{frame}"
        );
        assert!(!frame.contains("read-only"), "{frame}");
        assert!(!frame.contains("target:"), "{frame}");
        assert!(!frame.contains("TUI session"), "{frame}");
    }

    #[test]
    fn leader_replaces_every_footer_shortcut_row() {
        let mut app = test_app();
        app.handle_key(modified_key(KeyCode::Char('x'), KeyModifiers::CONTROL));

        let frame = render_app_text_at(&app, 120, 30);

        assert!(frame.contains("Workspaces"), "{frame}");
        assert!(!frame.contains("ctrl+p"), "{frame}");
        assert!(!frame.contains("ctrl+h"), "{frame}");
    }

    #[test]
    fn executes_palette_view_action() {
        let mut app = test_app();

        app.handle_key(modified_key(KeyCode::Char('p'), KeyModifiers::CONTROL));
        for character in "ops / oplog".chars() {
            app.handle_key(key(KeyCode::Char(character)));
        }
        app.handle_key(key(KeyCode::Enter));

        let frame = render_app_text(&app);
        assert_eq!(app.ops.active(), OpsDestination::Oplog);
        assert!(frame.contains("View  [Grouped] Entries"), "{frame}");
        assert!(frame.contains("Oplog has no Agent scope"), "{frame}");
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

        driver.assert_visible(&app, action(ActionId::RefreshAgents).label);
        driver.assert_visible(&app, "Workspaces");
        driver.assert_visible(&app, action(ActionId::ShowHelp).label);
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

        assert_eq!(app.mode, InteractionMode::Help);
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
        assert_eq!(app.mode, InteractionMode::AgentFilter);
        assert_eq!(app.agents.query, "r");
        driver.key(&mut app, key(KeyCode::Esc));
        driver.key(
            &mut app,
            modified_key(KeyCode::Char('h'), KeyModifiers::CONTROL),
        );

        assert_eq!(app.mode, InteractionMode::Help);
        driver.assert_visible(&app, "Agents Overview");
    }

    #[test]
    fn legacy_build_key_does_not_change_help_context() {
        let mut app = test_app();
        let mut driver = TuiTestDriver::new(120, 48);

        driver.key(&mut app, key(KeyCode::Char('b')));
        assert_eq!(app.mode, InteractionMode::AgentFilter);
        assert_eq!(app.agents.query, "b");
        driver.key(&mut app, key(KeyCode::Esc));
        driver.key(
            &mut app,
            modified_key(KeyCode::Char('h'), KeyModifiers::CONTROL),
        );

        assert_eq!(app.mode, InteractionMode::Help);
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

        assert_eq!(app.mode, InteractionMode::Palette);
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
        assert_eq!(app.active_workspace, Workspace::Ops);
        assert_eq!(app.mode, InteractionMode::AgentFilter);
        assert_eq!(app.agents.query, "b");
    }

    #[test]
    fn bare_clean_key_starts_agent_find_in_ops_first_shell() {
        let mut app = test_app();

        app.handle_key(key(KeyCode::Char('c')));

        assert!(app.command_run.is_none());
        assert_eq!(app.active_workspace, Workspace::Ops);
        assert_eq!(app.mode, InteractionMode::AgentFilter);
        assert_eq!(app.agents.query, "c");
    }

    #[test]
    fn clean_palette_action_opens_a_review_form() {
        let mut app = test_app();
        app.active_workspace = Workspace::Dev;

        app.handle_key(modified_key(KeyCode::Char('p'), KeyModifiers::CONTROL));
        for character in "clean".chars() {
            app.handle_key(key(KeyCode::Char(character)));
        }
        app.handle_key(key(KeyCode::Enter));

        assert!(app.command_run.is_none());
        assert_eq!(app.mode, InteractionMode::CommandForm);
        assert!(matches!(
            app.command_form,
            Some(CommandReviewForm {
                kind: CommandKind::Clean,
                ..
            })
        ));
    }

    #[test]
    fn review_forms_start_on_run_and_hide_internal_build_steps() {
        let mut app = test_app();
        app.open_command_form(CommandKind::Build);

        let frame = render_app_text_at(&app, 120, 32);

        assert_eq!(app.command_form.as_ref().map(|form| form.selected), Some(0));
        assert!(frame.contains("[ Run ]"), "{frame}");
        assert!(!frame.contains("step check"), "{frame}");
        assert!(frame.contains("skip check"), "{frame}");
    }

    #[test]
    fn review_form_field_order_keeps_confirmation_options_first() {
        let build = CommandReviewForm::new(CommandKind::Build, CommandFormValues::default());
        assert_eq!(
            command_form_rows(&build)
                .iter()
                .map(|row| row.label)
                .collect::<Vec<_>>(),
            vec!["--yes", "skip check", "force build"]
        );

        let server = SessionReviewForm::Server {
            values: ServerFormValues::new("unused"),
            selected: 0,
            editing: None,
            restart: false,
            error: None,
        };
        assert_eq!(
            session_form_rows(&server)
                .iter()
                .map(|row| row.label)
                .take(2)
                .collect::<Vec<_>>(),
            vec!["clean", "--yes"]
        );
    }

    #[test]
    fn free_text_review_edits_are_transactional() {
        let mut app = test_app();
        app.open_command_form(CommandKind::Deploy);
        let form = app.command_form.as_mut().expect("deploy review form");
        form.selected = 4;
        form.values.rollback_version = "7".to_string();
        form.values.rollback_revision = "keep-until-apply".to_string();

        app.handle_command_form_key(key(KeyCode::Enter), None);
        app.handle_command_form_key(
            modified_key(KeyCode::Char('u'), KeyModifiers::CONTROL),
            None,
        );
        app.handle_command_form_key(key(KeyCode::Char('9')), None);
        app.handle_command_form_key(key(KeyCode::Esc), None);

        let form = app.command_form.as_ref().expect("deploy review form");
        assert_eq!(form.values.rollback_version, "7");
        assert_eq!(form.values.rollback_revision, "keep-until-apply");

        app.handle_command_form_key(key(KeyCode::Enter), None);
        app.handle_command_form_key(
            modified_key(KeyCode::Char('u'), KeyModifiers::CONTROL),
            None,
        );
        app.handle_command_form_key(key(KeyCode::Char('9')), None);
        app.handle_command_form_key(key(KeyCode::Enter), None);

        let form = app.command_form.as_ref().expect("deploy review form");
        assert_eq!(form.values.rollback_version, "9");
        assert!(form.values.rollback_revision.is_empty());
    }

    #[test]
    fn free_text_editor_moves_across_unicode_boundaries_and_scrolls_to_cursor() {
        let mut editor = TextEditState::new("abé界");
        editor.move_left();
        editor.backspace();
        editor.insert('🙂');

        assert_eq!(editor.draft, "ab🙂界");
        assert!(editor.draft.is_char_boundary(editor.cursor));
        let (visible, cursor_column) = editor.viewport(4);
        assert!(UnicodeWidthStr::width(visible.as_str()) <= 4);
        assert!(cursor_column < 4);
    }

    #[test]
    fn active_free_text_review_field_shows_cursor_and_input_background() {
        let mut app = test_app();
        app.open_command_form(CommandKind::Deploy);
        app.command_form
            .as_mut()
            .expect("deploy review form")
            .selected = 4;
        app.handle_command_form_key(key(KeyCode::Enter), None);

        let (_, cursor) = render_app_text_and_cursor(&app);
        let buffer = render_app_buffer_at(&app, 100, 24);
        assert_ne!((cursor.x, cursor.y), (0, 0));
        assert_eq!(buffer[(cursor.x, cursor.y)].bg, theme().input_background);
        let frame = render_buffer_text(&buffer);
        assert!(frame.contains("enter Apply"), "{frame}");
        assert!(frame.contains("esc Cancel edit"), "{frame}");
    }

    #[test]
    fn review_and_quit_dialogs_render_at_supported_extremes() {
        for (width, height) in [(120, 32), (64, 16), (40, 8), (1, 1)] {
            let mut command = test_app();
            command.open_command_form(CommandKind::Build);
            render_app_text_at(&command, width, height);

            let mut session = test_app();
            session.open_repl_form(false);
            render_app_text_at(&session, width, height);

            let mut quit = test_app();
            quit.command_run = Some(CommandRun::new(
                1,
                CommandKind::Build,
                vec!["build".to_string()],
                CommandOptions::default(),
                "sample-app:local".to_string(),
            ));
            quit.request_quit();
            render_app_text_at(&quit, width, height);
        }
    }

    #[test]
    fn rendered_layout_drives_exact_nested_terminal_size() {
        let mut app = test_app();
        app.active_workspace = Workspace::Dev;
        app.dev_primary = DevView::BuildDeploy;
        app.dev_focus = DevView::BuildDeploy;
        app.start_command(CommandKind::Build, None);

        render_app_text_at(&app, 120, 32);
        app.sync_nested_session_sizes();

        let area = snapshot_region(&app, RegionKind::DevViewBody(DevView::BuildDeploy));
        let (cols, rows) = pty_size_for_region(area);
        assert_eq!(
            app.command_run.as_ref().map(|run| run.terminal.size()),
            Some((rows, cols))
        );
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
        app.active_workspace = Workspace::Dev;

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
    fn bare_build_key_opens_agent_find_with_a_cursor() {
        let mut app = test_app();

        app.handle_key(key(KeyCode::Char('b')));
        let (_, cursor) = render_app_text_and_cursor(&app);

        assert_eq!(app.mode, InteractionMode::AgentFilter);
        assert_eq!(app.agents.query, "b");
        assert_ne!((cursor.x, cursor.y), (0, 0));
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
        app.active_workspace = Workspace::Dev;
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

        app.scroll_output_up_by(10);
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
    fn tui_control_page_up_enters_scrollback_and_escape_returns_to_tui() {
        let mut app = test_app();
        app.active_workspace = Workspace::Dev;
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
        app.mode.replace(InteractionMode::TerminalInput);

        app.handle_key(modified_key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        assert_eq!(app.mode, InteractionMode::Normal);
        app.handle_key(key(KeyCode::PageUp));
        assert_eq!(app.mode, InteractionMode::TerminalScrollback);
        assert!(app.command_run.as_ref().unwrap().terminal.scrollback() > 0);

        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.mode, InteractionMode::Normal);
        assert_eq!(app.command_run.as_ref().unwrap().terminal.scrollback(), 0);
    }

    #[test]
    fn nested_cli_summary_does_not_repeat_input_handoff_shortcuts() {
        let mut app = test_app();
        app.open_dev_workspace(DevView::Repl);
        app.start_repl_with_values(ReplFormValues::default(), None);

        let frame = render_app_text_at(&app, 140, 30);

        assert!(frame.contains("input CLI · session repl"), "{frame}");
        assert!(!frame.contains("input CLI · ctrl+x"), "{frame}");
        assert!(!frame.contains("action ctrl+x"), "{frame}");
        assert!(frame.contains("ctrl+x TUI control"), "{frame}");
    }

    #[test]
    fn command_stop_action_cancels_then_force_kills_running_command() {
        let mut app = test_app();

        app.start_command(CommandKind::Build, None);
        app.cancel_or_force_kill_command();
        app.command_run.as_mut().unwrap().stop_requested_at =
            Some(Instant::now() - FORCE_STOP_DELAY);
        assert_eq!(
            app.command_run.as_ref().map(|run| run.status),
            Some(CommandStatus::Cancelling)
        );

        app.cancel_or_force_kill_command();
        assert_eq!(
            app.command_run.as_ref().map(|run| run.status),
            Some(CommandStatus::Killed)
        );
        assert_eq!(app.mode, InteractionMode::Normal);
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
        assert_eq!(app.active_workspace, Workspace::Dev);
        assert_eq!(app.dev_focus, DevView::BuildDeploy);
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
    async fn terminal_and_control_events_preempt_buffered_nested_cli_output() {
        let (terminal_tx, mut terminal_rx) = mpsc::channel(4);
        let (control_tx, mut control_rx) = mpsc::channel(4);
        let (output_tx, mut output_rx) = mpsc::channel(4);
        output_tx
            .send(TuiEvent::NestedCli {
                target: NestedCliTarget::Server,
                event: NestedCliEvent::Output(b"bulk server output".to_vec()),
            })
            .await
            .unwrap();
        control_tx.send(TuiEvent::SpinnerTick(7)).await.unwrap();
        terminal_tx.send(Event::Resize(101, 37)).await.unwrap();

        assert!(matches!(
            next_tui_event(&mut terminal_rx, &mut control_rx, &mut output_rx).await,
            Some(TuiEvent::Terminal(Event::Resize(101, 37)))
        ));
        assert!(matches!(
            next_tui_event(&mut terminal_rx, &mut control_rx, &mut output_rx).await,
            Some(TuiEvent::SpinnerTick(7))
        ));
        assert!(matches!(
            next_tui_event(&mut terminal_rx, &mut control_rx, &mut output_rx).await,
            Some(TuiEvent::NestedCli {
                target: NestedCliTarget::Server,
                event: NestedCliEvent::Output(_),
            })
        ));
    }

    #[test]
    fn tight_agent_tables_reserve_more_width_for_agent_id() {
        let state = PaneTableState::new(&AGENT_COLUMNS, 0);
        let rows: &[&[&str]] = &[&[
            "agent-with-a-long-structured-identifier",
            "Running",
            "LongAgentType",
            "long-component-name",
            "123",
            "42",
            "2026-10-07T12:00:00Z",
        ]];

        let resolved = responsive_table_columns(&AGENT_COLUMNS, rows, &state, 48);

        assert_eq!(resolved[0].width, 18);
        assert!(resolved[0].width > resolved[1].width);
        assert!(resolved[0].width > resolved[2].width);
        assert!(resolved[0].width > resolved[3].width);
    }

    #[test]
    fn dev_server_is_a_normal_view_not_a_global_drawer() {
        let mut app = test_app();

        app.open_dev_workspace(DevView::Server);
        let frame = render_app_text(&app);

        assert_eq!(app.active_workspace, Workspace::Dev);
        assert!(frame.contains("( Ops )"), "{frame}");
        assert!(frame.contains("Server"), "{frame}");
        assert_eq!(
            frame
                .matches("○ stopped · input TUI · session server")
                .count(),
            1,
            "{frame}"
        );
    }

    #[test]
    fn server_form_clean_is_applied_to_the_reviewed_command() {
        let app = test_app();
        let mut values = ServerFormValues::new(&app.selected_target.context.config_dir);
        values.clean = true;
        let args = server_args_from_values(&values);

        assert_eq!(args, vec!["server", "run", "--clean"]);
    }

    #[test]
    fn server_ports_file_is_optional_and_only_emitted_when_configured() {
        let app = test_app();
        let mut values = ServerFormValues::new(&app.selected_target.context.config_dir);
        assert_eq!(server_args_from_values(&values), vec!["server", "run"]);

        values.ports_file = " ./ports.json ".to_string();
        assert_eq!(
            server_args_from_values(&values),
            vec!["server", "run", "--ports-file", "./ports.json"]
        );
    }

    #[test]
    fn server_launch_reveals_its_view_but_stays_tui_owned() {
        let mut app = test_app();
        app.start_server_with_values(ServerFormValues::new("unused"), None);

        assert_eq!(app.active_workspace, Workspace::Dev);
        assert_eq!(app.dev_focus, DevView::Server);
        assert!(app.dev_view_visible(DevView::Server));
        assert_eq!(app.mode, InteractionMode::Normal);
        assert_eq!(app.server.run.status, ServerStatus::Starting);

        app.server.run.mark_spawned();
        assert_eq!(app.server.run.status, ServerStatus::Running);
        let frame = render_app_text_at(&app, 120, 30);
        assert!(frame.contains("Server ● running"), "{frame}");
        assert!(frame.contains("enter CLI input"), "{frame}");
        assert!(frame.contains("TUI · enter → CLI"), "{frame}");
    }

    #[test]
    fn reviewed_dev_commands_build_exact_nested_cli_arguments() {
        let app = test_app();
        let build = CommandFormValues {
            skip_check: true,
            force_build: true,
            yes: true,
            ..CommandFormValues::default()
        };
        assert_eq!(
            app.command_args(CommandKind::Build, &build),
            vec!["build", "--skip-check", "--force-build", "--yes",]
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

        app.selected_target.context.environment = "other".to_string();
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
        let mut server_values =
            ServerFormValues::new(&server_app.selected_target.context.config_dir);
        server_values.data_dir = "./data".to_string();
        server_values.clean = true;
        server_values.yes = true;
        server_app.session_form = Some(SessionReviewForm::Server {
            values: server_values,
            selected: 10,
            editing: None,
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
            editing: None,
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
        app.open_server_form(false, false);
        assert_eq!(app.server.run.status, ServerStatus::Stopped);
        assert_eq!(app.mode, InteractionMode::SessionForm);
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
        assert_eq!(app.active_workspace, Workspace::Ops);
        assert_eq!(app.server.run.status, ServerStatus::Stopped);

        app.server.run.status = ServerStatus::Running;
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.server.run.status, ServerStatus::Running);
    }

    #[test]
    fn server_start_action_opens_review_without_rearranging_views() {
        let mut app = test_app();
        let workspace = app.active_workspace;
        let focus = app.dev_focus;
        let visible = app.layout.dev_visible;

        app.execute_action(ActionId::StartServer, None);

        assert_eq!(app.active_workspace, workspace);
        assert_eq!(app.dev_focus, focus);
        assert_eq!(app.layout.dev_visible, visible);
        assert_eq!(app.server.run.status, ServerStatus::Stopped);
        assert_eq!(app.mode, InteractionMode::SessionForm);
    }

    #[test]
    fn lifecycle_palette_exposes_only_state_valid_verbs() {
        let mut app = test_app();
        app.active_workspace = Workspace::Dev;
        let ids = |app: &TuiApp| {
            palette_actions(PaletteScope::All, app)
                .into_iter()
                .map(|action| action.id)
                .collect::<HashSet<_>>()
        };

        let stopped = ids(&app);
        assert!(stopped.contains(&ActionId::StartServer));
        assert!(stopped.contains(&ActionId::StartRepl));
        assert!(!stopped.contains(&ActionId::StopServer));
        assert!(!stopped.contains(&ActionId::RestartServer));

        app.server.run.status = ServerStatus::Running;
        app.repl.run.status = ReplStatus::Running;
        let running = ids(&app);
        assert!(running.contains(&ActionId::StopServer));
        assert!(running.contains(&ActionId::RestartServer));
        assert!(running.contains(&ActionId::StopRepl));
        assert!(running.contains(&ActionId::RestartRepl));
        assert!(!running.contains(&ActionId::StartServer));
        assert!(!running.contains(&ActionId::StartRepl));

        app.server.run.status = ServerStatus::Stopping;
        app.server.run.stop_requested_at = Some(Instant::now());
        app.repl.run.status = ReplStatus::Stopping;
        app.repl.run.stop_requested_at = Some(Instant::now() - FORCE_STOP_DELAY);
        let stopping = ids(&app);
        assert!(stopping.contains(&ActionId::ForceStopServer));
        assert!(stopping.contains(&ActionId::ForceStopRepl));
        assert_eq!(
            app.action_availability(action(ActionId::ForceStopServer)),
            ActionAvailability::Unavailable("available five seconds after stop is requested")
        );
        assert_eq!(
            app.action_availability(action(ActionId::ForceStopRepl)),
            ActionAvailability::Available
        );
    }

    #[test]
    fn dev_execution_reveals_and_focuses_its_owning_view() {
        let mut command = test_app();
        command.dev_primary = DevView::Repl;
        command.dev_focus = DevView::Repl;
        command.layout.dev_visible = [false, false, true];
        let command_primary = command.dev_primary;
        command.start_command(CommandKind::Build, None);
        assert_eq!(command.active_workspace, Workspace::Dev);
        assert_eq!(command.dev_primary, command_primary);
        assert_eq!(command.dev_focus, DevView::BuildDeploy);
        assert!(command.dev_view_visible(DevView::BuildDeploy));
        assert_eq!(command.mode, InteractionMode::TerminalInput);

        let mut server = test_app();
        server.dev_primary = DevView::BuildDeploy;
        server.dev_focus = DevView::BuildDeploy;
        server.layout.dev_visible = [true, false, false];
        server.start_server_with_values(ServerFormValues::new("unused"), None);
        assert_eq!(server.active_workspace, Workspace::Dev);
        assert_eq!(server.dev_primary, DevView::BuildDeploy);
        assert_eq!(server.dev_focus, DevView::Server);
        assert!(server.dev_view_visible(DevView::Server));
        assert_eq!(server.mode, InteractionMode::Normal);

        let mut repl = test_app();
        repl.dev_primary = DevView::BuildDeploy;
        repl.dev_focus = DevView::BuildDeploy;
        repl.layout.dev_visible = [true, false, false];
        repl.start_repl_with_values(ReplFormValues::default(), None);
        assert_eq!(repl.active_workspace, Workspace::Dev);
        assert_eq!(repl.dev_primary, DevView::BuildDeploy);
        assert_eq!(repl.dev_focus, DevView::Repl);
        assert!(repl.dev_view_visible(DevView::Repl));
        assert_eq!(repl.mode, InteractionMode::TerminalInput);
    }

    #[test]
    fn visible_nested_cli_launches_take_input_without_promoting() {
        let mut command = test_app();
        command.active_workspace = Workspace::Dev;
        command.dev_primary = DevView::BuildDeploy;
        command.dev_focus = DevView::BuildDeploy;
        command.layout.dev_visible = [true, false, false];
        command.start_command(CommandKind::Build, None);
        assert_eq!(command.mode, InteractionMode::TerminalInput);

        let mut repl = test_app();
        repl.open_dev_workspace(DevView::Repl);
        let primary = repl.dev_primary;
        let visible = repl.layout.dev_visible;
        repl.start_repl_with_values(ReplFormValues::default(), None);
        assert_eq!(repl.dev_primary, primary);
        assert_eq!(repl.layout.dev_visible, visible);
        assert_eq!(repl.mode, InteractionMode::TerminalInput);
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
        let server = snapshot_region(&app, RegionKind::DevViewBody(DevView::Server));

        app.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::ScrollUp,
                column: server.x.saturating_add(1),
                row: server.y.saturating_add(2),
                modifiers: KeyModifiers::empty(),
            },
            None,
        );

        assert_eq!(app.server.run.terminal.scrollback(), 3);
        assert_eq!(app.mode, InteractionMode::TerminalScrollback);
    }

    #[test]
    fn production_layout_omits_legacy_dev_panel_regions() {
        let app = test_app();
        render_app_text_at(&app, 120, 32);
        assert!(
            app.geometry
                .region(RegionKind::DevViewBody(DevView::BuildDeploy))
                .is_none()
        );
    }

    #[test]
    fn production_layout_exposes_both_workspace_selectors() {
        let app = test_app();
        render_app_text_at(&app, 120, 32);
        let snapshot = app.geometry.snapshot().unwrap();
        assert!(
            snapshot
                .region(RegionKind::Workspace(Workspace::Ops))
                .is_some()
        );
        assert!(
            snapshot
                .region(RegionKind::Workspace(Workspace::Dev))
                .is_some()
        );
    }

    #[test]
    fn production_layout_does_not_expose_dev_panel_hit_targets() {
        let app = test_app();
        render_app_text_at(&app, 120, 32);
        assert!(
            app.geometry
                .region(RegionKind::DevViewBody(DevView::BuildDeploy))
                .is_none()
        );
    }

    #[test]
    fn mouse_click_agent_row_selects_agent() {
        let mut app = test_app();
        app.active_workspace = Workspace::Ops;
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
        app.active_workspace = Workspace::Ops;
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
        app.active_workspace = Workspace::Ops;
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
        confirm_app
            .mode
            .replace(InteractionMode::ContextSwitchConfirm);
        confirm_app.command_run = Some(CommandRun::new(
            1,
            CommandKind::Build,
            vec!["build".to_string()],
            CommandOptions::default(),
            "sample-app:local".to_string(),
        ));
        confirm_app.context_switcher.pending_action =
            Some(PendingContextSwitchAction::Switch(ContextSwitchRequest {
                global_flags: GolemCliGlobalFlags::default(),
                environment_reference: None,
                server_identity: ServerIdentity::Named("other".to_string()),
                dev_eligible: false,
            }));
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
        cancel_app
            .mode
            .replace(InteractionMode::ContextSwitchConfirm);
        cancel_app.context_switcher.pending_action =
            Some(PendingContextSwitchAction::Switch(ContextSwitchRequest {
                global_flags: GolemCliGlobalFlags::default(),
                environment_reference: None,
                server_identity: ServerIdentity::Named("other".to_string()),
                dev_eligible: false,
            }));
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

        assert_eq!(cancel_app.mode, InteractionMode::ContextPicker);
        assert!(cancel_app.context_switcher.pending_action.is_none());
    }

    #[test]
    fn production_layout_omits_dev_resize_split() {
        let app = test_app();
        render_app_text_at(&app, 120, 32);
        assert!(app.geometry.region(RegionKind::DevPrimarySplit).is_none());
    }

    #[test]
    fn production_layout_exposes_ops_details_resize_split() {
        let mut app = test_app();
        app.agents.detail_visible = true;
        render_app_text_at(&app, 120, 32);
        assert!(app.geometry.region(RegionKind::OpsDetailsSplit).is_some());
    }

    #[test]
    fn deployment_history_navigation_and_responsive_rendering_are_stable() {
        let mut app = test_app();
        app.active_workspace = Workspace::Ops;
        app.ops.select(OpsDestination::Deployments);
        app.deployments.history = (1..=15)
            .rev()
            .map(|revision| crate::tui::ops::deployments::DeploymentRecord {
                revision,
                version: format!("version-{revision}"),
                hash: format!("hash-{revision}"),
            })
            .collect();

        for (width, height) in [(120, 32), (80, 12), (42, 24), (1, 1)] {
            let frame = render_app_text_at(&app, width, height);
            if width > 40 && height > 4 {
                assert!(frame.contains("Current & recent"), "{frame}");
            }
        }

        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.deployments.selected, 1);
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.ops.current_route(), &OpsRoute::DeploymentRevision(14));
        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.ops.current_route(), &OpsRoute::Root);

        app.handle_key(modified_key(KeyCode::Char('a'), KeyModifiers::CONTROL));
        assert_eq!(app.ops.current_route(), &OpsRoute::DeploymentHistory);
        let frame = render_app_text_at(&app, 100, 24);
        assert!(frame.contains("All History"), "{frame}");
    }

    #[test]
    fn bare_repl_key_starts_agent_find_in_ops_first_shell() {
        let mut app = test_app();

        app.handle_key(key(KeyCode::Char('r')));

        assert_eq!(app.active_workspace, Workspace::Ops);
        assert_eq!(app.dev_focus, DevView::Repl);
        assert_eq!(app.mode, InteractionMode::AgentFilter);
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
        app.handle_event(
            TuiEvent::NestedCli {
                target: NestedCliTarget::Repl,
                event: NestedCliEvent::Output(b"hello\x1b[2DXY".to_vec()),
            },
            &tx,
        );

        let frame = render_app_text(&app);
        assert!(!frame.contains("helXY"), "{frame}");
    }

    #[test]
    fn inactive_repl_keeps_the_cursor_on_agent_find() {
        let mut app = test_app();
        let (tx, _rx) = test_event_channel();

        app.handle_key(key(KeyCode::Char('r')));
        app.handle_event(
            TuiEvent::NestedCli {
                target: NestedCliTarget::Repl,
                event: NestedCliEvent::Output(b"abc".to_vec()),
            },
            &tx,
        );
        let (_, cursor) = render_app_text_and_cursor(&app);

        assert_eq!(app.mode, InteractionMode::AgentFilter);
        assert_eq!(app.agents.query, "r");
        assert_ne!((cursor.x, cursor.y), (0, 0));
    }

    #[test]
    fn repl_leader_actions_are_not_exposed() {
        let mut app = test_app();

        app.handle_key(modified_key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        assert_eq!(app.mode, InteractionMode::Leader);

        app.handle_key(key(KeyCode::Char('q')));
        assert_eq!(app.mode, InteractionMode::Normal);
        assert_eq!(app.repl.run.status, ReplStatus::Stopped);

        app.handle_key(modified_key(KeyCode::Char('x'), KeyModifiers::CONTROL));
        app.handle_key(key(KeyCode::Char('k')));
        assert_eq!(app.mode, InteractionMode::Normal);
        assert_eq!(app.repl.run.status, ReplStatus::Stopped);
    }

    #[test]
    fn native_ssh_input_owns_typing_and_renders_its_cursor() {
        let mut app = test_app();
        let agent = AgentIdentity {
            component: "component".to_string(),
            agent_id: "Agent(\"a-long-identifier\")".to_string(),
        };
        app.ops.open_ssh(agent.clone());
        app.ssh.target = Some(agent);
        app.ssh.status = SshStatus::Ready;
        app.mode.replace(InteractionMode::SshInput);

        for character in "echo hello".chars() {
            app.handle_key(key(KeyCode::Char(character)));
        }
        let (frame, cursor) = render_app_text_and_cursor(&app);

        assert_eq!(app.ssh.input, "echo hello");
        assert!(frame.contains("Agents / SSH"), "{frame}");
        assert!(frame.contains("echo hello"), "{frame}");
        assert_ne!((cursor.x, cursor.y), (0, 0));

        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.mode, InteractionMode::Normal);
        assert_eq!(app.ssh.input, "echo hello");
    }

    #[test]
    async fn native_ssh_ctrl_c_cancels_the_running_command() {
        let mut app = test_app();
        let (cancel, cancelled) = oneshot::channel();
        app.ssh.status = SshStatus::Running;
        app.ssh.cancel = Some(cancel);
        app.mode.replace(InteractionMode::SshInput);

        app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));

        assert!(cancelled.await.is_ok());
        assert!(app.ssh.cancel.is_none());
        assert_eq!(app.mode, InteractionMode::SshInput);
    }

    #[test]
    fn returning_to_the_same_agent_ssh_preserves_the_session_transcript() {
        let mut app = test_app();
        let agent = AgentIdentity {
            component: "component".to_string(),
            agent_id: "Agent(\"retained\")".to_string(),
        };
        app.ops.open_agent(agent.clone());
        app.ssh.target = Some(agent.clone());
        app.ssh.status = SshStatus::Ready;
        app.ssh
            .push(SshLineKind::Stdout, "retained output".to_string());

        app.open_agent_ssh(None);

        assert_eq!(app.ops.current_route(), &OpsRoute::Ssh(agent));
        assert_eq!(app.ssh.status, SshStatus::Ready);
        assert_eq!(app.ssh.lines[0].text, "retained output");
        assert_eq!(app.mode, InteractionMode::SshInput);
    }

    fn test_app() -> TuiApp {
        let mut ops = OpsApp::default();
        ops.select(OpsDestination::Agents);
        TuiApp {
            should_quit: false,
            active_workspace: Workspace::Ops,
            ops,
            dev_primary: DevView::Repl,
            dev_focus: DevView::Repl,
            mode: ModeStack::default(),
            leader: LeaderState::default(),
            quit_choice: QuitChoice::default(),
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
            overview: OverviewState::default(),
            deployments: DeploymentsState::default(),
            metrics: MetricsState::default(),
            metrics_range_form: None,
            activity: ActivityState::default(),
            resources: ResourcesState::default(),
            stream: StreamState::default(),
            ssh: SshState::default(),
            next_command_id: 1,
            selected_target: SelectedTarget {
                context: TuiContextInfo {
                    application: "sample-app".to_string(),
                    environment: "local".to_string(),
                    server: "local".to_string(),
                    config_dir: "/tmp/golem-config".to_string(),
                    uses_local_server: true,
                    dev_eligible: true,
                },
                cli_args: Vec::new(),
                environment_reference: None,
                server_identity: ServerIdentity::Local,
            },
            context_switcher: ContextSwitcherState::new(vec![TuiContextTarget {
                key: "manifest:local".to_string(),
                server_identity: ServerIdentity::Local,
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
            context_executor: None,
            auth_prompt: None,
            layout: TuiLayoutState::default(),
            scrollbar_dragging: None,
            geometry: UiGeometry::default(),
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
            server_identity: ServerIdentity::Local,
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
            server_identity: ServerIdentity::Named("cloud".to_string()),
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
            server_identity: ServerIdentity::Named(key.to_string()),
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
        let (tx, rx) = mpsc::channel(16);
        (TuiEventSender::single(tx), rx)
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

    fn sample_ops_environment_snapshot(deployment_revision: &str) -> OpsEnvironmentSnapshot {
        OpsEnvironmentSnapshot {
            deployment: Some(OpsDeploymentSnapshot {
                deployment_revision: deployment_revision.to_string(),
                version: format!("demo-{deployment_revision}"),
                hash: format!("hash-{deployment_revision}"),
            }),
        }
    }

    fn sample_overview_capability(now: i64) -> LocalObservabilityCapability {
        LocalObservabilityCapability {
            status: LocalObservabilityStatus::Ready,
            message: None,
            exporter_name: "golem-agent-otel".to_string(),
            exporter_version: "1.5.7".to_string(),
            sample_interval_seconds: 5,
            retention_days: 7,
            retention_bytes: 512 * 1024 * 1024,
            discarded_points: 0,
            last_service_sample_unix_nanos: Some(now.saturating_sub(2_000_000_000)),
            last_agent_ingest_unix_nanos: Some(now.saturating_sub(4_000_000_000)),
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

    fn sample_activity_entries(count: usize) -> Vec<activity::ActivityEntry> {
        (0..count)
            .map(|offset| {
                let index = offset as u64 + 1;
                activity::ActivityEntry {
                    component: "cart".to_string(),
                    agent_id: format!("CartAgent(\"cart-{offset:02}\")"),
                    presentation: crate::model::agent::oplog::OplogEntryPresentation {
                        index,
                        timestamp: golem_common::model::Timestamp::from(index),
                        kind: "ImportedFunctionInvoked".to_string(),
                        group: Some(OplogEntryGroupKey::OplogIndex(index)),
                        detail: serde_json::json!({
                            "index": index,
                            "arguments": (0..40).collect::<Vec<_>>(),
                            "result": { "ok": true }
                        }),
                        attribution: None,
                        entry: None,
                        search_text: std::sync::OnceLock::new(),
                    },
                }
            })
            .collect()
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
        app.geometry
            .region(kind)
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

    fn sample_metrics_dashboard(panel_count: usize) -> LocalMetricDashboardResponse {
        LocalMetricDashboardResponse {
            source: LocalMetricSource::Agents,
            range: LocalMetricTimeRange {
                from_unix_nanos: 10,
                to_unix_nanos: 20,
            },
            generated_at_unix_nanos: 20,
            freshest_sample_unix_nanos: Some(20),
            stats: vec![LocalMetricDashboardStat {
                id: "invocations".to_string(),
                label: "Invocations started".to_string(),
                value: Some(42.0),
                unit: "1".to_string(),
                approximate: false,
            }],
            panels: (0..panel_count)
                .map(|index| LocalMetricDashboardPanel {
                    id: format!("panel-{index}"),
                    title: format!("Panel {index}"),
                    kind: LocalMetricDashboardPanelKind::TimeSeries,
                    series: vec![LocalMetricDashboardSeries {
                        key: format!("series-{index}"),
                        label: format!("Series {index}"),
                        unit: "invocations/s".to_string(),
                        samples: vec![
                            LocalMetricSample {
                                timestamp_unix_nanos: 10,
                                value: index as f64,
                                statistic: "rate".to_string(),
                                source_oplog_index: Some(index as u64),
                            },
                            LocalMetricSample {
                                timestamp_unix_nanos: 20,
                                value: index.saturating_add(1) as f64,
                                statistic: "rate".to_string(),
                                source_oplog_index: Some(index.saturating_add(1) as u64),
                            },
                        ],
                        latest_value: Some(index.saturating_add(1) as f64),
                        approximate: false,
                    }],
                    empty_reason: None,
                    observed_from_unix_nanos: Some(10),
                    observed_to_unix_nanos: Some(20),
                    approximate: false,
                })
                .collect(),
        }
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
