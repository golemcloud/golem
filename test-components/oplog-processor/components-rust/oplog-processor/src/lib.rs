use golem_rust::bindings::golem::api::oplog::{
    enrich_oplog_entries, AgentInvocation, AgentInvocationStartedParameters, OplogEntry,
    OplogIndex, PublicOplogEntry,
};
use golem_rust::golem_wasm::golem_core_1_5_x::types::{AgentId, ComponentId};
use golem_rust::oplog_processor::exports::golem::api::oplog_processor::Guest as OplogProcessorGuest;
use uuid::Uuid;
use wstd::http::{Body, Client, Request};
use wstd::runtime::block_on;

use std::cell::RefCell;
use std::collections::HashMap;

/// Per-batch delivery record posted to the callback URL.
#[derive(serde::Serialize)]
struct BatchCallback {
    /// Source worker identity
    source_worker_id: String,
    /// Account that owns the worker
    account_id: String,
    /// Component the worker belongs to
    component_id: String,
    /// Index of the first entry in this batch
    first_entry_index: u64,
    /// Number of entries in this batch
    entry_count: u64,
    /// Invocations observed in this batch (only AgentInvocationFinished entries)
    invocations: Vec<InvocationRecord>,
}

#[derive(serde::Serialize)]
struct InvocationRecord {
    oplog_index: u64,
    fn_name: String,
}

thread_local! {
    static CURRENT_INVOCATIONS: RefCell<HashMap<String, AgentInvocationStartedParameters>> =
        RefCell::new(HashMap::new());
    static LAST_PROCESSED_INDEX: RefCell<HashMap<String, OplogIndex>> =
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
        if entries.is_empty() {
            return Ok(());
        }

        let callback_url = config
            .iter()
            .find(|(k, _)| k == "callback-url")
            .map(|(_, v)| v.clone());

        let account_id = Uuid::from_u64_pair(
            account_info.account_id.uuid.high_bits,
            account_info.account_id.uuid.low_bits,
        );

        let comp_id = Uuid::from_u64_pair(
            component_id.uuid.high_bits,
            component_id.uuid.low_bits,
        );

        let mut invocations: Vec<InvocationRecord> = Vec::new();

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
        for ((oplog_index, _raw_entry), entry) in indexed_entries.iter().zip(enriched_entries.iter())
        {
            if let PublicOplogEntry::AgentInvocationStarted(params) = entry {
                CURRENT_INVOCATIONS.with(|ci| {
                    ci.borrow_mut()
                        .insert(format!("{worker_id:?}"), params.clone());
                });
            } else if let PublicOplogEntry::AgentInvocationFinished(_params) = entry {
                if let Some(invocation) =
                    CURRENT_INVOCATIONS.with(|ci| ci.borrow().get(&format!("{worker_id:?}")).cloned())
                {
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

                    invocations.push(InvocationRecord {
                        oplog_index: *oplog_index,
                        fn_name: function_name,
                    });
                } else {
                    println!(
                        "AgentInvocationFinished without corresponding AgentInvocationStarted"
                    )
                }
            }
        }

        let entry_count = indexed_entries.len() as u64;
        let last_index = first_entry_index + entry_count - 1;
        let source_key = format!("{worker_id:?}");
        LAST_PROCESSED_INDEX.with(|lpi| {
            lpi.borrow_mut().insert(source_key, last_index);
        });

        if let Some(url) = callback_url {
            let batch = BatchCallback {
                source_worker_id: format!("{}", worker_id.agent_id),
                account_id: account_id.to_string(),
                component_id: comp_id.to_string(),
                first_entry_index,
                entry_count,
                invocations,
            };
            let json = serde_json::to_string(&batch).map_err(|err| err.to_string())?;
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

        Ok(())
    }

    fn get_last_processed_index(source_agent_id: AgentId) -> OplogIndex {
        let source_key = format!("{source_agent_id:?}");
        LAST_PROCESSED_INDEX.with(|lpi| lpi.borrow().get(&source_key).copied().unwrap_or(0))
    }
}

golem_rust::oplog_processor::export_oplog_processor!(OplogProcessorComponent with_types_in golem_rust::oplog_processor);
