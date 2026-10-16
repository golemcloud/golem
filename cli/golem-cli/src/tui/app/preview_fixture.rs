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
use crate::model::agent::oplog::{OplogEntryGroupKey, OplogEntryPresentation};
use crate::model::local_observability::{
    LocalMetricDashboardPanel, LocalMetricDashboardPanelKind, LocalMetricDashboardResponse,
    LocalMetricDashboardSeries, LocalMetricDashboardStat, LocalMetricFamily, LocalMetricKind,
    LocalMetricSample, LocalMetricSamplesResponse, LocalMetricSeries, LocalMetricSource,
    LocalMetricTemporality, LocalMetricTimeRange,
};
use crate::tui::ops::activity::{ActivityAgentProgress, ActivityEntry};
use crate::tui::ops::deployments::DeploymentRecord;
use crate::tui::ops::resources::{ResourceField, ResourceFocus, ResourceRow};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

pub(in crate::tui) fn render_preview_buffer(
    case_id: &str,
    width: u16,
    height: u16,
) -> anyhow::Result<ratatui::buffer::Buffer> {
    if !matches!(
        case_id,
        "ops-overview"
            | "ops-agents"
            | "ops-agent-detail"
            | "ops-ssh"
            | "ops-oplog"
            | "ops-oplog-entries"
            | "ops-stream"
            | "ops-metrics-dashboard"
            | "ops-metrics-explore"
            | "ops-deployments"
            | "ops-configuration"
    ) {
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
    ops.select(match case_id {
        "ops-overview" => OpsDestination::Overview,
        "ops-agents" | "ops-agent-detail" | "ops-ssh" => OpsDestination::Agents,
        "ops-oplog" | "ops-oplog-entries" => OpsDestination::Oplog,
        "ops-stream" => OpsDestination::Stream,
        "ops-metrics-dashboard" | "ops-metrics-explore" => OpsDestination::Metrics,
        "ops-deployments" => OpsDestination::Deployments,
        "ops-configuration" => OpsDestination::Configuration,
        _ => unreachable!("validated preview case"),
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
        metrics_range_form: None,
        activity: ActivityState::default(),
        resources: ResourcesState::default(),
        stream: StreamState::default(),
        ssh: SshState::default(),
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
            raw: serde_json::json!({
                "agentId": "checkout(cart-17)",
                "agentType": "CheckoutAgent",
                "componentRevision": 7,
                "createdAt": "2026-09-23T12:00:00Z",
                "pendingInvocations": 2,
                "status": "running"
            }),
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
            raw: serde_json::json!({
                "agentId": "orders(order-04)",
                "agentType": "OrderAgent",
                "componentRevision": 4,
                "createdAt": "2026-09-23T11:30:00Z",
                "pendingInvocations": 0,
                "status": "idle"
            }),
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
            exporter_version: "1.5.7".into(),
            sample_interval_seconds: 5,
            retention_days: 7,
            retention_bytes: 512 * 1024 * 1024,
            discarded_points: 0,
            last_service_sample_unix_nanos: Some(now.saturating_sub(2_000_000_000)),
            last_agent_ingest_unix_nanos: Some(now.saturating_sub(4_000_000_000)),
        };
        app.ops_environment.snapshot = Some(preview_environment_snapshot());
        app.overview.observability.value = Some(OverviewObservability {
            capability: Some(capability),
        });
        app.stream.status = StreamStatus::Connected;
        app.stream.target = Some(AgentIdentity {
            component: "checkout".into(),
            agent_id: "checkout(cart-17)".into(),
        });
        app.stream.unread = 3;
    }
    match case_id {
        "ops-agent-detail" => populate_agent_detail(&mut app),
        "ops-ssh" => populate_ssh(&mut app),
        "ops-oplog" => {
            populate_oplog(&mut app);
            app.activity.selected = 3;
        }
        "ops-oplog-entries" => {
            populate_oplog(&mut app);
            app.activity.representation = ActivityRepresentation::Entries;
            app.activity.selected = 1;
        }
        "ops-stream" => populate_stream(&mut app),
        "ops-metrics-dashboard" => populate_metrics_dashboard(&mut app),
        "ops-metrics-explore" => populate_metrics_explore(&mut app),
        "ops-deployments" => populate_deployments(&mut app),
        "ops-configuration" => populate_configuration(&mut app),
        _ => {}
    }
    app
}

fn populate_ssh(app: &mut TuiApp) {
    let target = app.agents.agents[0].identity();
    app.ops.open_agent(target.clone());
    app.ops.open_ssh(target.clone());
    app.ssh.target = Some(target);
    app.ssh.status = SshStatus::Ready;
    app.ssh.push(
        SshLineKind::Status,
        "Connected with the Agent's bound `bash` tool".into(),
    );
    app.ssh.push(
        SshLineKind::Command,
        "pwd && find . -maxdepth 2 -type f".into(),
    );
    app.ssh.push(SshLineKind::Stdout, "/workspace".into());
    app.ssh
        .push(SshLineKind::Stdout, "./config/app.toml".into());
    app.ssh.push(SshLineKind::Stdout, "./data/orders.db".into());
    app.ssh.push(SshLineKind::Status, "exit 0 · 34ms".into());
    app.ssh.input = "tail -n 20 logs/agent.log".into();
    app.ssh.cursor = app.ssh.input.len();
    app.mode.replace(InteractionMode::SshInput);
}

fn preview_environment_snapshot() -> OpsEnvironmentSnapshot {
    OpsEnvironmentSnapshot {
        deployment: Some(OpsDeploymentSnapshot {
            deployment_revision: "12".into(),
            version: "demo-12".into(),
            hash: "d933d8d24db984b94aa1".into(),
        }),
    }
}

fn populate_agent_detail(app: &mut TuiApp) {
    app.agents.agent_types.insert(
        ("checkout".into(), "CheckoutAgent".into()),
        serde_json::json!({
            "agentType": "CheckoutAgent",
            "constructor": {
                "parameters": [
                    {"name": "cartId", "type": "string"},
                    {"name": "currency", "type": "string"}
                ]
            },
            "methods": ["addItem", "checkout", "abandon"]
        }),
    );
    app.ops.open_agent(app.agents.agents[0].identity());
}

fn populate_oplog(app: &mut TuiApp) {
    let checkout = AgentIdentity {
        component: "checkout".into(),
        agent_id: "checkout(cart-17)".into(),
    };
    let orders = AgentIdentity {
        component: "orders".into(),
        agent_id: "orders(order-04)".into(),
    };
    app.ops.open_oplog(OplogRoute {
        agents: vec![checkout.clone(), orders.clone()],
        focus: None,
        from_unix_nanos: None,
        to_unix_nanos: None,
    });
    app.activity.agents = vec![
        ActivityAgent {
            component: checkout.component.clone(),
            agent_id: checkout.agent_id.clone(),
        },
        ActivityAgent {
            component: orders.component.clone(),
            agent_id: orders.agent_id.clone(),
        },
    ];
    app.activity.entries = vec![
        preview_oplog_entry(&checkout, 1846, "AgentInvocationFinished", None),
        preview_oplog_entry(&checkout, 1845, "End", Some(1843)),
        preview_oplog_entry(&checkout, 1844, "Log", None),
        preview_oplog_entry(&checkout, 1843, "Start", Some(1843)),
        preview_oplog_entry(&checkout, 1842, "AgentInvocationStarted", None),
        preview_oplog_entry(&orders, 918, "Error", None),
        preview_oplog_entry(&orders, 917, "AgentInvocationStarted", None),
        preview_oplog_entry(&orders, 916, "AgentInvocationStarted", None),
    ];
    app.activity.progress = vec![
        preview_oplog_progress(&checkout, 1844, true),
        preview_oplog_progress(&orders, 918, false),
    ];
    app.activity.paused = true;
}

fn preview_oplog_entry(
    agent: &AgentIdentity,
    index: u64,
    kind: &str,
    group: Option<u64>,
) -> ActivityEntry {
    ActivityEntry {
        component: agent.component.clone(),
        agent_id: agent.agent_id.clone(),
        presentation: OplogEntryPresentation {
            index,
            timestamp: golem_common::model::Timestamp::from(index),
            kind: kind.into(),
            group: group.map(OplogEntryGroupKey::OplogIndex),
            detail: preview_oplog_detail(index, kind, &agent.agent_id),
            attribution: None,
            entry: None,
            search_text: std::sync::OnceLock::new(),
        },
    }
}

fn preview_oplog_detail(index: u64, kind: &str, agent_id: &str) -> serde_json::Value {
    match kind {
        "AgentInvocationStarted" => serde_json::json!({
            "type": kind,
            "methodName": "checkout",
            "agentId": agent_id,
            "index": index,
        }),
        "AgentInvocationFinished" => serde_json::json!({
            "type": kind,
            "methodName": "checkout",
            "consumedFuel": 18742,
            "index": index,
        }),
        "Start" => serde_json::json!({
            "type": kind,
            "functionName": "golem:key-value/get",
            "index": index,
        }),
        "Log" => serde_json::json!({
            "type": kind,
            "message": "inventory reserved",
            "level": "info",
            "index": index,
        }),
        "Error" => serde_json::json!({
            "type": kind,
            "error": "payment provider timed out",
            "index": index,
        }),
        _ => serde_json::json!({"type": kind, "index": index}),
    }
}

fn preview_oplog_progress(
    agent: &AgentIdentity,
    last_index: u64,
    scan_limit_reached: bool,
) -> ActivityAgentProgress {
    ActivityAgentProgress {
        agent: ActivityAgent {
            component: agent.component.clone(),
            agent_id: agent.agent_id.clone(),
        },
        next: None,
        scanned_through_index: Some(last_index.saturating_sub(12)),
        last_index,
        oldest_loaded_index: Some(last_index.saturating_sub(99)),
        has_more: true,
        scan_limit_reached,
    }
}

fn populate_stream(app: &mut TuiApp) {
    let target = AgentIdentity {
        component: "checkout".into(),
        agent_id: "checkout(cart-17)".into(),
    };
    app.ops.open_stream(target.clone());
    app.stream.target = Some(target);
    app.stream.status = StreamStatus::Connected;
    let timestamp = golem_common::model::Timestamp::now_utc();
    app.stream.events = [
        AgentStreamItem::Stdout {
            timestamp,
            message: "cart restored with 3 items".into(),
        },
        AgentStreamItem::Log {
            timestamp,
            level: LogLevel::Info,
            context: "checkout".into(),
            message: "payment authorization started".into(),
        },
        AgentStreamItem::PluginError {
            timestamp,
            plugin_name: "fraud-check".into(),
            message: "remote score timed out; using policy fallback".into(),
        },
        AgentStreamItem::Stderr {
            timestamp,
            message: "retrying payment provider in 250ms".into(),
        },
    ]
    .into_iter()
    .collect();
    app.stream.buffered_bytes = 284;
    app.stream.server_missed = 2;
    app.stream.locally_dropped = 1;
}

fn populate_metrics_dashboard(app: &mut TuiApp) {
    let now = preview_now();
    let range = preview_metric_range(now);
    app.metrics.source = LocalMetricSource::Agents;
    app.metrics.mode = MetricsMode::Dashboard;
    app.metrics.time_range = Some(range);
    app.metrics.dashboard = Some(LocalMetricDashboardResponse {
        source: LocalMetricSource::Agents,
        range,
        generated_at_unix_nanos: now,
        freshest_sample_unix_nanos: Some(now.saturating_sub(4_000_000_000)),
        stats: vec![
            preview_metric_stat("invocations", "Invocations started", 1842.0, "1", false),
            preview_metric_stat("errors", "Error events", 7.0, "1", false),
            preview_metric_stat(
                "duration-p95",
                "Completed duration p95",
                18_400_000.0,
                "ns",
                true,
            ),
            preview_metric_stat("observed-agents", "Observed Agents", 326.0, "agents", false),
        ],
        panels: vec![
            preview_metric_panel(
                "invocation-rate",
                "Invocation rate",
                LocalMetricDashboardPanelKind::TimeSeries,
                vec![preview_dashboard_series(
                    "all",
                    "All invocations",
                    "invocations/s",
                    now,
                    &[8.0, 11.0, 9.0, 14.0, 18.0, 16.0, 21.0, 24.0],
                    "rate",
                )],
                range,
            ),
            preview_metric_panel(
                "completed-duration",
                "Completed duration",
                LocalMetricDashboardPanelKind::Distribution,
                vec![
                    preview_dashboard_series(
                        "p50",
                        "p50",
                        "ms",
                        now,
                        &[7.0, 8.0, 7.5, 9.0, 10.0, 8.5, 9.0, 9.5],
                        "p50",
                    ),
                    preview_dashboard_series(
                        "p95",
                        "p95",
                        "ms",
                        now,
                        &[14.0, 16.0, 15.0, 21.0, 19.0, 18.0, 23.0, 18.4],
                        "p95",
                    ),
                ],
                range,
            ),
            preview_metric_panel(
                "reliability-events",
                "Reliability events",
                LocalMetricDashboardPanelKind::TimeSeries,
                vec![preview_dashboard_series(
                    "errors",
                    "Errors",
                    "events",
                    now,
                    &[0.0, 1.0, 0.0, 2.0, 1.0, 0.0, 3.0, 0.0],
                    "count",
                )],
                range,
            ),
            preview_metric_panel(
                "top-agent-types",
                "Top Agent Types",
                LocalMetricDashboardPanelKind::Ranking,
                vec![
                    preview_dashboard_series(
                        "checkout",
                        "CheckoutAgent",
                        "invocations",
                        now,
                        &[412.0],
                        "sum",
                    ),
                    preview_dashboard_series(
                        "orders",
                        "OrderAgent",
                        "invocations",
                        now,
                        &[287.0],
                        "sum",
                    ),
                ],
                range,
            ),
            preview_metric_panel(
                "memory",
                "Observed last-known memory",
                LocalMetricDashboardPanelKind::TimeSeries,
                vec![preview_dashboard_series(
                    "memory",
                    "Agent memory",
                    "MiB",
                    now,
                    &[42.0, 43.0, 44.0, 48.0, 47.0, 51.0, 50.0, 52.0],
                    "value",
                )],
                range,
            ),
            preview_metric_panel(
                "resources",
                "Active resources",
                LocalMetricDashboardPanelKind::TimeSeries,
                vec![preview_dashboard_series(
                    "resources",
                    "Open resources",
                    "resources",
                    now,
                    &[11.0, 12.0, 12.0, 14.0, 15.0, 13.0, 16.0, 17.0],
                    "value",
                )],
                range,
            ),
        ],
    });
}

fn populate_metrics_explore(app: &mut TuiApp) {
    let now = preview_now();
    let range = preview_metric_range(now);
    app.metrics.source = LocalMetricSource::Agents;
    app.metrics.mode = MetricsMode::Explore;
    app.metrics.focus = MetricsFocus::Chart;
    app.metrics.time_range = Some(range);
    app.metrics.agent_scope = vec![
        AgentIdentity {
            component: "checkout".into(),
            agent_id: "checkout(cart-17)".into(),
        },
        AgentIdentity {
            component: "orders".into(),
            agent_id: "orders(order-04)".into(),
        },
    ];
    app.metrics.families = vec![
        preview_metric_family(
            "golem.agent.invocations",
            "Agent invocation events",
            "1",
            LocalMetricKind::Counter,
            18,
        ),
        preview_metric_family(
            "golem.agent.completed_duration",
            "Completed invocation duration",
            "ns",
            LocalMetricKind::Event,
            36,
        ),
        preview_metric_family(
            "checkout.cart.value",
            "User-defined cart value",
            "USD",
            LocalMetricKind::Gauge,
            12,
        ),
    ];
    app.metrics.series = vec![
        preview_metric_series(
            "checkout-rate",
            "rate",
            "checkout(cart-17)",
            "CheckoutAgent",
            24.0,
            now,
        ),
        preview_metric_series(
            "orders-rate",
            "rate",
            "orders(order-04)",
            "OrderAgent",
            17.0,
            now,
        ),
        preview_metric_series(
            "checkout-p95",
            "p95",
            "checkout(cart-17)",
            "CheckoutAgent",
            18.4,
            now,
        ),
    ];
    app.metrics.selected_series_keys = vec!["checkout-rate".into(), "orders-rate".into()];
    app.metrics.series_samples = vec![
        preview_series_samples(
            "checkout-rate",
            now,
            &[8.0, 11.0, 9.0, 14.0, 18.0, 16.0, 21.0, 24.0],
            "rate",
        ),
        preview_series_samples(
            "orders-rate",
            now,
            &[6.0, 7.0, 10.0, 8.0, 12.0, 13.0, 15.0, 17.0],
            "rate",
        ),
    ];
    app.metrics.chart_cursor = Some(6);
    app.metrics.families_next_cursor = Some("family-page-2".into());
    app.metrics.series_next_cursor = Some("series-page-2".into());
}

fn preview_metric_stat(
    id: &str,
    label: &str,
    value: f64,
    unit: &str,
    approximate: bool,
) -> LocalMetricDashboardStat {
    LocalMetricDashboardStat {
        id: id.into(),
        label: label.into(),
        value: Some(value),
        unit: unit.into(),
        approximate,
    }
}

fn preview_metric_panel(
    id: &str,
    title: &str,
    kind: LocalMetricDashboardPanelKind,
    series: Vec<LocalMetricDashboardSeries>,
    range: LocalMetricTimeRange,
) -> LocalMetricDashboardPanel {
    LocalMetricDashboardPanel {
        id: id.into(),
        title: title.into(),
        kind,
        series,
        empty_reason: None,
        observed_from_unix_nanos: Some(range.from_unix_nanos),
        observed_to_unix_nanos: Some(range.to_unix_nanos),
        approximate: false,
    }
}

fn preview_dashboard_series(
    key: &str,
    label: &str,
    unit: &str,
    now: i64,
    values: &[f64],
    statistic: &str,
) -> LocalMetricDashboardSeries {
    LocalMetricDashboardSeries {
        key: key.into(),
        label: label.into(),
        unit: unit.into(),
        samples: preview_metric_samples(now, values, statistic),
        latest_value: values.last().copied(),
        approximate: false,
    }
}

fn preview_metric_family(
    name: &str,
    description: &str,
    unit: &str,
    kind: LocalMetricKind,
    series_count: u64,
) -> LocalMetricFamily {
    LocalMetricFamily {
        name: name.into(),
        description: description.into(),
        unit: unit.into(),
        kind,
        temporality: LocalMetricTemporality::Delta,
        category: "agent".into(),
        series_count,
        latest_value: Some(24.0),
        latest_unix_nanos: Some(preview_now()),
    }
}

fn preview_metric_series(
    series_key: &str,
    statistic: &str,
    agent_id: &str,
    agent_type: &str,
    latest_value: f64,
    now: i64,
) -> LocalMetricSeries {
    LocalMetricSeries {
        series_key: series_key.into(),
        statistic: statistic.into(),
        temporality: LocalMetricTemporality::Delta,
        attributes: BTreeMap::from([
            ("agent.type".into(), agent_type.into()),
            (
                "component".into(),
                agent_type.trim_end_matches("Agent").to_lowercase(),
            ),
        ]),
        environment_id: None,
        component_id: None,
        agent_id: Some(agent_id.into()),
        agent_type: Some(agent_type.into()),
        latest_value: Some(latest_value),
        latest_unix_nanos: Some(now),
    }
}

fn preview_series_samples(
    series_key: &str,
    now: i64,
    values: &[f64],
    statistic: &str,
) -> metrics::MetricsSeriesSamples {
    let samples = preview_metric_samples(now, values, statistic);
    metrics::MetricsSeriesSamples {
        series_key: series_key.into(),
        response: LocalMetricSamplesResponse {
            observed_from_unix_nanos: samples.first().map(|sample| sample.timestamp_unix_nanos),
            observed_to_unix_nanos: samples.last().map(|sample| sample.timestamp_unix_nanos),
            original_point_count: samples.len() as u64,
            samples,
            approximate: false,
            bucket_width_unix_nanos: None,
        },
    }
}

fn preview_metric_samples(now: i64, values: &[f64], statistic: &str) -> Vec<LocalMetricSample> {
    let interval = 60 * 1_000_000_000_i64;
    let first = now.saturating_sub(interval.saturating_mul(values.len().saturating_sub(1) as i64));
    values
        .iter()
        .enumerate()
        .map(|(index, value)| LocalMetricSample {
            timestamp_unix_nanos: first.saturating_add(interval.saturating_mul(index as i64)),
            value: *value,
            statistic: statistic.into(),
            source_oplog_index: Some(1800 + index as u64),
        })
        .collect()
}

fn preview_metric_range(now: i64) -> LocalMetricTimeRange {
    LocalMetricTimeRange {
        from_unix_nanos: now.saturating_sub(15 * 60 * 1_000_000_000_i64),
        to_unix_nanos: now,
    }
}

fn preview_now() -> i64 {
    SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .min(i64::MAX as u128) as i64
}

fn populate_deployments(app: &mut TuiApp) {
    app.ops_environment.snapshot = Some(preview_environment_snapshot());
    app.deployments.history = (1..=12)
        .rev()
        .map(|revision| DeploymentRecord {
            revision,
            version: format!("demo-{revision}"),
            hash: format!("d933d8d24db984b94aa{revision:02}"),
        })
        .collect();
}

fn populate_configuration(app: &mut TuiApp) {
    let context_id = TuiContextId::new(1);
    let (generation, scope, kind) = app.resources.begin_load(context_id);
    app.resources.finish(
        generation,
        context_id,
        Ok(ResourcesRefreshPayload {
            scope,
            kind,
            rows: vec![
                ResourceRow {
                    name: "/payments/provider-token".into(),
                    summary: "revision 7 · metadata only".into(),
                    fields: vec![
                        ResourceField {
                            label: "Revision".into(),
                            value: "7".into(),
                        },
                        ResourceField {
                            label: "Created".into(),
                            value: "2026-09-23T10:14:00Z".into(),
                        },
                        ResourceField {
                            label: "Value".into(),
                            value: "masked; not requested by the TUI".into(),
                        },
                    ],
                },
                ResourceRow {
                    name: "/shipping/webhook-secret".into(),
                    summary: "revision 3 · metadata only".into(),
                    fields: vec![ResourceField {
                        label: "Revision".into(),
                        value: "3".into(),
                    }],
                },
            ],
        }),
    );
    app.resources.cycle_focus(true);
    debug_assert_eq!(app.resources.focus(), ResourceFocus::Items);
}
