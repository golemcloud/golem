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
use processing::process_entries;
use state::WORKER_STATES;

use golem_rust::bindings::golem::api::oplog::{OplogEntry, OplogIndex};
use golem_rust::golem_wasm::golem_core_1_5_x::types::{AgentId, ComponentId};
use golem_rust::oplog_processor::exports::golem::api::oplog_processor::Guest as OplogProcessorGuest;

use std::collections::HashMap;

struct OtlpExporterComponent;

impl OplogProcessorGuest for OtlpExporterComponent {
    fn process(
        _account_info: golem_rust::oplog_processor::exports::golem::api::oplog_processor::AccountInfo,
        config: Vec<(String, String)>,
        component_id: ComponentId,
        worker_id: AgentId,
        metadata: golem_rust::bindings::golem::api::host::AgentMetadata,
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
                return Err(format!(
                    "OTLP exporter: configuration error: {e}"
                ));
            }
        };

        let key = worker_key(&component_id, &worker_id);

        // Clone current state for processing — do NOT mutate the stored state yet
        let mut working_state = WORKER_STATES.with(|states| {
            let states = states.borrow();
            states.get(&key).cloned().unwrap_or_else(|| state::WorkerState {
                trace_id: String::new(),
                trace_states: Vec::new(),
                pending_spans: HashMap::new(),
                implicit_spans: Vec::new(),
                terminal_error: None,
                inherited_span_parents: HashMap::new(),
                invocation_start_ns: None,
                total_memory_bytes: 0,
                active_resources: 0,
            })
        });

        let output = process_entries(&mut working_state, entries);

        let has_traces = exporter_config.signals.traces && !output.spans.is_empty();
        let has_logs = exporter_config.signals.logs && !output.log_records.is_empty();
        let has_metrics = exporter_config.signals.metrics && !output.metrics.is_empty();

        if !has_traces && !has_logs && !has_metrics {
            WORKER_STATES.with(|states| {
                let mut states = states.borrow_mut();
                if working_state.is_empty() {
                    states.remove(&key);
                } else {
                    states.insert(key, working_state);
                }
            });
            return Ok(());
        }

        let resource_attrs =
            build_resource_attributes(&exporter_config, &component_id, &worker_id, &metadata);

        let scope = InstrumentationScope {
            name: "golem-otlp-exporter".to_string(),
            version: "1.5.0".to_string(),
        };

        if has_traces {
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
            send_spans(&exporter_config, request_body)?;
        }

        if has_logs {
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
            send_logs(&exporter_config, request_body)?;
        }

        if has_metrics {
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
            send_metrics(&exporter_config, request_body)?;
        }

        WORKER_STATES.with(|states| {
            let mut states = states.borrow_mut();
            if working_state.is_empty() {
                states.remove(&key);
            } else {
                states.insert(key, working_state);
            }
        });

        Ok(())
    }
}

golem_rust::oplog_processor::export_oplog_processor!(OtlpExporterComponent with_types_in golem_rust::oplog_processor);
