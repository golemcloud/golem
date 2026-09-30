use crate::export::build_otel_span;
use crate::helpers::{
    attribute_value_to_string, datetime_to_nanos, oplog_payload_size, timestamp_to_nanos,
    worker_error_to_string, wrapped_function_type_name,
};
use crate::otlp_json::{
    KeyValue, OtlpGauge, OtlpLogRecord, OtlpMetric, OtlpNumberDataPoint, OtlpSpan, OtlpSum,
    OtlpValue,
};
use crate::state::{PendingSpan, PendingSpanLink, WorkerState};
use golem_rust::bindings::golem::api::oplog::{
    AgentMode, DurableStreamEventSummary, DurableStreamOutcome, FailedUpdateParameters,
    GrowMemoryParameters, LogLevel, LogParameters, OplogEntry, OwnerKind,
    RawAgentInvocationFinishedParameters, RawAgentInvocationStartedParameters,
    RawCancelledParameters, RawCreateParameters, RawCreateResourceParameters,
    RawDropResourceParameters, RawEndParameters, RawOplogProcessorCheckpointParameters,
    RawSnapshotParameters, RawStartParameters, RawSuccessfulUpdateParameters,
    RemoteTransactionParameters, SpanAttributes, SpanData, SpanFinished, SpanKind, SpanOutcome,
    SpanStarted,
};
use std::collections::HashMap;

pub(crate) struct ProcessingOutput {
    pub(crate) spans: Vec<OtlpSpan>,
    pub(crate) log_records: Vec<OtlpLogRecord>,
    pub(crate) metrics: Vec<OtlpMetric>,
}

#[cfg(test)]
pub(crate) fn process_entries(
    state: &mut WorkerState,
    entries: Vec<OplogEntry>,
) -> ProcessingOutput {
    process_entries_from(state, 0, entries)
}

