use golem_rust::bindings::golem::api::oplog::{
    AgentInvocation, AgentInvocationStartedParameters, OplogEntry, OplogIndex, PublicOplogEntry,
    enrich_oplog_entries,
};
use golem_rust::oplog_processor::exports::golem::api::oplog_processor::Guest as OplogProcessorGuest;
use golem_rust::schema::wit::wire::{AgentId, ComponentId};
use golem_rust::wasip3::http::{client, types};
use golem_rust::wasip3::{wit_future, wit_stream};
use uuid::Uuid;

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
}

struct OplogProcessorComponent;

impl OplogProcessorGuest for OplogProcessorComponent {
    fn process(
        account_info: golem_rust::oplog_processor::exports::golem::api::oplog_processor::AccountInfo,
        config: Vec<(String, String)>,
        component_id: ComponentId,
        worker_id: AgentId,
        metadata: golem_rust::oplog_processor::host::AgentMetadata,
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

        let comp_id = Uuid::from_u64_pair(component_id.uuid.high_bits, component_id.uuid.low_bits);

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
        for ((oplog_index, _raw_entry), entry) in
            indexed_entries.iter().zip(enriched_entries.iter())
        {
            if let PublicOplogEntry::AgentInvocationStarted(params) = entry {
                CURRENT_INVOCATIONS.with(|ci| {
                    ci.borrow_mut()
                        .insert(format!("{worker_id:?}"), params.clone());
                });
            } else if let PublicOplogEntry::AgentInvocationFinished(_params) = entry {
                let function_name = if let Some(invocation) = CURRENT_INVOCATIONS
                    .with(|ci| ci.borrow().get(&format!("{worker_id:?}")).cloned())
                {
                    match &invocation.invocation {
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
                    }
                } else {
                    // AgentInvocationStarted was in a previous batch sent to a
                    // different plugin worker (e.g. shard reassignment / locality
                    // recovery spawned a new instance). Still record the invocation
                    // so we don't silently lose callbacks.
                    println!(
                        "AgentInvocationFinished without corresponding AgentInvocationStarted"
                    );
                    "unknown".to_string()
                };

                invocations.push(InvocationRecord {
                    oplog_index: *oplog_index,
                    fn_name: function_name,
                });
            }
        }

        let entry_count = indexed_entries.len() as u64;

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
            wit_bindgen::block_on(send_http_post(&url, json.into_bytes()))?;
        }

        Ok(())
    }
}

async fn send_http_post(url: &str, body: Vec<u8>) -> Result<(), String> {
    let headers =
        types::Fields::from_list(&[("content-type".to_string(), b"application/json".to_vec())])
            .map_err(|err| format!("invalid headers: {err:?}"))?;

    let (mut body_tx, body_rx) = wit_stream::new();
    let (trailers_tx, trailers_rx) = wit_future::new(|| Ok(None));

    let (request, transmit) = types::Request::new(headers, Some(body_rx), trailers_rx, None);
    request.set_method(&types::Method::Post).unwrap();
    set_request_uri(&request, url)?;

    let (send_result, transmit_result, ()) = futures::join!(
        async { client::send(request).await },
        async { transmit.await },
        async {
            let remaining = body_tx.write_all(body).await;
            assert!(remaining.is_empty());
            let _ = trailers_tx.write(Ok(None)).await;
            drop(body_tx);
        }
    );

    let response = send_result.map_err(|err| format!("HTTP transport error: {err:?}"))?;
    transmit_result.map_err(|err| format!("HTTP request body error: {err:?}"))?;
    drop(response);

    Ok(())
}

fn set_request_uri(request: &types::Request, url: &str) -> Result<(), String> {
    let uri: http::Uri = url
        .parse()
        .map_err(|err| format!("invalid callback URL {url}: {err}"))?;
    match uri.scheme_str() {
        Some("http") => request.set_scheme(Some(&types::Scheme::Http)).unwrap(),
        Some("https") => request.set_scheme(Some(&types::Scheme::Https)).unwrap(),
        Some(scheme) => return Err(format!("unsupported callback URL scheme: {scheme}")),
        None => return Err(format!("callback URL must include a scheme: {url}")),
    }

    let authority = uri
        .authority()
        .ok_or_else(|| format!("callback URL must include an authority: {url}"))?;
    let path_with_query = uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/");
    let normalized_path_with_query;
    let path_with_query = if path_with_query.starts_with('?') {
        normalized_path_with_query = format!("/{path_with_query}");
        normalized_path_with_query.as_str()
    } else {
        path_with_query
    };

    request.set_authority(Some(authority.as_str())).unwrap();
    request.set_path_with_query(Some(path_with_query)).unwrap();

    Ok(())
}

golem_rust::oplog_processor::export_oplog_processor!(OplogProcessorComponent with_types_in golem_rust::oplog_processor);
