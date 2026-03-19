use crate::export::build_otel_span;
use crate::helpers::{
    attribute_value_to_string, datetime_to_nanos, oplog_payload_size, timestamp_to_nanos,
    worker_error_to_string, wrapped_function_type_name,
};
use crate::otlp_json::{
    KeyValue, OtlpGauge, OtlpLogRecord, OtlpMetric, OtlpNumberDataPoint, OtlpSpan, OtlpSum,
    OtlpValue,
};
use crate::state::{PendingSpan, WorkerState};
use golem_rust::bindings::golem::api::oplog::{
    FailedUpdateParameters, FinishSpanParameters, GrowMemoryParameters, LogLevel, LogParameters,
    OplogEntry, RawAgentInvocationFinishedParameters, RawAgentInvocationStartedParameters,
    RawCreateParameters, RawCreateResourceParameters, RawDropResourceParameters,
    RawHostCallParameters, RawOplogProcessorCheckpointParameters, RawSnapshotParameters,
    RawSuccessfulUpdateParameters, RemoteTransactionParameters, SetSpanAttributeParameters,
    SpanData, StartSpanParameters,
};
use std::collections::HashMap;

pub(crate) struct ProcessingOutput {
    pub(crate) spans: Vec<OtlpSpan>,
    pub(crate) log_records: Vec<OtlpLogRecord>,
    pub(crate) metrics: Vec<OtlpMetric>,
}

