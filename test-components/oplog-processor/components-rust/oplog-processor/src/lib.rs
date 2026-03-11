use golem_rust::bindings::golem::api::oplog::{
    enrich_oplog_entries, AgentInvocation, AgentInvocationStartedParameters, OplogEntry,
    OplogIndex, PublicOplogEntry,
};
use golem_rust::oplog_processor::exports::golem::api::oplog_processor::Guest as OplogProcessorGuest;
use golem_rust::golem_wasm::golem_core_1_5_x::types::{AgentId, ComponentId};
use uuid::Uuid;
use wstd::http::{Body, Client, Request};
use wstd::runtime::block_on;

use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    static CURRENT_INVOCATIONS: RefCell<HashMap<String, AgentInvocationStartedParameters>> =
        RefCell::new(HashMap::new());
}

struct OplogProcessorComponent;

impl OplogProcessorGuest for OplogProcessorComponent {
    fn process(
        account_info: golem_rust::oplog_processor::exports::golem::api::oplog_processor::AccountInfo,
        config: Vec<(String, String)>,
        component_id: ComponentId,
        worker_id: AgentId,
        metadata: golem_rust::bindings::golem::api::host::AgentMetadata,
        first_entry_index: OplogIndex,
        entries: Vec<OplogEntry>,
    ) -> Result<(), String> {
        let callback_url = config
            .iter()
            .find(|(k, _)| k == "callback-url")
            .map(|(_, v)| v.clone());

        let mut invocations: Vec<String> = Vec::new();

        let indexed_entries: Vec<(OplogIndex, OplogEntry)> = entries
            .into_iter()
            .enumerate()
            .map(|(idx, entry)| ((first_entry_index + (idx as u64)), entry))
            .collect();
        let enriched_entries = enrich_oplog_entries(
            metadata.environment_id.clone(),
            &metadata.agent_id.clone(),
            &indexed_entries,
            metadata.component_revision,
        )
        .unwrap();
        for ((oplog_index, _raw_entry), entry) in indexed_entries.iter().zip(enriched_entries.iter()) {
            if let PublicOplogEntry::AgentInvocationStarted(params) = entry {
                CURRENT_INVOCATIONS.with(|ci| {
                    ci.borrow_mut().insert(format!("{worker_id:?}"), params.clone());
                });
            } else if let PublicOplogEntry::AgentInvocationFinished(_params) = entry {
                if let Some(invocation) =
                    CURRENT_INVOCATIONS.with(|ci| ci.borrow().get(&format!("{worker_id:?}")).cloned())
                {
                    let account_id = Uuid::from_u64_pair(
                        account_info.account_id.uuid.high_bits,
                        account_info.account_id.uuid.low_bits,
                    );

                    let component_id = Uuid::from_u64_pair(
                        component_id.uuid.high_bits,
                        component_id.uuid.low_bits,
                    );

                    let function_name = match &invocation.invocation {
                        AgentInvocation::AgentInitialization(_) => {
                            "agent-initialization".to_string()
                        }
                        AgentInvocation::AgentMethodInvocation(method_params) => {
                            method_params.method_name.clone()
                        }
                        AgentInvocation::SaveSnapshot => "save-snapshot".to_string(),
                        AgentInvocation::LoadSnapshot(_) => "load-snapshot".to_string(),
                        AgentInvocation::ProcessOplogEntries(_) => {
                            "process-oplog-entries".to_string()
                        }
                        AgentInvocation::ManualUpdate(_) => "manual-update".to_string(),
                    };

                    invocations.push(format!(
                        "{}/{}/{}/{}/{}",
                        account_id, component_id, worker_id.agent_id, oplog_index, function_name
                    ));
                } else {
                    println!(
                        "AgentInvocationFinished without corresponding AgentInvocationStarted"
                    )
                }
            }
        }

        if !invocations.is_empty() {
            if let Some(url) = callback_url {
                let json = serde_json::to_string(&invocations).map_err(|err| err.to_string())?;
                block_on(async move {
                    let body = Body::from(json.into_bytes());
                    let request = Request::post(&url)
                        .header("Content-Type", "application/json")
                        .body(body)
                        .map_err(|err| err.to_string())?;
                    let _ = Client::new()
                        .send(request)
                        .await
                        .map_err(|err| err.to_string())?;
                    Ok::<(), String>(())
                })?;
            }
        }

        Ok(())
    }
}

golem_rust::oplog_processor::export_oplog_processor!(OplogProcessorComponent with_types_in golem_rust::oplog_processor);
