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
    if !matches!(case_id, "ops-agents" | "ops-overview") {
        anyhow::bail!("unknown TUI preview case: {case_id}");
    }
    render_fixture_buffer(case_id, width, height, false)
}

#[cfg(test)]
pub(in crate::tui) fn render_production_preview_buffer(
    width: u16,
    height: u16,
    styled: bool,
) -> anyhow::Result<ratatui::buffer::Buffer> {
    render_fixture_buffer("ops-agents", width, height, styled)
}

fn render_fixture_buffer(
    case_id: &str,
    width: u16,
    height: u16,
    explicitly_styled: bool,
) -> anyhow::Result<ratatui::buffer::Buffer> {
    let app = preview_app(case_id);
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

fn preview_app(case_id: &str) -> TuiApp {
    let mut ops = OpsApp::default();
    ops.select(if case_id == "ops-overview" {
        OpsDestination::Overview
    } else {
        OpsDestination::Agents
    });
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
        overview: OverviewState::default(),
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
    if case_id == "ops-overview" {
        let now = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .min(i64::MAX as u128) as i64;
        let capability = crate::model::local_observability::LocalObservabilityCapability {
            status: crate::model::local_observability::LocalObservabilityStatus::Ready,
            message: None,
            exporter_name: "golem-agent-otel".into(),
            exporter_version: "1.6.0".into(),
            sample_interval_seconds: 5,
            retention_days: 7,
            retention_bytes: 512 * 1024 * 1024,
            discarded_points: 0,
            last_service_sample_unix_nanos: Some(now.saturating_sub(2_000_000_000)),
            last_agent_ingest_unix_nanos: Some(now.saturating_sub(4_000_000_000)),
        };
        app.ops_environment.snapshot = Some(OpsEnvironmentSnapshot {
            application_name: "preview-app".into(),
            environment_name: "local".into(),
            environment_revision: "12".into(),
            diff_model_version: 2,
            compatibility_check: true,
            tool_compatibility_mode: "Strict".into(),
            version_check: true,
            security_overrides: false,
            deployment: Some(OpsDeploymentSnapshot {
                deployment_revision: "12".into(),
                version: "demo-12".into(),
                hash: "d933d8d24db984b94aa1".into(),
                composition: Some(OpsDeploymentComposition {
                    components: 4,
                    agent_types: 12,
                    http_apis: 2,
                    mcp: 1,
                    tools: 6,
                    middleware: 3,
                    bindings: 5,
                }),
            }),
        });
        app.overview.observability.value = Some(OverviewObservability {
            capability: Some(capability.clone()),
        });
        app.overview.agent_signals.value = Some(OverviewAgentSignals {
            capability: Some(capability),
            dashboard: Some(
                crate::model::local_observability::LocalMetricDashboardResponse {
                    source: crate::model::local_observability::LocalMetricSource::Agents,
                    range: crate::model::local_observability::LocalMetricTimeRange {
                        from_unix_nanos: now.saturating_sub(15 * 60 * 1_000_000_000),
                        to_unix_nanos: now,
                    },
                    generated_at_unix_nanos: now,
                    freshest_sample_unix_nanos: Some(now.saturating_sub(4_000_000_000)),
                    stats: [
                        ("invocations", "Invocations started", 1842.0, "1"),
                        ("errors", "Error events", 7.0, "1"),
                        ("duration-p95", "Completed p95", 18_400_000.0, "ns"),
                        ("observed-agents", "Observed Agents", 326.0, "agents"),
                    ]
                    .into_iter()
                    .map(|(id, label, value, unit)| {
                        crate::model::local_observability::LocalMetricDashboardStat {
                            id: id.into(),
                            label: label.into(),
                            value: Some(value),
                            unit: unit.into(),
                            approximate: id == "duration-p95",
                        }
                    })
                    .collect(),
                    panels: Vec::new(),
                },
            ),
        });
        app.stream.status = StreamStatus::Connected;
        app.stream.target = Some(AgentIdentity {
            component: "checkout".into(),
            agent_id: "checkout(cart-17)".into(),
        });
        app.stream.unread = 3;
    }
    app
}