pub(crate) fn process_entries(
    state: &mut WorkerState,
    entries: Vec<OplogEntry>,
) -> ProcessingOutput {
    let mut completed_spans: Vec<OtlpSpan> = Vec::new();
    let mut log_records: Vec<OtlpLogRecord> = Vec::new();
    let mut metrics: Vec<OtlpMetric> = Vec::new();

    for entry in entries {
        match entry {
            OplogEntry::Create(params) => {
                handle_create(state, params, &mut metrics);
            }
            OplogEntry::AgentInvocationStarted(params) => {
                handle_invocation_started(state, params, &mut metrics);
            }
            OplogEntry::StartSpan(params) => {
                handle_start_span(state, params);
            }
            OplogEntry::SetSpanAttribute(params) => {
                handle_set_span_attribute(state, params);
            }
            OplogEntry::FinishSpan(params) => {
                handle_finish_span(state, params, &mut completed_spans);
            }
            OplogEntry::AgentInvocationFinished(params) => {
                handle_invocation_finished(
                    state,
                    params,
                    &mut completed_spans,
                    &mut metrics,
                );
            }
            OplogEntry::Error(params) => {
                let error_msg = worker_error_to_string(&params.error);
                state.terminal_error = Some(error_msg.clone());

                let time_ns = datetime_to_nanos(&params.timestamp);

                handle_terminal(state, time_ns, true, &mut completed_spans);

                metrics.push(counter_metric(
                    "golem.error.count",
                    "1",
                    "Agent errors",
                    &time_ns.to_string(),
                    vec![KeyValue {
                        key: "error.type".to_string(),
                        value: OtlpValue {
                            string_value: worker_error_variant_name(&params.error),
                        },
                    }],
                ));

                log_records.push(OtlpLogRecord {
                    time_unix_nano: time_ns.to_string(),
                    observed_time_unix_nano: time_ns.to_string(),
                    severity_number: 17,
                    severity_text: "ERROR".to_string(),
                    body: Some(OtlpValue {
                        string_value: error_msg,
                    }),
                    attributes: vec![KeyValue {
                        key: "error.type".to_string(),
                        value: OtlpValue {
                            string_value: worker_error_variant_name(&params.error),
                        },
                    }],
                    trace_id: non_empty_trace_id(&state.trace_id),
                    span_id: None,
                });
            }
            OplogEntry::Interrupted(ts) => {
                let time_ns = timestamp_to_nanos(&ts);
                state.terminal_error = Some("interrupted".to_string());
                handle_terminal(state, time_ns, true, &mut completed_spans);

                metrics.push(counter_metric(
                    "golem.interruption.count",
                    "1",
                    "Agent interruptions",
                    &time_ns.to_string(),
                    Vec::new(),
                ));

                log_records.push(OtlpLogRecord {
                    time_unix_nano: time_ns.to_string(),
                    observed_time_unix_nano: time_ns.to_string(),
                    severity_number: 13,
                    severity_text: "WARN".to_string(),
                    body: Some(OtlpValue {
                        string_value: "Agent interrupted".to_string(),
                    }),
                    attributes: Vec::new(),
                    trace_id: non_empty_trace_id(&state.trace_id),
                    span_id: None,
                });
            }
            OplogEntry::Exited(ts) => {
                let time_ns = timestamp_to_nanos(&ts);
                state.terminal_error = Some("exited".to_string());
                handle_terminal(state, time_ns, true, &mut completed_spans);

                metrics.push(counter_metric(
                    "golem.exit.count",
                    "1",
                    "Agent exits",
                    &time_ns.to_string(),
                    Vec::new(),
                ));

                log_records.push(OtlpLogRecord {
                    time_unix_nano: time_ns.to_string(),
                    observed_time_unix_nano: time_ns.to_string(),
                    severity_number: 9,
                    severity_text: "INFO".to_string(),
                    body: Some(OtlpValue {
                        string_value: "Agent exited".to_string(),
                    }),
                    attributes: Vec::new(),
                    trace_id: non_empty_trace_id(&state.trace_id),
                    span_id: None,
                });
            }
            OplogEntry::Log(params) => {
                let time_ns = datetime_to_nanos(&params.timestamp).to_string();
                metrics.push(counter_metric(
                    "golem.log.count",
                    "1",
                    "Log message count",
                    &time_ns,
                    vec![KeyValue {
                        key: "level".to_string(),
                        value: OtlpValue {
                            string_value: log_level_severity_text(&params.level).to_string(),
                        },
                    }],
                ));
                handle_log(state, params, &mut log_records);
            }
            OplogEntry::GrowMemory(params) => {
                handle_grow_memory(state, params, &mut metrics);
            }
            OplogEntry::HostCall(params) => {
                handle_host_call(params, &mut metrics);
            }
            OplogEntry::PendingAgentInvocation(params) => {
                let time_ns = datetime_to_nanos(&params.timestamp).to_string();
                metrics.push(counter_metric(
                    "golem.invocation.pending_count",
                    "1",
                    "Pending invocation requests",
                    &time_ns,
                    Vec::new(),
                ));
            }
            OplogEntry::CreateResource(params) => {
                handle_create_resource(state, params, &mut metrics);
            }
            OplogEntry::DropResource(params) => {
                handle_drop_resource(state, params, &mut metrics);
            }
            OplogEntry::Restart(ts) => {
                let time_ns = timestamp_to_nanos(&ts).to_string();
                metrics.push(counter_metric(
                    "golem.restart.count",
                    "1",
                    "Agent restarts",
                    &time_ns,
                    Vec::new(),
                ));
            }
            OplogEntry::SuccessfulUpdate(params) => {
                handle_successful_update(params, &mut metrics);
            }
            OplogEntry::FailedUpdate(params) => {
                handle_failed_update(params, &mut metrics);
            }
            OplogEntry::CommittedRemoteTransaction(params) => {
                handle_committed_transaction(params, &mut metrics);
            }
            OplogEntry::RolledBackRemoteTransaction(params) => {
                handle_rolled_back_transaction(params, &mut metrics);
            }
            OplogEntry::Snapshot(params) => {
                handle_snapshot(params, &mut metrics);
            }
            OplogEntry::OplogProcessorCheckpoint(params) => {
                handle_oplog_processor_checkpoint(params, &mut metrics);
            }
            _ => {} // ignore all other entry types
        }
    }

    ProcessingOutput {
        spans: completed_spans,
        log_records,
        metrics,
    }
}

fn non_empty_trace_id(trace_id: &str) -> Option<String> {
    if trace_id.is_empty() {
        None
    } else {
        Some(trace_id.to_string())
    }
}

