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

use super::*;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

pub(in crate::tui) fn render_preview_buffer(
    case_id: &str,
    width: u16,
    height: u16,
) -> anyhow::Result<ratatui::buffer::Buffer> {
    if case_id != "ops-agents" {
        anyhow::bail!("unknown TUI preview case: {case_id}");
    }
    render_fixture_buffer(width, height, false)
}

#[cfg(test)]
pub(in crate::tui) fn render_production_preview_buffer(
    width: u16,
    height: u16,
    styled: bool,
) -> anyhow::Result<ratatui::buffer::Buffer> {
    render_fixture_buffer(width, height, styled)
}

fn render_fixture_buffer(
    width: u16,
    height: u16,
    explicitly_styled: bool,
) -> anyhow::Result<ratatui::buffer::Buffer> {
    let app = preview_app();
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend)?;
    terminal.draw(|frame| {
        if explicitly_styled {
            render_styled(frame, &app, &TuiVisualStyle::production());
        } else {
            render(frame, &app);
        }
    })?;
    Ok(terminal.backend().buffer().clone())
}

fn preview_app() -> TuiApp {
    let mut ops = OpsApp::default();
    ops.select(OpsDestination::Agents);
    let mut app = TuiApp {
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
        deployments: DeploymentsState::default(),
        metrics: MetricsState::default(),
        activity: ActivityState::default(),
        resources: ResourcesState::default(),
        stream: StreamState::default(),
        next_command_id: 1,
        selected_target: SelectedTarget {
            context: TuiContextInfo {
                application: "preview-app".into(),
                environment: "local".into(),
                server: "local".into(),
                config_dir: "/preview/config".into(),
                uses_local_server: true,
                dev_eligible: true,
            },
            cli_args: Vec::new(),
            environment_reference: None,
            server_identity: ServerIdentity::Local,
        },
        context_switcher: ContextSwitcherState::new(vec![TuiContextTarget {
            key: "manifest:local".into(),
            server_identity: ServerIdentity::Local,
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
        context_executor: None,
        auth_prompt: None,
        layout: TuiLayoutState::default(),
        scrollbar_dragging: None,
        geometry: UiGeometry::default(),
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