pub(crate) fn process_entries_from(
    state: &mut WorkerState,
    first_entry_index: u64,
    entries: Vec<OplogEntry>,
) -> ProcessingOutput {
    let mut completed_spans: Vec<OtlpSpan> = Vec::new();
    let mut log_records: Vec<OtlpLogRecord> = Vec::new();
    let mut metrics: Vec<OtlpMetric> = Vec::new();

    for (offset, entry) in entries.into_iter().enumerate() {
        let entry_index = first_entry_index.saturating_add(offset as u64);
        match entry {
            OplogEntry::Create(params) => {
                handle_create(state, params, &mut metrics);
            }
            OplogEntry::AgentInvocationStarted(params) => {
                handle_invocation_started(
                    state,
                    params,
                    entry_index,
                    &mut completed_spans,
                    &mut metrics,
                );
            }
            OplogEntry::AgentInvocationFinished(params) => {
                handle_invocation_finished(state, params, &mut completed_spans, &mut metrics);
            }
            OplogEntry::Error(params) => {
                let error_msg = worker_error_to_string(&params.error);
                let time_ns = datetime_to_nanos(&params.timestamp);
                state.terminal_error = Some((error_msg.clone(), time_ns));
                let error_kind = oplog_error_kind_name(&params.kind);

                metrics.push(counter_metric(
                    "golem.error.count",
                    "1",
                    "Agent errors",
                    &time_ns.to_string(),
                    vec![
                        KeyValue {
                            key: "error.type".to_string(),
                            value: OtlpValue {
                                string_value: worker_error_variant_name(&params.error),
                            },
                        },
                        KeyValue {
                            key: "error.kind".to_string(),
                            value: OtlpValue {
                                string_value: error_kind.to_string(),
                            },
                        },
                    ],
                ));

                log_records.push(OtlpLogRecord {
                    time_unix_nano: time_ns.to_string(),
                    observed_time_unix_nano: time_ns.to_string(),
                    severity_number: 17,
                    severity_text: "ERROR".to_string(),
                    body: Some(OtlpValue {
                        string_value: error_msg,
                    }),
                    attributes: vec![
                        KeyValue {
                            key: "error.type".to_string(),
                            value: OtlpValue {
                                string_value: worker_error_variant_name(&params.error),
                            },
                        },
                        KeyValue {
                            key: "error.kind".to_string(),
                            value: OtlpValue {
                                string_value: error_kind.to_string(),
                            },
                        },
                    ],
                    trace_id: non_empty_trace_id(&state.trace_id),
                    span_id: None,
                });
            }
            OplogEntry::Interrupted(ts) => {
                let time_ns = timestamp_to_nanos(&ts);
                state.terminal_error = Some(("interrupted".to_string(), time_ns));
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
                state.terminal_error = Some(("exited".to_string(), time_ns));
                flush_implicit_spans(state, time_ns, true, &mut completed_spans);

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
            OplogEntry::Start(params) => {
                handle_start(state, params, entry_index, &mut metrics);
            }
            OplogEntry::End(params) => {
                handle_end(state, params, &mut completed_spans);
            }
            OplogEntry::Cancelled(params) => {
                handle_cancelled(state, params, &mut completed_spans);
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
            OplogEntry::RecoverySucceeded(ts) => {
                let time_ns = timestamp_to_nanos(&ts).to_string();
                state.terminal_error = None;
                metrics.push(counter_metric(
                    "golem.recovery.success_count",
                    "1",
                    "Successful agent recoveries",
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
            OplogEntry::StreamRegistered(params)
            | OplogEntry::StreamItems(params)
            | OplogEntry::StreamEnd(params)
            | OplogEntry::StreamCancel(params)
            | OplogEntry::StreamSession(params) => {
                if let Some(summary) = params.summary {
                    handle_stream_summary(&params.timestamp, summary, &mut metrics);
                }
            }
            OplogEntry::Revert(params) => {
                discard_openings_in_region(state, &params.dropped_region);
            }
            OplogEntry::Jump(params) => {
                discard_openings_in_region(state, &params.jump);
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

fn handle_stream_summary(
    timestamp: &golem_rust::wasip3::clocks::system_clock::Instant,
    summary: DurableStreamEventSummary,
    metrics: &mut Vec<OtlpMetric>,
) {
    let time = datetime_to_nanos(timestamp).to_string();
    let (event, count, outcome) = match summary {
        DurableStreamEventSummary::Registered => ("registered", 1, None),
        DurableStreamEventSummary::Items(count) => ("items", count, None),
        DurableStreamEventSummary::End(value) => ("end", 1, Some(value)),
        DurableStreamEventSummary::Cancelled => ("cancelled", 1, None),
        DurableStreamEventSummary::SessionResult => ("session-result", 1, None),
        DurableStreamEventSummary::SessionFinished(value) => ("session-finished", 1, Some(value)),
        DurableStreamEventSummary::SessionCancellation => ("session-cancellation", 1, None),
        DurableStreamEventSummary::SessionExpired => ("session-expired", 1, None),
    };
    let mut attributes = vec![KeyValue {
        key: "event".to_string(),
        value: OtlpValue {
            string_value: event.to_string(),
        },
    }];
    if let Some(outcome) = outcome {
        attributes.push(KeyValue {
            key: "outcome".to_string(),
            value: OtlpValue {
                string_value: match outcome {
                    DurableStreamOutcome::Success => "success",
                    DurableStreamOutcome::Error => "error",
                }
                .to_string(),
            },
        });
    }
    metrics.push(counter_metric(
        "golem.durable_stream.event_count",
        "1",
        "Durable stream events and logical items",
        &time,
        attributes,
    ));
    if count != 1 {
        metrics
            .last_mut()
            .unwrap()
            .sum
            .as_mut()
            .unwrap()
            .data_points[0]
            .as_int = Some(count.to_string());
    }
}

fn discard_openings_in_region(
    state: &mut WorkerState,
    region: &golem_rust::bindings::golem::api::oplog::OplogRegion,
) {
    let is_inside = |index: u64| index >= region.start && index <= region.end;
    state
        .pending_spans
        .retain(|_, span| !span.opening_index.is_some_and(is_inside));
    state
        .implicit_spans
        .retain(|span| !span.opening_index.is_some_and(is_inside));
}

fn oplog_error_kind_name(
    kind: &golem_rust::bindings::golem::api::oplog::OplogErrorKind,
) -> &'static str {
    use golem_rust::bindings::golem::api::oplog::OplogErrorKind;
    match kind {
        OplogErrorKind::Invocation => "invocation",
        OplogErrorKind::Recovery => "recovery",
    }
}

fn non_empty_trace_id(trace_id: &str) -> Option<String> {
    if trace_id.is_empty() {
        None
    } else {
        Some(trace_id.to_string())
    }
}

fn worker_error_variant_name(e: &golem_rust::bindings::golem::api::oplog::WorkerError) -> String {
    match e {
        golem_rust::bindings::golem::api::oplog::WorkerError::Unknown(_) => "Unknown".to_string(),
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
        golem_rust::bindings::golem::api::oplog::WorkerError::DeterministicTrap(_) => {
            "DeterministicTrap".to_string()
        }
        golem_rust::bindings::golem::api::oplog::WorkerError::TransientError(_) => {
            "TransientError".to_string()
        }
        golem_rust::bindings::golem::api::oplog::WorkerError::PermanentError(_) => {
            "PermanentError".to_string()
        }
        golem_rust::bindings::golem::api::oplog::WorkerError::ExceededTableLimit => {
            "ExceededTableLimit".to_string()
        }
        golem_rust::bindings::golem::api::oplog::WorkerError::ExceededHttpCallLimit => {
            "ExceededHttpCallLimit".to_string()
        }
        golem_rust::bindings::golem::api::oplog::WorkerError::ExceededRpcCallLimit => {
            "ExceededRpcCallLimit".to_string()
        }
        golem_rust::bindings::golem::api::oplog::WorkerError::AgentTerminatedByQuota(_) => {
            "AgentTerminatedByQuota".to_string()
        }
        golem_rust::bindings::golem::api::oplog::WorkerError::EphemeralSleepTooLong(_) => {
            "EphemeralSleepTooLong".to_string()
        }
        golem_rust::bindings::golem::api::oplog::WorkerError::EphemeralFuelExhausted(_) => {
            "EphemeralFuelExhausted".to_string()
        }
        golem_rust::bindings::golem::api::oplog::WorkerError::EphemeralCannotSuspend(_) => {
            "EphemeralCannotSuspend".to_string()
        }
        golem_rust::bindings::golem::api::oplog::WorkerError::ReadOnlyViolation(_) => {
            "ReadOnlyViolation".to_string()
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

fn handle_log(_state: &WorkerState, params: LogParameters, log_records: &mut Vec<OtlpLogRecord>) {
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
        trace_id: params
            .trace_context
            .as_ref()
            .map(|context| context.trace_id.clone()),
        span_id: params.trace_context.map(|context| context.span_id),
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

    state.trace_id.clear();
    state.trace_states.clear();
    state.pending_spans.clear();
    state.implicit_spans.clear();
    state.terminal_error = None;
    state.invocation_start_ns = None;
    state.active_resources = 0;
    state.total_memory_bytes = params.initial_total_linear_memory_size;
    state.resource_identity = Some(crate::state::ResourceIdentity {
        instance_id: (params.instance_id.high_bits, params.instance_id.low_bits),
        environment_id: (
            params.environment_id.uuid.high_bits,
            params.environment_id.uuid.low_bits,
        ),
        agent_mode: match params.agent_mode {
            AgentMode::Durable => "durable",
            AgentMode::Ephemeral => "ephemeral",
        },
        owner_kind: match params.owner_kind {
            OwnerKind::ComponentAgent => "component-agent",
            OwnerKind::EphemeralExternalTool => "ephemeral-external-tool",
        },
    });

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

fn handle_grow_memory(
    state: &mut WorkerState,
    params: GrowMemoryParameters,
    metrics: &mut Vec<OtlpMetric>,
) {
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

fn handle_start(
    state: &mut WorkerState,
    params: RawStartParameters,
    entry_index: u64,
    metrics: &mut Vec<OtlpMetric>,
) {
    if let Some(started) = params.span_started.clone() {
        handle_span_started(state, started, Some(entry_index));
    }
    // Phase 1 of the concurrent durability refactor: `Start` covers both real
    // host calls and synthetic durable scope markers (e.g. batched-write
    // scopes). Only real host calls carry a `request` payload, and scope
    // markers use `<scope:...>` function names — exclude both so this metric
    // stays semantically equivalent to the legacy `HostCall` counter.
    if params.request.is_none()
        || params.function_name.starts_with("<scope:")
        || is_internal_span_operation(&params.function_name)
    {
        return;
    }
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

fn is_internal_span_operation(function_name: &str) -> bool {
    matches!(
        function_name,
        "golem::api::context::start-span"
            | "golem::api::context::span::finish"
            | "golem::api::context::span::drop"
            | "golem::api::context::span::set-attributes"
            | "golem::rpc::wasm-rpc::drop"
            | "http::client::span-cleanup"
    )
}

fn handle_invocation_started(
    state: &mut WorkerState,
    params: RawAgentInvocationStartedParameters,
    entry_index: u64,
    completed: &mut Vec<OtlpSpan>,
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

    if !state.implicit_spans.is_empty() {
        // Retries replay their original invocation start. A new recorded start cannot
        // resume these invocation-owned spans, even when a revert permits key reuse.
        let (_, end_time_ns) = state.terminal_error.get_or_insert_with(|| {
            (
                "invocation superseded before completion".to_string(),
                time_ns,
            )
        });
        let end_time_ns = *end_time_ns;
        flush_implicit_spans(state, end_time_ns, true, completed);
    }
    state.terminal_error = None;

    state.trace_id = params.trace_id;
    state.trace_states = params.trace_states;

    for span_data in params.invocation_context {
        match span_data {
            SpanData::LocalSpan(local) if !local.inherited => {
                let attrs: HashMap<String, String> = local
                    .attributes
                    .into_iter()
                    .map(|a| (a.key, attribute_value_to_string(&a.value)))
                    .collect();

                state.implicit_spans.push(PendingSpan {
                    span_id: local.span_id,
                    trace_id: state.trace_id.clone(),
                    trace_states: state.trace_states.clone(),
                    parent_span_id: local.parent,
                    links: Vec::new(),
                    start_time_ns: datetime_to_nanos(&local.start),
                    attributes: attrs,
                    kind: None,
                    opening_index: Some(entry_index),
                });
            }
            _ => {}
        }
    }
}

fn handle_span_started(state: &mut WorkerState, params: SpanStarted, opening_index: Option<u64>) {
    let attrs: HashMap<String, String> = params
        .attributes
        .into_iter()
        .map(|a| (a.key, attribute_value_to_string(&a.value)))
        .collect();

    state.pending_spans.insert(
        params.span_id.clone(),
        PendingSpan {
            span_id: params.span_id,
            trace_id: params.trace_id,
            trace_states: params.trace_states,
            parent_span_id: params.parent_span_id,
            links: params
                .links
                .into_iter()
                .map(|link| PendingSpanLink {
                    trace_id: link.trace_id,
                    span_id: link.span_id,
                    trace_states: link.trace_states,
                })
                .collect(),
            start_time_ns: datetime_to_nanos(&params.started_at),
            attributes: attrs,
            kind: Some(span_kind(&params.kind)),
            opening_index,
        },
    );
}

fn handle_span_attributes(state: &mut WorkerState, params: SpanAttributes) {
    if let Some(span) = state.pending_spans.get_mut(&params.span_id) {
        for attribute in params.attributes {
            span.attributes
                .insert(attribute.key, attribute_value_to_string(&attribute.value));
        }
    } else {
        println!(
            "OTLP exporter: attributes for unknown span {}",
            params.span_id
        );
    }
}

fn handle_span_finished(
    state: &mut WorkerState,
    params: SpanFinished,
    completed: &mut Vec<OtlpSpan>,
) {
    if let Some(span) = state.pending_spans.remove(&params.span_id) {
        let end_time_ns = datetime_to_nanos(&params.finished_at);
        let (is_error, message) = span_outcome(&params.outcome);
        completed.push(build_otel_span(span, end_time_ns, is_error, message));
    } else {
        println!("OTLP exporter: close for unknown span {}", params.span_id);
    }
}

fn handle_end(state: &mut WorkerState, params: RawEndParameters, completed: &mut Vec<OtlpSpan>) {
    if let Some(attributes) = params.span_attributes {
        handle_span_attributes(state, attributes);
    }
    if let Some(finished) = params.span_finished {
        handle_span_finished(state, finished, completed);
    }
}

fn handle_cancelled(
    state: &mut WorkerState,
    params: RawCancelledParameters,
    completed: &mut Vec<OtlpSpan>,
) {
    if let Some(finished) = params.span_finished {
        handle_span_finished(state, finished, completed);
    }
}

fn span_kind(kind: &SpanKind) -> u32 {
    match kind {
        SpanKind::Internal => 1,
        SpanKind::Server => 2,
        SpanKind::Client => 3,
    }
}

fn span_outcome(outcome: &SpanOutcome) -> (bool, Option<&'static str>) {
    match outcome {
        SpanOutcome::Completed => (false, None),
        SpanOutcome::Failed => (true, Some("failed")),
        SpanOutcome::Cancelled => (true, Some("cancelled")),
        SpanOutcome::Abandoned => (true, Some("abandoned")),
        SpanOutcome::Denied => (true, Some("denied")),
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

fn flush_implicit_spans(
    state: &mut WorkerState,
    end_time_ns: u128,
    is_error: bool,
    completed: &mut Vec<OtlpSpan>,
) {
    let error_msg = state.terminal_error.clone();
    let spans = std::mem::take(&mut state.implicit_spans);

    for span in spans {
        completed.push(build_otel_span(
            span,
            end_time_ns,
            is_error,
            error_msg.as_ref().map(|(message, _)| message.as_str()),
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

    metrics.push(gauge_metric(
        "golem.resources.active",
        "1",
        "Active resource instances",
        &time_ns,
        state.active_resources as u64,
    ));
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

    metrics.push(gauge_metric(
        "golem.resources.active",
        "1",
        "Active resource instances",
        &time_ns,
        state.active_resources as u64,
    ));
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

#[cfg(test)]
mod tests {
    use super::*;
    use golem_rust::bindings::golem::api::context::{Attribute, AttributeValue};
    use golem_rust::bindings::golem::api::oplog::{
        EnvironmentId, JumpParameters, LocalSpanData, OplogErrorKind, OplogPayload, OplogRegion,
        RawCreateResourceParameters, RawErrorParameters, RawInvocationWalletPin, ResourceTypeId,
        RevertParameters, SpanLink, Timestamp, WalletVersionToken, WorkerError,
        WrappedFunctionType,
    };
    use golem_rust::schema::wit::wire::{AccountId, AgentId, ComponentId, Uuid};
    use golem_rust::wasip3::clocks::system_clock::Instant;

    fn instant(seconds: i64, nanoseconds: u32) -> Instant {
        Instant {
            seconds,
            nanoseconds,
        }
    }

    fn state() -> WorkerState {
        WorkerState {
            trace_id: String::new(),
            trace_states: Vec::new(),
            pending_spans: HashMap::new(),
            implicit_spans: Vec::new(),
            terminal_error: None,
            invocation_start_ns: None,
            total_memory_bytes: 0,
            active_resources: 0,
            resource_identity: None,
        }
    }

    fn attribute(key: &str, value: &str) -> Attribute {
        Attribute {
            key: key.to_string(),
            value: AttributeValue::String(value.to_string()),
        }
    }

    fn opening(span_id: &str) -> SpanStarted {
        SpanStarted {
            span_id: span_id.to_string(),
            trace_id: "trace-a".to_string(),
            trace_states: vec!["vendor=a".to_string()],
            parent_span_id: Some("parent-a".to_string()),
            links: vec![SpanLink {
                trace_id: "trace-b".to_string(),
                span_id: "span-b".to_string(),
                trace_states: vec!["vendor=b".to_string()],
            }],
            started_at: instant(11, 7),
            attributes: vec![attribute("name", "embedded")],
            kind: SpanKind::Client,
        }
    }

    #[test]
    fn logs_use_recorded_context_instead_of_ambient_invocation() {
        let mut state = state();
        state.trace_id = "ambient-trace".to_string();
        let output = process_entries(
            &mut state,
            vec![OplogEntry::Log(LogParameters {
                timestamp: instant(12, 0),
                level: LogLevel::Info,
                context: "test".to_string(),
                message: "correlated".to_string(),
                trace_context: Some(golem_rust::bindings::golem::api::oplog::LogTraceContext {
                    trace_id: "recorded-trace".to_string(),
                    span_id: "recorded-span".to_string(),
                }),
            })],
        );
        assert_eq!(
            output.log_records[0].trace_id.as_deref(),
            Some("recorded-trace")
        );
        assert_eq!(
            output.log_records[0].span_id.as_deref(),
            Some("recorded-span")
        );
    }

    #[test]
    fn stream_summaries_emit_only_bounded_event_and_outcome_labels() {
        let mut state = state();
        let output = process_entries(
            &mut state,
            vec![OplogEntry::StreamSession(
                golem_rust::bindings::golem::api::oplog::RawDurableStreamRecordParameters {
                    timestamp: instant(12, 0),
                    record: OplogPayload::Inline(Vec::new()),
                    summary: Some(DurableStreamEventSummary::SessionFinished(
                        DurableStreamOutcome::Error,
                    )),
                },
            )],
        );
        let metric = output
            .metrics
            .iter()
            .find(|metric| metric.name == "golem.durable_stream.event_count")
            .unwrap();
        let attributes = &metric.sum.as_ref().unwrap().data_points[0].attributes;
        assert_eq!(attributes.len(), 2);
        assert!(attributes.iter().any(|attribute| {
            attribute.key == "event" && attribute.value.string_value == "session-finished"
        }));
        assert!(attributes.iter().any(|attribute| {
            attribute.key == "outcome" && attribute.value.string_value == "error"
        }));
    }

    fn close(span_id: &str, outcome: SpanOutcome) -> SpanFinished {
        SpanFinished {
            span_id: span_id.to_string(),
            finished_at: instant(29, 13),
            outcome,
        }
    }

    fn start(function_name: &str, span: Option<SpanStarted>) -> OplogEntry {
        OplogEntry::Start(RawStartParameters {
            timestamp: instant(11, 0),
            parent_start_index: None,
            function_name: function_name.to_string(),
            invocation_id: None,
            observational_owner: None,
            request: Some(OplogPayload::Inline(Vec::new())),
            durable_function_type: WrappedFunctionType::ReadLocal,
            span_started: span,
        })
    }

    fn create(instance: u64, initial_memory: u64) -> OplogEntry {
        let uuid = |value| Uuid {
            high_bits: 0,
            low_bits: value,
        };
        OplogEntry::Create(RawCreateParameters {
            timestamp: instant(10, 0),
            agent_id: AgentId {
                component_id: ComponentId { uuid: uuid(1) },
                agent_id: "agent".to_string(),
            },
            owner_kind: OwnerKind::ComponentAgent,
            agent_mode: AgentMode::Durable,
            component_revision: 1,
            env: Vec::new(),
            environment_id: EnvironmentId { uuid: uuid(2) },
            created_by: AccountId { uuid: uuid(3) },
            parent: None,
            component_size: 100,
            initial_total_linear_memory_size: initial_memory,
            initial_active_plugins: Vec::new(),
            local_agent_config: Vec::new(),
            original_phantom_id: None,
            instance_id: uuid(instance),
        })
    }

    fn local_span(span_id: &str, parent: Option<&str>, inherited: bool) -> SpanData {
        SpanData::LocalSpan(LocalSpanData {
            span_id: span_id.to_string(),
            start: instant(12, 0),
            parent: parent.map(str::to_string),
            linked_context: None,
            attributes: Vec::new(),
            inherited,
        })
    }

    fn invocation_started(trace_id: &str, context: Vec<SpanData>) -> OplogEntry {
        invocation_started_with_key("invocation", trace_id, context)
    }

    fn invocation_started_with_key(
        idempotency_key: &str,
        trace_id: &str,
        context: Vec<SpanData>,
    ) -> OplogEntry {
        OplogEntry::AgentInvocationStarted(RawAgentInvocationStartedParameters {
            timestamp: instant(12, 0),
            idempotency_key: idempotency_key.to_string(),
            payload: OplogPayload::Inline(Vec::new()),
            trace_id: trace_id.to_string(),
            trace_states: Vec::new(),
            invocation_context: context,
            wallet_pin: RawInvocationWalletPin {
                wallet_token: WalletVersionToken {
                    wallet_id_hash: Vec::new(),
                    generation: 0,
                },
                pinned_card_ids: Vec::new(),
                scope_card_id: None,
            },
        })
    }

    fn invocation_finished() -> OplogEntry {
        OplogEntry::AgentInvocationFinished(RawAgentInvocationFinishedParameters {
            timestamp: instant(20, 0),
            result: OplogPayload::Inline(Vec::new()),
            method_name: None,
            consumed_fuel: 0,
            component_revision: 0,
        })
    }

    fn revert_original_start() -> OplogEntry {
        OplogEntry::Revert(RevertParameters {
            timestamp: instant(19, 0),
            dropped_region: OplogRegion { start: 1, end: 1 },
        })
    }

    #[test]
    fn split_batches_preserve_origin_context_links_delta_and_explicit_times() {
        let mut state = state();
        handle_span_started(&mut state, opening("span-a"), None);

        state.trace_id = "trace-current-b".to_string();
        handle_span_attributes(
            &mut state,
            SpanAttributes {
                span_id: "span-a".to_string(),
                attributes: vec![attribute("result", "updated")],
            },
        );

        let mut completed = Vec::new();
        handle_span_finished(
            &mut state,
            close("span-a", SpanOutcome::Completed),
            &mut completed,
        );
        let span = &completed[0];
        assert_eq!(span.trace_id, "trace-a");
        assert_eq!(span.trace_state.as_deref(), Some("vendor=a"));
        assert_eq!(span.start_time_unix_nano, "11000000007");
        assert_eq!(span.end_time_unix_nano, "29000000013");
        assert_eq!(span.kind, 3);
        assert_eq!(span.links[0].trace_id, "trace-b");
        assert_eq!(span.links[0].trace_state.as_deref(), Some("vendor=b"));
        assert!(
            span.attributes
                .iter()
                .any(|attribute| attribute.key == "result")
        );
    }

    #[test]
    fn end_applies_attributes_before_close_and_repeat_close_is_ignored() {
        let mut state = state();
        handle_span_started(&mut state, opening("span"), None);
        let mut completed = Vec::new();
        handle_end(
            &mut state,
            RawEndParameters {
                timestamp: instant(100, 0),
                start_index: 1,
                response: None,
                forced_commit: false,
                span_finished: Some(close("span", SpanOutcome::Completed)),
                span_attributes: Some(SpanAttributes {
                    span_id: "span".to_string(),
                    attributes: vec![attribute("final", "yes")],
                }),
            },
            &mut completed,
        );
        handle_span_finished(
            &mut state,
            close("span", SpanOutcome::Failed),
            &mut completed,
        );
        assert_eq!(completed.len(), 1);
        assert!(
            completed[0]
                .attributes
                .iter()
                .any(|attribute| attribute.key == "final")
        );
    }

    #[test]
    fn terminal_outcomes_are_distinct() {
        for (outcome, message) in [
            (SpanOutcome::Failed, "failed"),
            (SpanOutcome::Cancelled, "cancelled"),
            (SpanOutcome::Abandoned, "abandoned"),
            (SpanOutcome::Denied, "denied"),
        ] {
            let mut state = state();
            handle_span_started(&mut state, opening(message), None);
            let mut completed = Vec::new();
            handle_span_finished(&mut state, close(message, outcome), &mut completed);
            let status = completed[0].status.as_ref().unwrap();
            assert_eq!(status.code, 2);
            assert_eq!(status.message.as_deref(), Some(message));
        }
    }

    #[test]
    fn retry_hints_do_not_close_embedded_or_implicit_spans() {
        let mut state = state();
        handle_span_started(&mut state, opening("embedded"), None);
        state.implicit_spans.push(PendingSpan {
            span_id: "implicit".to_string(),
            trace_id: "trace-a".to_string(),
            trace_states: Vec::new(),
            parent_span_id: None,
            links: Vec::new(),
            start_time_ns: 1,
            attributes: HashMap::new(),
            kind: None,
            opening_index: None,
        });
        state.terminal_error = Some(("interrupted".to_string(), 9));

        assert_eq!(state.pending_spans.len(), 1);
        assert_eq!(state.implicit_spans.len(), 1);
        let mut completed = Vec::new();
        flush_implicit_spans(&mut state, 10, false, &mut completed);
        assert_eq!(completed.len(), 1);
        assert!(completed[0].status.is_none());
        assert_eq!(state.pending_spans.len(), 1);
    }

    #[test]
    fn metric_state_prevents_worker_state_eviction_without_spans() {
        let mut state = state();
        state.total_memory_bytes = 64;
        assert!(!state.is_empty());
        state.total_memory_bytes = 0;
        state.active_resources = 1;
        assert!(!state.is_empty());
    }

    #[test]
    fn active_resource_count_is_an_absolute_gauge() {
        let mut state = state();
        let output = process_entries(
            &mut state,
            vec![OplogEntry::CreateResource(RawCreateResourceParameters {
                timestamp: instant(12, 0),
                id: 1,
                resource_type_id: ResourceTypeId {
                    name: "connection".to_string(),
                    owner: "test".to_string(),
                },
            })],
        );

        let active = output
            .metrics
            .iter()
            .find(|metric| metric.name == "golem.resources.active")
            .unwrap();
        assert!(active.sum.is_none());
        assert_eq!(
            active.gauge.as_ref().unwrap().data_points[0]
                .as_int
                .as_deref(),
            Some("1")
        );

        let output = process_entries_from(
            &mut state,
            2,
            vec![OplogEntry::DropResource(RawDropResourceParameters {
                timestamp: instant(13, 0),
                id: 1,
                resource_type_id: ResourceTypeId {
                    name: "connection".to_string(),
                    owner: "test".to_string(),
                },
            })],
        );
        let active = output
            .metrics
            .iter()
            .find(|metric| metric.name == "golem.resources.active")
            .unwrap();
        assert!(active.sum.is_none());
        assert_eq!(
            active.gauge.as_ref().unwrap().data_points[0]
                .as_int
                .as_deref(),
            Some("0")
        );
    }

    #[test]
    fn replacement_create_cannot_inherit_previous_incarnation_state() {
        let mut state = state();
        process_entries(&mut state, vec![create(10, 64)]);
        handle_span_started(&mut state, opening("old"), Some(5));
        state.active_resources = 3;
        state.implicit_spans.push(PendingSpan {
            span_id: "old-implicit".to_string(),
            trace_id: "old-trace".to_string(),
            trace_states: Vec::new(),
            parent_span_id: None,
            links: Vec::new(),
            start_time_ns: 1,
            attributes: HashMap::new(),
            kind: None,
            opening_index: Some(6),
        });

        process_entries(&mut state, vec![create(20, 32)]);

        assert!(state.pending_spans.is_empty());
        assert!(state.implicit_spans.is_empty());
        assert_eq!(state.active_resources, 0);
        assert_eq!(state.total_memory_bytes, 32);
        assert_eq!(
            state.resource_identity.as_ref().unwrap().instance_id,
            (0, 20)
        );
    }

    #[test]
    fn internal_span_management_operations_are_not_user_host_calls() {
        let internal = [
            "golem::api::context::start-span",
            "golem::api::context::span::finish",
            "golem::api::context::span::drop",
            "golem::api::context::span::set-attributes",
            "golem::rpc::wasm-rpc::drop",
            "http::client::span-cleanup",
        ];
        for name in internal {
            assert!(is_internal_span_operation(name));
        }

        let mut state = state();
        let output = process_entries(
            &mut state,
            internal
                .into_iter()
                .chain(["golem::rpc::wasm-rpc::invoke"])
                .map(|name| start(name, None))
                .collect(),
        );
        let host_calls = output
            .metrics
            .iter()
            .filter(|metric| metric.name == "golem.host_call.count")
            .count();
        assert_eq!(host_calls, 1);
    }

    #[test]
    fn split_batch_start_and_end_export_attributes_and_finish() {
        let mut state = state();
        let first = process_entries_from(
            &mut state,
            70,
            vec![start("user-call", Some(opening("split")))],
        );
        assert!(first.spans.is_empty());
        assert_eq!(state.pending_spans["split"].opening_index, Some(70));

        let second = process_entries_from(
            &mut state,
            71,
            vec![OplogEntry::End(RawEndParameters {
                timestamp: instant(30, 0),
                start_index: 70,
                response: Some(OplogPayload::Inline(Vec::new())),
                forced_commit: false,
                span_finished: Some(close("split", SpanOutcome::Completed)),
                span_attributes: Some(SpanAttributes {
                    span_id: "split".to_string(),
                    attributes: vec![attribute("finish", "yes")],
                }),
            })],
        );
        assert_eq!(second.spans.len(), 1);
        assert!(second.spans[0].status.is_none());
        assert!(
            second.spans[0]
                .attributes
                .iter()
                .any(|attribute| attribute.key == "finish")
        );
    }

    #[test]
    fn split_batch_cancelled_exports_cancelled_span() {
        let mut state = state();
        process_entries_from(
            &mut state,
            80,
            vec![start("user-call", Some(opening("cancelled")))],
        );
        let output = process_entries_from(
            &mut state,
            81,
            vec![OplogEntry::Cancelled(RawCancelledParameters {
                timestamp: instant(30, 0),
                start_index: 80,
                partial: None,
                span_finished: Some(close("cancelled", SpanOutcome::Cancelled)),
            })],
        );
        assert_eq!(output.spans.len(), 1);
        assert_eq!(
            output.spans[0].status.as_ref().unwrap().message.as_deref(),
            Some("cancelled")
        );
    }

    #[test]
    fn jump_discards_openings_at_both_region_edges_and_keeps_outside_opening() {
        let mut state = state();
        for (span_id, opening_index) in [("implicit-outside", 39), ("implicit-inside", 40)] {
            state.implicit_spans.push(PendingSpan {
                span_id: span_id.to_string(),
                trace_id: "trace-a".to_string(),
                trace_states: Vec::new(),
                parent_span_id: None,
                links: Vec::new(),
                start_time_ns: 1,
                attributes: HashMap::new(),
                kind: None,
                opening_index: Some(opening_index),
            });
        }
        let output = process_entries_from(
            &mut state,
            39,
            vec![
                start("internal", Some(opening("outside"))),
                start("internal", Some(opening("left-edge"))),
                start("internal", Some(opening("right-edge"))),
                OplogEntry::Jump(JumpParameters {
                    timestamp: instant(12, 0),
                    jump: OplogRegion { start: 40, end: 41 },
                }),
            ],
        );
        assert!(output.spans.is_empty());
        assert_eq!(state.pending_spans.len(), 1);
        assert!(state.pending_spans.contains_key("outside"));
        assert_eq!(state.implicit_spans.len(), 1);
        assert_eq!(state.implicit_spans[0].span_id, "implicit-outside");
    }

    #[test]
    fn revert_removes_openings_from_the_dropped_region_without_exporting_them() {
        let mut state = state();
        let output = process_entries_from(
            &mut state,
            40,
            vec![
                OplogEntry::Start(RawStartParameters {
                    timestamp: instant(11, 0),
                    parent_start_index: None,
                    function_name: "golem::api::context::start-span".to_string(),
                    invocation_id: None,
                    observational_owner: None,
                    request: Some(OplogPayload::Inline(Vec::new())),
                    durable_function_type: WrappedFunctionType::ReadLocal,
                    span_started: Some(opening("reverted")),
                }),
                OplogEntry::Revert(RevertParameters {
                    timestamp: instant(12, 0),
                    dropped_region: OplogRegion { start: 40, end: 40 },
                }),
            ],
        );

        assert!(output.spans.is_empty());
        assert!(state.pending_spans.is_empty());
        assert!(
            output
                .metrics
                .iter()
                .all(|metric| metric.name != "golem.host_call.count")
        );
    }

    #[test]
    fn inherited_caller_chain_keeps_immediate_parent_for_local_callee_span() {
        let mut state = state();
        let output = process_entries(
            &mut state,
            vec![
                invocation_started(
                    "trace-b",
                    vec![
                        local_span("caller-root", None, true),
                        local_span("caller-rpc", Some("caller-root"), true),
                        local_span("callee-invocation", Some("caller-rpc"), false),
                    ],
                ),
                invocation_finished(),
            ],
        );

        assert_eq!(output.spans.len(), 1);
        assert_eq!(output.spans[0].span_id, "callee-invocation");
        assert_eq!(
            output.spans[0].parent_span_id.as_deref(),
            Some("caller-rpc")
        );
    }

    #[test]
    fn explicit_origin_survives_failed_implicit_invocation_and_closes_once() {
        let mut state = state();
        let output = process_entries(
            &mut state,
            vec![
                OplogEntry::Start(RawStartParameters {
                    timestamp: instant(11, 0),
                    parent_start_index: None,
                    function_name: "rpc".to_string(),
                    invocation_id: None,
                    observational_owner: None,
                    request: None,
                    durable_function_type: WrappedFunctionType::ReadLocal,
                    span_started: Some(opening("explicit-a")),
                }),
                invocation_started(
                    "trace-b",
                    vec![local_span("implicit-b", Some("caller-rpc"), false)],
                ),
                OplogEntry::Interrupted(Timestamp {
                    timestamp: instant(18, 0),
                }),
                invocation_finished(),
                OplogEntry::End(RawEndParameters {
                    timestamp: instant(30, 0),
                    start_index: 0,
                    response: None,
                    forced_commit: false,
                    span_finished: Some(close("explicit-a", SpanOutcome::Completed)),
                    span_attributes: None,
                }),
            ],
        );

        assert_eq!(output.spans.len(), 2);
        let explicit = output
            .spans
            .iter()
            .find(|span| span.span_id == "explicit-a")
            .unwrap();
        assert_eq!(explicit.trace_id, "trace-a");
        assert_eq!(
            output
                .spans
                .iter()
                .filter(|span| span.span_id == "implicit-b")
                .count(),
            1
        );
        assert!(state.pending_spans.is_empty());
        assert!(state.implicit_spans.is_empty());
    }

    #[test]
    fn next_invocation_start_closes_failed_implicit_span_after_revert() {
        for next_key in ["failed-key", "next-key"] {
            let mut state = state();
            handle_span_started(&mut state, opening("explicit"), None);
            let failed = process_entries(
                &mut state,
                vec![
                    invocation_started_with_key(
                        "failed-key",
                        "trace-failed",
                        vec![local_span("failed-invocation", None, false)],
                    ),
                    OplogEntry::Error(RawErrorParameters {
                        timestamp: instant(18, 0),
                        kind: OplogErrorKind::Invocation,
                        error: WorkerError::Unknown("boom".to_string()),
                        retry_from: 0,
                        inside_atomic_region: false,
                        retry_policy_state: None,
                    }),
                ],
            );

            assert!(failed.spans.is_empty());
            assert_eq!(state.implicit_spans.len(), 1);
            assert_eq!(state.pending_spans.len(), 1);

            let output = process_entries(
                &mut state,
                vec![
                    revert_original_start(),
                    invocation_started_with_key(
                        next_key,
                        "trace-retry",
                        vec![local_span("retry-invocation", None, false)],
                    ),
                    invocation_finished(),
                    OplogEntry::End(RawEndParameters {
                        timestamp: instant(30, 0),
                        start_index: 0,
                        response: None,
                        forced_commit: false,
                        span_finished: Some(close("explicit", SpanOutcome::Completed)),
                        span_attributes: None,
                    }),
                ],
            );

            let failed = output
                .spans
                .iter()
                .find(|span| span.span_id == "failed-invocation")
                .unwrap();
            assert_eq!(failed.trace_id, "trace-failed");
            assert_eq!(failed.end_time_unix_nano, "18000000000");
            assert_eq!(failed.status.as_ref().unwrap().code, 2);

            let retry = output
                .spans
                .iter()
                .find(|span| span.span_id == "retry-invocation")
                .unwrap();
            assert_eq!(retry.trace_id, "trace-retry");
            assert!(retry.status.is_none());

            let explicit = output
                .spans
                .iter()
                .find(|span| span.span_id == "explicit")
                .unwrap();
            assert_eq!(explicit.trace_id, "trace-a");
            assert!(explicit.status.is_none());
            assert!(state.pending_spans.is_empty());
            assert!(state.implicit_spans.is_empty());
        }
    }

    #[test]
    fn reverted_invocation_without_error_closes_at_next_start_time() {
        let mut state = state();
        let output = process_entries(
            &mut state,
            vec![
                invocation_started(
                    "trace-reverted",
                    vec![local_span("reverted-invocation", None, false)],
                ),
                revert_original_start(),
                invocation_started(
                    "trace-next",
                    vec![local_span("next-invocation", None, false)],
                ),
            ],
        );

        assert_eq!(output.spans.len(), 1);
        assert_eq!(output.spans[0].span_id, "reverted-invocation");
        assert_eq!(output.spans[0].trace_id, "trace-reverted");
        assert_eq!(output.spans[0].end_time_unix_nano, "12000000000");
        assert_eq!(
            output.spans[0].status.as_ref().unwrap().message.as_deref(),
            Some("invocation superseded before completion")
        );
        assert_eq!(state.implicit_spans[0].span_id, "next-invocation");
    }
}