fn worker_error_variant_name(
    e: &golem_rust::bindings::golem::api::oplog::WorkerError,
) -> String {
    match e {
        golem_rust::bindings::golem::api::oplog::WorkerError::Unknown(_) => {
            "Unknown".to_string()
        }
        golem_rust::bindings::golem::api::oplog::WorkerError::InvalidRequest(_) => {
            "InvalidRequest".to_string()
        }
        golem_rust::bindings::golem::api::oplog::WorkerError::StackOverflow => {
            "StackOverflow".to_string()
        }
        golem_rust::bindings::golem::api::oplog::WorkerError::OutOfMemory => {
            "OutOfMemory".to_string()
        }
        golem_rust::bindings::golem::api::oplog::WorkerError::ExceededMemoryLimit => {
            "ExceededMemoryLimit".to_string()
        }
        golem_rust::bindings::golem::api::oplog::WorkerError::InternalError(_) => {
            "InternalError".to_string()
        }
    }
}

fn log_level_severity_number(level: &LogLevel) -> u32 {
    match level {
        LogLevel::Stdout => 1,
        LogLevel::Stderr => 13,
        LogLevel::Trace => 1,
        LogLevel::Debug => 5,
        LogLevel::Info => 9,
        LogLevel::Warn => 13,
        LogLevel::Error => 17,
        LogLevel::Critical => 21,
    }
}

fn log_level_severity_text(level: &LogLevel) -> &'static str {
    match level {
        LogLevel::Stdout => "STDOUT",
        LogLevel::Stderr => "STDERR",
        LogLevel::Trace => "TRACE",
        LogLevel::Debug => "DEBUG",
        LogLevel::Info => "INFO",
        LogLevel::Warn => "WARN",
        LogLevel::Error => "ERROR",
        LogLevel::Critical => "CRITICAL",
    }
}

fn handle_log(
    state: &WorkerState,
    params: LogParameters,
    log_records: &mut Vec<OtlpLogRecord>,
) {
    let time_ns = datetime_to_nanos(&params.timestamp).to_string();
    let mut attributes = Vec::new();
    if !params.context.is_empty() {
        attributes.push(KeyValue {
            key: "log.context".to_string(),
            value: OtlpValue {
                string_value: params.context,
            },
        });
    }

    log_records.push(OtlpLogRecord {
        time_unix_nano: time_ns.clone(),
        observed_time_unix_nano: time_ns,
        severity_number: log_level_severity_number(&params.level),
        severity_text: log_level_severity_text(&params.level).to_string(),
        body: Some(OtlpValue {
            string_value: params.message,
        }),
        attributes,
        trace_id: non_empty_trace_id(&state.trace_id),
        span_id: None,
    });
}

fn counter_metric(
    name: &str,
    unit: &str,
    description: &str,
    time_ns: &str,
    attributes: Vec<KeyValue>,
) -> OtlpMetric {
    OtlpMetric {
        name: name.to_string(),
        unit: unit.to_string(),
        description: description.to_string(),
        sum: Some(OtlpSum {
            aggregation_temporality: 1,
            is_monotonic: true,
            data_points: vec![OtlpNumberDataPoint {
                start_time_unix_nano: time_ns.to_string(),
                time_unix_nano: time_ns.to_string(),
                as_int: Some("1".to_string()),
                as_double: None,
                attributes,
            }],
        }),
        gauge: None,
    }
}

fn gauge_metric(
    name: &str,
    unit: &str,
    description: &str,
    time_ns: &str,
    value: u64,
) -> OtlpMetric {
    OtlpMetric {
        name: name.to_string(),
        unit: unit.to_string(),
        description: description.to_string(),
        sum: None,
        gauge: Some(OtlpGauge {
            data_points: vec![OtlpNumberDataPoint {
                start_time_unix_nano: time_ns.to_string(),
                time_unix_nano: time_ns.to_string(),
                as_int: Some(value.to_string()),
                as_double: None,
                attributes: Vec::new(),
            }],
        }),
    }
}

fn handle_create(
    state: &mut WorkerState,
    params: RawCreateParameters,
    metrics: &mut Vec<OtlpMetric>,
) {
    let time_ns = datetime_to_nanos(&params.timestamp).to_string();

    state.total_memory_bytes = params.initial_total_linear_memory_size;

    metrics.push(gauge_metric(
        "golem.memory.initial_bytes",
        "By",
        "Initial linear memory size",
        &time_ns,
        params.initial_total_linear_memory_size,
    ));

    metrics.push(gauge_metric(
        "golem.memory.total_bytes",
        "By",
        "Total linear memory size",
        &time_ns,
        state.total_memory_bytes,
    ));

    metrics.push(gauge_metric(
        "golem.component.size_bytes",
        "By",
        "Component size",
        &time_ns,
        params.component_size,
    ));
}

