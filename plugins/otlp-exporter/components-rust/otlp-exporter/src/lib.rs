mod config;
mod export;
mod helpers;
mod otlp_json;
mod processing;
mod state;

use config::ExporterConfig;
use export::{build_resource_attributes, send_spans};
use helpers::worker_key;
use otlp_json::{
    ExportTraceServiceRequest, InstrumentationScope, OtlpResource, ResourceSpans, ScopeSpans,
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
            })
        });

        let mut completed_spans: Vec<otlp_json::OtlpSpan> = Vec::new();
        process_entries(&mut working_state, entries, &mut completed_spans);

        if completed_spans.is_empty() {
            // No spans to send — commit state update (safe, nothing to retry)
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

        let request_body = ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                resource: OtlpResource {
                    attributes: resource_attrs,
                },
                scope_spans: vec![ScopeSpans {
                    scope: InstrumentationScope {
                        name: "golem-otlp-exporter".to_string(),
                        version: "1.5.0".to_string(),
                    },
                    spans: completed_spans,
                }],
            }],
        };

        // Only commit state after successful export
        send_spans(&exporter_config, request_body)?;

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
