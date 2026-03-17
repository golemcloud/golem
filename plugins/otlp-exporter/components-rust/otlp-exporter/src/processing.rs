use crate::export::build_otel_span;
use crate::helpers::{attribute_value_to_string, datetime_to_nanos, timestamp_to_nanos, worker_error_to_string};
use crate::otlp_json::OtlpSpan;
use crate::state::{PendingSpan, WorkerState};
use golem_rust::bindings::golem::api::oplog::{
    FinishSpanParameters, OplogEntry, RawAgentInvocationFinishedParameters,
    RawAgentInvocationStartedParameters, SetSpanAttributeParameters, SpanData, StartSpanParameters,
};
use std::collections::HashMap;

pub(crate) fn process_entries(
    state: &mut WorkerState,
    entries: Vec<OplogEntry>,
    completed_spans: &mut Vec<OtlpSpan>,
) {
    for entry in entries {
        match entry {
            OplogEntry::AgentInvocationStarted(params) => {
                handle_invocation_started(state, params);
            }
            OplogEntry::StartSpan(params) => {
                handle_start_span(state, params);
            }
            OplogEntry::SetSpanAttribute(params) => {
                handle_set_span_attribute(state, params);
            }
            OplogEntry::FinishSpan(params) => {
                handle_finish_span(state, params, completed_spans);
            }
            OplogEntry::AgentInvocationFinished(params) => {
                handle_invocation_finished(state, params, completed_spans);
            }
            OplogEntry::Error(params) => {
                state.terminal_error = Some(worker_error_to_string(&params.error));
                handle_terminal(
                    state,
                    datetime_to_nanos(&params.timestamp),
                    true,
                    completed_spans,
                );
            }
            OplogEntry::Interrupted(ts) => {
                state.terminal_error = Some("interrupted".to_string());
                handle_terminal(state, timestamp_to_nanos(&ts), true, completed_spans);
            }
            OplogEntry::Exited(ts) => {
                state.terminal_error = Some("exited".to_string());
                handle_terminal(state, timestamp_to_nanos(&ts), true, completed_spans);
            }
            _ => {} // ignore all other entry types
        }
    }
}

fn handle_invocation_started(
    state: &mut WorkerState,
    params: RawAgentInvocationStartedParameters,
) {
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

    state.trace_id = params.trace_id;
    state.trace_states = params.trace_states;

    for span_data in params.invocation_context {
        match span_data {
            SpanData::LocalSpan(local) => {
                if local.inherited {
                    continue;
                }
                let attrs: HashMap<String, String> = local
                    .attributes
                    .into_iter()
                    .map(|a| (a.key, attribute_value_to_string(&a.value)))
                    .collect();

                state.implicit_spans.push(PendingSpan {
                    span_id: local.span_id,
                    parent_span_id: local.parent,
                    start_time_ns: datetime_to_nanos(&local.start),
                    attributes: attrs,
                });
            }
            SpanData::ExternalSpan(_) => {
                continue;
            }
        }
    }
}

fn handle_start_span(state: &mut WorkerState, params: StartSpanParameters) {
    let attrs: HashMap<String, String> = params
        .attributes
        .into_iter()
        .map(|a| (a.key, attribute_value_to_string(&a.value)))
        .collect();

    state.pending_spans.insert(
        params.span_id.clone(),
        PendingSpan {
            span_id: params.span_id,
            parent_span_id: params.parent,
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
) {
    let end_time_ns = datetime_to_nanos(&params.timestamp);
    flush_implicit_spans(state, end_time_ns, false, completed);
    flush_remaining_explicit_spans(state, end_time_ns, false, completed);
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

    if !spans.is_empty() && !is_error {
        println!(
            "OTLP exporter: {} explicit spans still open at invocation end, force-closing",
            spans.len()
        );
    }

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