fn handle_grow_memory(state: &mut WorkerState, params: GrowMemoryParameters, metrics: &mut Vec<OtlpMetric>) {
    let time_ns = datetime_to_nanos(&params.timestamp).to_string();

    state.total_memory_bytes += params.delta;

    metrics.push(OtlpMetric {
        name: "golem.memory.growth_bytes".to_string(),
        unit: "By".to_string(),
        description: "Linear memory growth".to_string(),
        sum: Some(OtlpSum {
            aggregation_temporality: 1,
            is_monotonic: true,
            data_points: vec![OtlpNumberDataPoint {
                start_time_unix_nano: time_ns.clone(),
                time_unix_nano: time_ns.clone(),
                as_int: Some(params.delta.to_string()),
                as_double: None,
                attributes: Vec::new(),
            }],
        }),
        gauge: None,
    });

    metrics.push(gauge_metric(
        "golem.memory.total_bytes",
        "By",
        "Total linear memory size",
        &time_ns,
        state.total_memory_bytes,
    ));
}

fn handle_host_call(params: RawHostCallParameters, metrics: &mut Vec<OtlpMetric>) {
    let time_ns = datetime_to_nanos(&params.timestamp).to_string();
    let fn_type = wrapped_function_type_name(&params.durable_function_type);
    metrics.push(OtlpMetric {
        name: "golem.host_call.count".to_string(),
        unit: "1".to_string(),
        description: "Host function calls".to_string(),
        sum: Some(OtlpSum {
            aggregation_temporality: 1,
            is_monotonic: true,
            data_points: vec![OtlpNumberDataPoint {
                start_time_unix_nano: time_ns.clone(),
                time_unix_nano: time_ns,
                as_int: Some("1".to_string()),
                as_double: None,
                attributes: vec![
                    KeyValue {
                        key: "function.name".to_string(),
                        value: OtlpValue {
                            string_value: params.function_name,
                        },
                    },
                    KeyValue {
                        key: "durable_function_type".to_string(),
                        value: OtlpValue {
                            string_value: fn_type.to_string(),
                        },
                    },
                ],
            }],
        }),
        gauge: None,
    });
}

fn handle_invocation_started(
    state: &mut WorkerState,
    params: RawAgentInvocationStartedParameters,
    metrics: &mut Vec<OtlpMetric>,
) {
    let time_ns = datetime_to_nanos(&params.timestamp);
    state.invocation_start_ns = Some(time_ns);

    metrics.push(counter_metric(
        "golem.invocation.count",
        "1",
        "Invocation count",
        &time_ns.to_string(),
        Vec::new(),
    ));

    if !state.pending_spans.is_empty() || !state.implicit_spans.is_empty() {
        println!(
            "OTLP exporter: new invocation started with {} pending and {} implicit spans still open, discarding",
            state.pending_spans.len(),
            state.implicit_spans.len()
        );
    }
    state.pending_spans.clear();
    state.implicit_spans.clear();
    state.terminal_error = None;
    state.inherited_span_parents.clear();

    state.trace_id = params.trace_id;
    state.trace_states = params.trace_states;

    // First pass: build a raw map of inherited span_id → parent_span_id.
    // External spans are roots in the inherited chain (parent = None).
    let mut raw_inherited: HashMap<String, Option<String>> = HashMap::new();

    for span_data in &params.invocation_context {
        match span_data {
            SpanData::LocalSpan(local) if local.inherited => {
                raw_inherited.insert(local.span_id.clone(), local.parent.clone());
            }
            SpanData::ExternalSpan(ext) => {
                raw_inherited.insert(ext.span_id.clone(), None);
            }
            _ => {}
        }
    }

    // Resolve each inherited entry: follow the parent chain through the
    // inherited map until reaching a parent that is NOT in the map (i.e. it
    // was exported by the originating worker) or None (root).
    let resolved: HashMap<String, Option<String>> = raw_inherited
        .keys()
        .map(|span_id| {
            let resolved_parent = resolve_inherited_parent(span_id, &raw_inherited);
            (span_id.clone(), resolved_parent)
        })
        .collect();

    state.inherited_span_parents = resolved;

    // Second pass: collect non-inherited local spans, resolving parents
    // through the inherited map when necessary.
    for span_data in params.invocation_context {
        match span_data {
            SpanData::LocalSpan(local) if !local.inherited => {
                let attrs: HashMap<String, String> = local
                    .attributes
                    .into_iter()
                    .map(|a| (a.key, attribute_value_to_string(&a.value)))
                    .collect();

                let parent = resolve_parent_through_inherited(
                    local.parent,
                    &state.inherited_span_parents,
                );

                state.implicit_spans.push(PendingSpan {
                    span_id: local.span_id,
                    parent_span_id: parent,
                    start_time_ns: datetime_to_nanos(&local.start),
                    attributes: attrs,
                });
            }
            _ => {}
        }
    }
}

