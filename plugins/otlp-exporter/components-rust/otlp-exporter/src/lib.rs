mod config;
mod export;
mod helpers;
mod otlp_json;
mod processing;
mod state;

use config::ExporterConfig;
use export::{build_resource_attributes, send_logs, send_metrics, send_spans};
use helpers::worker_key;
use otlp_json::{
    ExportLogsServiceRequest, ExportMetricsServiceRequest, ExportTraceServiceRequest,
    InstrumentationScope, OtlpResource, ResourceLogs, ResourceMetrics, ResourceSpans, ScopeLogs,
    ScopeMetrics, ScopeSpans,
};
use state::WORKER_STATES;

use golem_rust::bindings::golem::api::oplog::{OplogEntry, OplogIndex};
use golem_rust::oplog_processor::exports::golem::api::oplog_processor::Guest as OplogProcessorGuest;
use golem_rust::schema::wit::wire::{AgentId, ComponentId};

use std::collections::HashMap;
use std::future::Future;

async fn export_processed_batch(
    key: String,
    working_state: state::WorkerState,
    traces: Option<impl Future<Output = Result<(), String>>>,
    logs: Option<impl Future<Output = Result<(), String>>>,
    metrics: Option<impl Future<Output = Result<(), String>>>,
) -> Result<(), String> {
    // Source batches are accepted independently of best-effort collector delivery.
    WORKER_STATES.with(|states| {
        let mut states = states.borrow_mut();
        if working_state.is_empty() {
            states.remove(&key);
        } else {
            states.insert(key, working_state);
        }
    });
    let mut failures = Vec::new();

    if let Some(traces) = traces
        && let Err(error) = traces.await
    {
        failures.push(format!("traces: {error}"));
    }
    if let Some(logs) = logs
        && let Err(error) = logs.await
    {
        failures.push(format!("logs: {error}"));
    }
    if let Some(metrics) = metrics
        && let Err(error) = metrics.await
    {
        failures.push(format!("metrics: {error}"));
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!("OTLP export failed: {}", failures.join("; ")))
    }
}

struct OtlpExporterComponent;

impl OplogProcessorGuest for OtlpExporterComponent {
    async fn process(
        _account_info: golem_rust::oplog_processor::exports::golem::api::oplog_processor::AccountInfo,
        config: Vec<(String, String)>,
        component_id: ComponentId,
        worker_id: AgentId,
        metadata: golem_rust::oplog_processor::host::AgentMetadata,
        _first_entry_index: OplogIndex,
        entries: Vec<OplogEntry>,
    ) -> Result<(), String> {
        if entries.is_empty() {
            return Ok(());
        }

        let exporter_config = match ExporterConfig::from_params(&config) {
            Ok(Some(c)) => c,
            Ok(None) => return Ok(()),
            Err(e) => {
                return Err(format!("OTLP exporter: configuration error: {e}"));
            }
        };

        let key = worker_key(&component_id, &worker_id);

        // Clone current state for processing — do NOT mutate the stored state yet
        let mut working_state = WORKER_STATES.with(|states| {
            let states = states.borrow();
            states
                .get(&key)
                .cloned()
                .unwrap_or_else(|| state::WorkerState {
                    trace_id: String::new(),
                    trace_states: Vec::new(),
                    pending_spans: HashMap::new(),
                    implicit_spans: Vec::new(),
                    terminal_error: None,
                    invocation_start_ns: None,
                    total_memory_bytes: 0,
                    active_resources: 0,
                    resource_identity: None,
                })
        });

        let output =
            processing::process_entries_from(&mut working_state, _first_entry_index, entries);

        let has_traces = exporter_config.signals.traces && !output.spans.is_empty();
        let has_logs = exporter_config.signals.logs && !output.log_records.is_empty();
        let has_metrics = exporter_config.signals.metrics && !output.metrics.is_empty();

        let resource_attrs = build_resource_attributes(
            &exporter_config,
            &component_id,
            &worker_id,
            &metadata,
            working_state.resource_identity.as_ref(),
        );

        let scope = InstrumentationScope {
            name: "golem-otlp-exporter".to_string(),
            version: "1.5.3".to_string(),
        };

        let traces = if has_traces {
            let span_count = output.spans.len();
            let request_body = ExportTraceServiceRequest {
                resource_spans: vec![ResourceSpans {
                    resource: OtlpResource {
                        attributes: resource_attrs.clone(),
                    },
                    scope_spans: vec![ScopeSpans {
                        scope: scope.clone(),
                        spans: output.spans,
                    }],
                }],
            };
            Some((request_body, span_count))
        } else {
            None
        };

        let logs = if has_logs {
            let log_count = output.log_records.len();
            let request_body = ExportLogsServiceRequest {
                resource_logs: vec![ResourceLogs {
                    resource: OtlpResource {
                        attributes: resource_attrs.clone(),
                    },
                    scope_logs: vec![ScopeLogs {
                        scope: scope.clone(),
                        log_records: output.log_records,
                    }],
                }],
            };
            Some((request_body, log_count))
        } else {
            None
        };

        let metrics = if has_metrics {
            let metric_count = output.metrics.len();
            let request_body = ExportMetricsServiceRequest {
                resource_metrics: vec![ResourceMetrics {
                    resource: OtlpResource {
                        attributes: resource_attrs,
                    },
                    scope_metrics: vec![ScopeMetrics {
                        scope,
                        metrics: output.metrics,
                    }],
                }],
            };
            Some((request_body, metric_count))
        } else {
            None
        };

        let trace_config = &exporter_config;
        let log_config = &exporter_config;
        let metric_config = &exporter_config;

        export_processed_batch(
            key,
            working_state,
            traces.map(|(request, count)| async move {
                send_spans(trace_config, request).await?;
                println!("OTLP: exported {count} trace span(s)");
                Ok(())
            }),
            logs.map(|(request, count)| async move {
                send_logs(log_config, request).await?;
                println!("OTLP: exported {count} log record(s)");
                Ok(())
            }),
            metrics.map(|(request, count)| async move {
                send_metrics(metric_config, request).await?;
                println!("OTLP: exported {count} metric(s)");
                Ok(())
            }),
        )
        .await
    }
}