/// Given a span_id in the inherited map, follow the parent chain until we find
/// a parent that is NOT itself in the map (meaning it was exported by the
/// originating worker), or `None` if the chain ends at a root.
fn resolve_inherited_parent(
    span_id: &str,
    inherited: &HashMap<String, Option<String>>,
) -> Option<String> {
    let mut current = span_id;
    loop {
        match inherited.get(current) {
            Some(Some(parent)) => {
                if inherited.contains_key(parent.as_str()) {
                    current = parent.as_str();
                } else {
                    // Parent is not inherited — it's the real ancestor
                    return Some(parent.clone());
                }
            }
            Some(None) => {
                // This entry is a root (external span or chain end)
                return None;
            }
            None => {
                // Not in the map — shouldn't happen for the initial call
                return None;
            }
        }
    }
}

/// If `parent` points to an inherited span, resolve through the inherited map
/// to find the real (non-inherited) ancestor. Otherwise return as-is.
fn resolve_parent_through_inherited(
    parent: Option<String>,
    inherited: &HashMap<String, Option<String>>,
) -> Option<String> {
    match parent {
        Some(ref pid) if inherited.contains_key(pid.as_str()) => {
            inherited.get(pid.as_str()).cloned().flatten()
        }
        other => other,
    }
}

fn handle_start_span(state: &mut WorkerState, params: StartSpanParameters) {
    let attrs: HashMap<String, String> = params
        .attributes
        .into_iter()
        .map(|a| (a.key, attribute_value_to_string(&a.value)))
        .collect();

    let parent = resolve_parent_through_inherited(params.parent, &state.inherited_span_parents);

    state.pending_spans.insert(
        params.span_id.clone(),
        PendingSpan {
            span_id: params.span_id,
            parent_span_id: parent,
            start_time_ns: datetime_to_nanos(&params.timestamp),
            attributes: attrs,
        },
    );
}

fn handle_set_span_attribute(state: &mut WorkerState, params: SetSpanAttributeParameters) {
    let value = attribute_value_to_string(&params.value);

    if let Some(span) = state.pending_spans.get_mut(&params.span_id) {
        span.attributes.insert(params.key, value);
        return;
    }

    for span in &mut state.implicit_spans {
        if span.span_id == params.span_id {
            span.attributes.insert(params.key, value);
            return;
        }
    }

    println!(
        "OTLP exporter: set-span-attribute for unknown span {}",
        params.span_id
    );
}

fn handle_finish_span(
    state: &mut WorkerState,
    params: FinishSpanParameters,
    completed: &mut Vec<OtlpSpan>,
) {
    if let Some(span) = state.pending_spans.remove(&params.span_id) {
        let end_time_ns = datetime_to_nanos(&params.timestamp);
        let trace_state = combined_trace_state(&state.trace_states);
        completed.push(build_otel_span(
            &state.trace_id,
            trace_state.as_deref(),
            span,
            end_time_ns,
            false,
            None,
        ));
    } else {
        println!(
            "OTLP exporter: finish-span for unknown span {}",
            params.span_id
        );
    }
}