golem_rust::oplog_processor::export_oplog_processor!(OtlpExporterComponent with_types_in golem_rust::oplog_processor);

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    async fn record_send(
        attempts: Rc<RefCell<Vec<&'static str>>>,
        signal: &'static str,
        failure: Option<&'static str>,
    ) -> Result<(), String> {
        attempts.borrow_mut().push(signal);
        match failure {
            Some(error) => Err(error.to_string()),
            None => Ok(()),
        }
    }

    fn worker_state(active_resources: i64) -> state::WorkerState {
        let mut pending_spans = HashMap::new();
        pending_spans.insert(
            "span".to_string(),
            state::PendingSpan {
                span_id: "span".to_string(),
                trace_id: "trace".to_string(),
                trace_states: Vec::new(),
                parent_span_id: None,
                links: Vec::new(),
                start_time_ns: 1,
                attributes: HashMap::new(),
                kind: None,
                opening_index: None,
            },
        );
        state::WorkerState {
            trace_id: "trace".to_string(),
            trace_states: Vec::new(),
            pending_spans,
            implicit_spans: Vec::new(),
            terminal_error: None,
            invocation_start_ns: None,
            total_memory_bytes: 0,
            active_resources,
            resource_identity: None,
        }
    }

    #[test]
    fn first_signal_failure_does_not_prevent_remaining_signals() {
        let attempts = Rc::new(RefCell::new(Vec::new()));
        let result = futures::executor::block_on(export_processed_batch(
            "first-signal-failure".to_string(),
            worker_state(1),
            Some(record_send(
                attempts.clone(),
                "traces",
                Some("collector unavailable"),
            )),
            Some(record_send(attempts.clone(), "logs", None)),
            Some(record_send(attempts.clone(), "metrics", Some("timed out"))),
        ));

        assert_eq!(*attempts.borrow(), ["traces", "logs", "metrics"]);
        assert_eq!(
            result,
            Err(
                "OTLP export failed: traces: collector unavailable; metrics: timed out".to_string()
            )
        );
    }

    #[test]
    fn middle_signal_failure_does_not_prevent_last_signal() {
        let attempts = Rc::new(RefCell::new(Vec::new()));
        let result = futures::executor::block_on(export_processed_batch(
            "middle-signal-failure".to_string(),
            worker_state(1),
            Some(record_send(attempts.clone(), "traces", None)),
            Some(record_send(attempts.clone(), "logs", Some("rejected"))),
            Some(record_send(attempts.clone(), "metrics", None)),
        ));

        assert_eq!(*attempts.borrow(), ["traces", "logs", "metrics"]);
        assert_eq!(
            result,
            Err("OTLP export failed: logs: rejected".to_string())
        );
    }

    #[test]
    fn failed_export_keeps_accepted_state_for_the_next_batch() {
        const KEY: &str = "collector-failure-continuity";
        WORKER_STATES.with(|states| states.borrow_mut().remove(KEY));

        let result = futures::executor::block_on(export_processed_batch(
            KEY.to_string(),
            worker_state(1),
            Some(async {
                WORKER_STATES.with(|states| {
                    assert_eq!(states.borrow()[KEY].active_resources, 1);
                });
                Err("down".to_string())
            }),
            None::<std::future::Ready<Result<(), String>>>,
            None::<std::future::Ready<Result<(), String>>>,
        ));
        assert!(result.is_err());

        let mut next_state = WORKER_STATES.with(|states| states.borrow()[KEY].clone());
        next_state.active_resources += 1;
        futures::executor::block_on(export_processed_batch(
            KEY.to_string(),
            next_state,
            None::<std::future::Ready<Result<(), String>>>,
            None::<std::future::Ready<Result<(), String>>>,
            None::<std::future::Ready<Result<(), String>>>,
        ))
        .unwrap();
        WORKER_STATES.with(|states| {
            assert_eq!(states.borrow()[KEY].active_resources, 2);
            states.borrow_mut().remove(KEY);
        });
    }
}