fn handle_invocation_finished(
    state: &mut WorkerState,
    params: RawAgentInvocationFinishedParameters,
    completed: &mut Vec<OtlpSpan>,
    metrics: &mut Vec<OtlpMetric>,
) {
    let end_time_ns = datetime_to_nanos(&params.timestamp);
    flush_implicit_spans(state, end_time_ns, false, completed);
    flush_remaining_explicit_spans(state, end_time_ns, false, completed);

    let time_ns = end_time_ns.to_string();

    if let Some(start_ns) = state.invocation_start_ns.take() {
        let duration_ns = end_time_ns.saturating_sub(start_ns);
        metrics.push(OtlpMetric {
            name: "golem.invocation.duration_ns".to_string(),
            unit: "ns".to_string(),
            description: "Invocation duration".to_string(),
            sum: Some(OtlpSum {
                aggregation_temporality: 1,
                is_monotonic: true,
                data_points: vec![OtlpNumberDataPoint {
                    start_time_unix_nano: time_ns.clone(),
                    time_unix_nano: time_ns.clone(),
                    as_int: Some(duration_ns.to_string()),
                    as_double: None,
                    attributes: Vec::new(),
                }],
            }),
            gauge: None,
        });
    }

    if params.consumed_fuel > 0 {
        metrics.push(OtlpMetric {
            name: "golem.invocation.fuel_consumed".to_string(),
            unit: "1".to_string(),
            description: "Fuel consumed by the invocation".to_string(),
            sum: Some(OtlpSum {
                aggregation_temporality: 1,
                is_monotonic: true,
                data_points: vec![OtlpNumberDataPoint {
                    start_time_unix_nano: time_ns.clone(),
                    time_unix_nano: time_ns,
                    as_int: Some(params.consumed_fuel.to_string()),
                    as_double: None,
                    attributes: Vec::new(),
                }],
            }),
            gauge: None,
        });
    }
}

fn handle_terminal(
    state: &mut WorkerState,
    end_time_ns: u128,
    is_error: bool,
    completed: &mut Vec<OtlpSpan>,
) {
    flush_implicit_spans(state, end_time_ns, is_error, completed);
    flush_remaining_explicit_spans(state, end_time_ns, is_error, completed);
}

fn combined_trace_state(trace_states: &[String]) -> Option<String> {
    if trace_states.is_empty() {
        None
    } else {
        Some(trace_states.join(","))
    }
}

fn flush_implicit_spans(
    state: &mut WorkerState,
    end_time_ns: u128,
    is_error: bool,
    completed: &mut Vec<OtlpSpan>,
) {
    let error_msg = state.terminal_error.clone();
    let trace_id = state.trace_id.clone();
    let trace_state = combined_trace_state(&state.trace_states);
    let spans = std::mem::take(&mut state.implicit_spans);

    for span in spans {
        completed.push(build_otel_span(
            &trace_id,
            trace_state.as_deref(),
            span,
            end_time_ns,
            is_error,
            error_msg.as_deref(),
        ));
    }
}

fn flush_remaining_explicit_spans(
    state: &mut WorkerState,
    end_time_ns: u128,
    is_error: bool,
    completed: &mut Vec<OtlpSpan>,
) {
    let error_msg = state.terminal_error.clone();
    let trace_id = state.trace_id.clone();
    let trace_state = combined_trace_state(&state.trace_states);
    let spans: Vec<PendingSpan> = state.pending_spans.drain().map(|(_, v)| v).collect();

    for span in spans {
        completed.push(build_otel_span(
            &trace_id,
            trace_state.as_deref(),
            span,
            end_time_ns,
            is_error,
            error_msg.as_deref(),
        ));
    }
}

fn handle_create_resource(
    state: &mut WorkerState,
    params: RawCreateResourceParameters,
    metrics: &mut Vec<OtlpMetric>,
) {
    let time_ns = datetime_to_nanos(&params.timestamp).to_string();

    state.active_resources += 1;

    metrics.push(counter_metric(
        "golem.resources.created",
        "1",
        "Resource instances created",
        &time_ns,
        Vec::new(),
    ));

    metrics.push(OtlpMetric {
        name: "golem.resources.active".to_string(),
        unit: "1".to_string(),
        description: "Active resource instances".to_string(),
        sum: Some(OtlpSum {
            aggregation_temporality: 1,
            is_monotonic: false,
            data_points: vec![OtlpNumberDataPoint {
                start_time_unix_nano: time_ns.clone(),
                time_unix_nano: time_ns,
                as_int: Some(state.active_resources.to_string()),
                as_double: None,
                attributes: Vec::new(),
            }],
        }),
        gauge: None,
    });
}

fn handle_drop_resource(
    state: &mut WorkerState,
    params: RawDropResourceParameters,
    metrics: &mut Vec<OtlpMetric>,
) {
    let time_ns = datetime_to_nanos(&params.timestamp).to_string();

    state.active_resources = (state.active_resources - 1).max(0);

    metrics.push(counter_metric(
        "golem.resources.dropped",
        "1",
        "Resource instances dropped",
        &time_ns,
        Vec::new(),
    ));

    metrics.push(OtlpMetric {
        name: "golem.resources.active".to_string(),
        unit: "1".to_string(),
        description: "Active resource instances".to_string(),
        sum: Some(OtlpSum {
            aggregation_temporality: 1,
            is_monotonic: false,
            data_points: vec![OtlpNumberDataPoint {
                start_time_unix_nano: time_ns.clone(),
                time_unix_nano: time_ns,
                as_int: Some(state.active_resources.to_string()),
                as_double: None,
                attributes: Vec::new(),
            }],
        }),
        gauge: None,
    });
}

fn handle_successful_update(params: RawSuccessfulUpdateParameters, metrics: &mut Vec<OtlpMetric>) {
    let time_ns = datetime_to_nanos(&params.timestamp).to_string();

    metrics.push(counter_metric(
        "golem.update.success_count",
        "1",
        "Successful component updates",
        &time_ns,
        Vec::new(),
    ));

    metrics.push(gauge_metric(
        "golem.component.size_bytes",
        "By",
        "Component size",
        &time_ns,
        params.new_component_size,
    ));
}

fn handle_failed_update(params: FailedUpdateParameters, metrics: &mut Vec<OtlpMetric>) {
    let time_ns = datetime_to_nanos(&params.timestamp).to_string();
    metrics.push(counter_metric(
        "golem.update.failure_count",
        "1",
        "Failed component updates",
        &time_ns,
        Vec::new(),
    ));
}

fn handle_committed_transaction(
    params: RemoteTransactionParameters,
    metrics: &mut Vec<OtlpMetric>,
) {
    let time_ns = datetime_to_nanos(&params.timestamp).to_string();
    metrics.push(counter_metric(
        "golem.transaction.committed",
        "1",
        "Committed remote transactions",
        &time_ns,
        Vec::new(),
    ));
}

fn handle_rolled_back_transaction(
    params: RemoteTransactionParameters,
    metrics: &mut Vec<OtlpMetric>,
) {
    let time_ns = datetime_to_nanos(&params.timestamp).to_string();
    metrics.push(counter_metric(
        "golem.transaction.rolled_back",
        "1",
        "Rolled back remote transactions",
        &time_ns,
        Vec::new(),
    ));
}

fn handle_snapshot(params: RawSnapshotParameters, metrics: &mut Vec<OtlpMetric>) {
    let time_ns = datetime_to_nanos(&params.timestamp).to_string();
    if let Some(size) = oplog_payload_size(&params.data) {
        metrics.push(OtlpMetric {
            name: "golem.snapshot.size_bytes".to_string(),
            unit: "By".to_string(),
            description: "Snapshot size".to_string(),
            sum: Some(OtlpSum {
                aggregation_temporality: 1,
                is_monotonic: true,
                data_points: vec![OtlpNumberDataPoint {
                    start_time_unix_nano: time_ns.clone(),
                    time_unix_nano: time_ns,
                    as_int: Some(size.to_string()),
                    as_double: None,
                    attributes: Vec::new(),
                }],
            }),
            gauge: None,
        });
    }
}

fn handle_oplog_processor_checkpoint(
    params: RawOplogProcessorCheckpointParameters,
    metrics: &mut Vec<OtlpMetric>,
) {
    let time_ns = datetime_to_nanos(&params.timestamp).to_string();
    let lag = params.sending_up_to.saturating_sub(params.confirmed_up_to);
    metrics.push(OtlpMetric {
        name: "golem.oplog_processor.lag".to_string(),
        unit: "1".to_string(),
        description: "Oplog processor delivery lag (entries)".to_string(),
        sum: None,
        gauge: Some(OtlpGauge {
            data_points: vec![OtlpNumberDataPoint {
                start_time_unix_nano: time_ns.clone(),
                time_unix_nano: time_ns,
                as_int: Some(lag.to_string()),
                as_double: None,
                attributes: Vec::new(),
            }],
        }),
    });
}
