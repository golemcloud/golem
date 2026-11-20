use crate::raw_http;
use crate::raw_http::Method;
use futures_concurrency::future::Join;
use golem_rust::agentic::AgentStream;
use golem_rust::bindings::golem::agent::host::{Datetime, WasmRpc};
use golem_rust::bindings::golem::api::oplog::{GetOplog, OplogReadError, SearchOplog};
use golem_rust::bindings::golem::tool::host as tool_host;
use golem_rust::retry::{
    CountBoxConfig, NamedRetryPolicy, PolicyNode, PredicateNode, RetryPolicy, RetryPredicate,
    get_retry_policies, get_retry_policy_by_name, remove_retry_policy, set_retry_policy,
    use_retry_policy,
};
use golem_rust::schema::{IntoTypedSchemaValue, try_into_schema_graph};
use golem_rust::{
    AgentAnyFilter, AgentId, AgentMetadata, ForkResult, FromSchema, GetAgents, IntoSchema,
    PromiseId, SchemaValue, Transaction, UpdateMode, Uuid, agent_definition, agent_implementation,
    encode_schema_value, encode_typed_schema_value, fallible_transaction, fork,
    generate_idempotency_key, get_agent_metadata, get_self_metadata, golem_operation,
    infallible_transaction, oplog_commit, resolve_component_id, update_agent,
};
use serde::{Deserialize, Serialize};

pub(crate) mod gated_host_bindings {
    wit_bindgen::generate!({
        path: "../../sdks/rust/golem-rust/wit",
        world: "golem-rust",
        generate_all,
        generate_unused_types: true,
        with: {
            "golem:core/types@2.0.0": golem_rust::schema::wit::wire,
            "wasi:clocks/system-clock@0.3.0": golem_rust::wasip3::clocks::system_clock,
            "wasi:clocks/types@0.3.0": golem_rust::wasip3::clocks::types,
        }
    });
}

use gated_host_bindings::golem::agent::host as agent_host;
use gated_host_bindings::golem::api::host as host_api;

#[derive(
    Clone,
    IntoSchema,
    FromSchema,
    golem_rust::IntoWire,
    golem_rust::FromWire,
    golem_rust::WireSchema,
    Serialize,
    Deserialize,
)]
pub struct ResolveComponentResult {
    pub component_found: bool,
    pub worker_found: bool,
    pub strict_worker_found: bool,
}

type ToolSummary = (String, String, String, Vec<String>, u64, u64);

#[derive(IntoSchema)]
struct EmptyToolInput {}

fn encode_parameters(
    values: Vec<SchemaValue>,
) -> Result<golem_rust::schema::wit::wire::SchemaValueTree, String> {
    encode_schema_value(&SchemaValue::Record { fields: values })
        .map_err(|error| format!("{error:?}"))
}

fn wire_agent_id(agent_id: AgentId) -> golem_rust::schema::wit::wire::AgentId {
    (&agent_id).into()
}

/// Gets the ID of the keyed agent of `agent_type` whose single constructor parameter is `name`.
fn lookup_agent_id(
    agent_type: &str,
    name: &str,
) -> Result<Option<golem_rust::schema::wit::wire::AgentId>, String> {
    agent_host::lookup_agent_id(
        agent_type,
        encode_parameters(vec![SchemaValue::String(name.to_string())])?,
    )
    .map_err(|error| format!("{error:?}"))
}

fn encode_tool_input(
    _input: String,
) -> Result<golem_rust::schema::wit::wire::TypedSchemaValue, String> {
    let input = EmptyToolInput {}
        .into_typed_schema_value()
        .map_err(|error| format!("{error:?}"))?;
    encode_typed_schema_value(&input).map_err(|error| format!("{error:?}"))
}

#[agent_definition()]
pub trait GolemHostApi {
    fn new(name: String) -> Self;

    fn resolve_component(&self) -> ResolveComponentResult;

    fn create_promise(&self) -> PromiseId;
    fn await_promise(&self, promise_id: PromiseId) -> Vec<u8>;
    /// Makes `count` local durable calls, which commit nothing explicitly, then awaits the promise.
    fn local_calls_then_await_promise(&self, count: u64, promise_id: PromiseId) -> Vec<u8>;
    fn create_and_await_promise(&self) -> Vec<u8>;
    fn fail_with_custom_max_retries(&self, max_retries: u64);
    fn trap_after_recorded_effect(&self, authority: String, via_host: bool) -> String;
    fn explicit_commit(&self, replicas: u8);
    async fn fallible_transaction_test(&self) -> u64;
    async fn infallible_transaction_test(&self) -> u64;
    fn fork_test(&self, input: String) -> String;

    fn get_workers(
        &self,
        agent_type: String,
        filter: Option<AgentAnyFilter>,
        precise: bool,
    ) -> Vec<AgentMetadata>;
    fn get_agents_next_result(&self, agent_type: String) -> Result<u64, String>;
    fn get_agents_across_promise(
        &self,
        agent_type: String,
        promise_id: PromiseId,
    ) -> Result<Vec<Vec<String>>, String>;
    fn get_self_metadata_result(&self) -> Result<String, String>;
    fn lookup_agent_id_result(&self, agent_type: String, name: String) -> Result<bool, String>;
    fn self_fork_result(&self) -> Result<String, String>;
    fn get_self_uri(&self) -> AgentMetadata;
    fn get_worker_metadata(&self, agent_id: AgentId) -> Option<AgentMetadata>;
    fn update_worker(&self, agent_id: AgentId, component_revision: u64, update_mode: UpdateMode);
    fn generate_idempotency_keys(&self) -> (Uuid, Uuid);
    fn list_retry_policy_names(&self) -> Vec<String>;
    fn get_retry_policy_count(&self) -> u64;
    fn set_simple_count_retry_policy(&self, name: String, priority: u32, max_retries: u32);
    fn remove_named_retry_policy(&self, name: String);
    fn has_retry_policy(&self, name: String) -> bool;

    fn get_all_tools(&self) -> Vec<ToolSummary>;
    fn get_tool(&self, name: String) -> Option<ToolSummary>;
    fn record_all_tools(&mut self) -> Vec<ToolSummary>;
    fn get_recorded_tools(&self) -> Vec<ToolSummary>;
    fn outbound_agent_rpc_invoke_result(
        &self,
        agent_type: String,
        constructor: String,
        method: String,
    ) -> Result<(), String>;
    async fn outbound_agent_rpc_async_invoke_and_await_result(
        &self,
        agent_type: String,
        constructor: String,
        method: String,
    ) -> Result<(), String>;
    fn outbound_agent_rpc_invoke_and_await_result(
        &self,
        agent_type: String,
        constructor: String,
        method: String,
    ) -> Result<(), String>;
    async fn outbound_agent_rpc_create_unkeyed_result(
        &self,
        agent_type: String,
        constructor: String,
    ) -> Result<String, String>;
    fn outbound_agent_rpc_schedule_result(
        &self,
        agent_type: String,
        constructor: String,
        method: String,
    ) -> Result<(), String>;
    fn outbound_agent_rpc_schedule_cancelable_result(
        &self,
        agent_type: String,
        constructor: String,
        method: String,
    ) -> Result<(), String>;
    /// Creates an RPC handle to the agent, calls `method` through it, and keeps the handle for
    /// the `kept_agent_rpc_*` methods of later invocations.
    fn keep_agent_rpc(
        &mut self,
        agent_type: String,
        constructor: String,
        method: String,
    ) -> Result<(), String>;
    fn kept_agent_rpc_invoke_and_await_result(&self, method: String) -> Result<(), String>;
    async fn kept_agent_rpc_async_invoke_and_await_result(
        &self,
        method: String,
    ) -> Result<(), String>;
    fn kept_agent_rpc_invoke_result(&self, method: String) -> Result<(), String>;
    /// Schedules `method` through the kept handle for a time that never comes due in a test.
    fn kept_agent_rpc_schedule_result(&self, method: String) -> Result<(), String>;
    /// Calls a method returning `AgentStream<u64>` through the kept handle and drains the stream.
    async fn kept_agent_rpc_invoke_and_await_u64_stream_result(
        &self,
        method: String,
    ) -> Result<Vec<u64>, String>;
    fn read_own_oplog_result(&self) -> Result<(), String>;
    fn search_own_oplog_result(&self, query: String) -> Result<(), String>;
    /// Reads a search of the agent's own oplog until it ends, and gives the number of batches.
    /// Fails when the search has not ended after `max_batches` batches.
    fn search_own_oplog_to_end(&self, query: String, max_batches: u64) -> Result<u64, String>;
    /// Opens a search of the agent's own oplog, reads its first batch, and keeps the search for
    /// `continue_kept_oplog_search`.
    fn open_kept_oplog_search(&mut self, query: String) -> Result<(), String>;
    /// Reads the kept search until it ends, and gives the indexes of the entries that it found.
    /// Fails when the search has not ended after `max_batches` batches.
    fn continue_kept_oplog_search(&self, max_batches: u64) -> Result<Vec<u64>, String>;
    fn read_oplog_result(&self, agent_id: AgentId) -> Result<(), String>;
    fn search_oplog_result(
        &mut self,
        agent_id: AgentId,
        query: String,
    ) -> Result<Option<Vec<u64>>, String>;
    /// Enriches a forged raw failed update at `entry_index` of the agent's own oplog, whose
    /// divergence names `divergence_index` with an expected host call of unknown request.
    fn enrich_forged_failed_update_result(
        &self,
        entry_index: u64,
        divergence_index: Option<u64>,
    ) -> Result<(), String>;
    fn get_config_value_result(&self, key: Vec<String>) -> Result<(), String>;
    fn create_webhook_result(&self, promise_id: PromiseId) -> Result<(), String>;
    fn get_agent_metadata_result(&self, agent_id: AgentId) -> Result<bool, String>;
    fn update_agent_result(&self, agent_id: AgentId, revision: u64) -> Result<(), String>;
    fn fork_agent_result(&self, source_agent_id: AgentId, oplog_index: u64) -> Result<(), String>;
    fn revert_agent_result(&self, agent_id: AgentId, oplog_index: u64) -> Result<(), String>;
    fn revert_last_invocations_result(&self, agent_id: AgentId, count: u64) -> Result<(), String>;
    async fn tool_rpc_invoke_result(
        &self,
        tool_name: String,
        command_path: Vec<String>,
        input: String,
    ) -> Result<(), String>;
    async fn tool_rpc_async_invoke_and_await_result(
        &self,
        tool_name: String,
        command_path: Vec<String>,
        input: String,
    ) -> Result<(), String>;
    async fn tool_rpc_invoke_and_await_result(
        &self,
        tool_name: String,
        command_path: Vec<String>,
        input: String,
    ) -> Result<(), String>;
    async fn tool_rpc_collect_stdout(
        &self,
        tool_name: String,
        checkpoint: Option<String>,
    ) -> (Result<(), String>, Result<Vec<u8>, String>);
    async fn tool_rpc_cancel_and_collect_stdout(
        &self,
        tool_name: String,
    ) -> (Result<(), String>, Result<Vec<u8>, String>);
}

#[agent_definition(mode = "ephemeral")]
pub trait EphemeralPromise {
    fn new(name: String) -> Self;
    fn create_and_await_promise(&self) -> Vec<u8>;
}

pub struct EphemeralPromiseImpl;

#[agent_implementation]
impl EphemeralPromise for EphemeralPromiseImpl {
    fn new(_name: String) -> Self {
        Self
    }

    fn create_and_await_promise(&self) -> Vec<u8> {
        golem_rust::blocking_await_promise(&golem_rust::create_promise())
    }
}

pub struct GolemHostApiImpl {
    _name: String,
    recorded_tools: Option<Vec<ToolSummary>>,
    kept_rpc: Option<WasmRpc>,
    kept_search: Option<SearchOplog>,
}

impl GolemHostApiImpl {
    fn kept_rpc(&self) -> &WasmRpc {
        self.kept_rpc
            .as_ref()
            .expect("keep_agent_rpc must be called before a kept_agent_rpc method")
    }
}

fn summarize_tool(tool: tool_host::RegisteredTool) -> ToolSummary {
    let (summary, aliases) = tool
        .definition
        .commands
        .nodes
        .first()
        .map(|node| (node.doc.summary.clone(), node.aliases.clone()))
        .unwrap_or_default();
    let uuid = tool.implemented_by.uuid;
    (
        tool.lookup_name,
        tool.definition.version,
        summary,
        aliases,
        uuid.high_bits,
        uuid.low_bits,
    )
}

#[agent_definition()]
pub trait ToolDiscoveryOther {
    fn new(name: String) -> Self;

    fn get_all_tools(&self) -> Vec<ToolSummary>;
}

pub struct ToolDiscoveryOtherImpl {
    _name: String,
}

#[agent_implementation]
impl ToolDiscoveryOther for ToolDiscoveryOtherImpl {
    fn new(name: String) -> Self {
        Self { _name: name }
    }

    fn get_all_tools(&self) -> Vec<ToolSummary> {
        tool_host::get_all_tools()
            .into_iter()
            .map(summarize_tool)
            .collect()
    }
}

#[agent_implementation]
impl GolemHostApi for GolemHostApiImpl {
    fn new(name: String) -> Self {
        Self {
            _name: name,
            recorded_tools: None,
            kept_rpc: None,
            kept_search: None,
        }
    }

    fn resolve_component(&self) -> ResolveComponentResult {
        let component_id = resolve_component_id("component-resolve-target");
        let worker_id = lookup_agent_id("RpcCounter", "counter-1").expect("agent lookup failed");
        let strict_worker_id =
            lookup_agent_id("RpcCounter", "counter-2").expect("agent lookup failed");
        ResolveComponentResult {
            component_found: component_id.is_some(),
            worker_found: worker_id.is_some(),
            strict_worker_found: strict_worker_id.is_some(),
        }
    }

    fn create_promise(&self) -> PromiseId {
        golem_rust::create_promise()
    }

    fn await_promise(&self, promise_id: PromiseId) -> Vec<u8> {
        golem_rust::blocking_await_promise(&promise_id)
    }

    fn local_calls_then_await_promise(&self, count: u64, promise_id: PromiseId) -> Vec<u8> {
        for _ in 0..count {
            let _ = wasi::clocks::wall_clock::now();
        }
        golem_rust::blocking_await_promise(&promise_id)
    }

    fn create_and_await_promise(&self) -> Vec<u8> {
        golem_rust::blocking_await_promise(&golem_rust::create_promise())
    }

    fn fail_with_custom_max_retries(&self, max_retries: u64) {
        let policy = NamedRetryPolicy {
            name: "__fail_with_custom_max_retries".to_string(),
            priority: 0,
            predicate: RetryPredicate {
                nodes: vec![PredicateNode::PredTrue],
            },
            policy: RetryPolicy {
                nodes: vec![
                    PolicyNode::CountBox(CountBoxConfig {
                        max_retries: max_retries as u32,
                        inner: 1,
                    }),
                    PolicyNode::Periodic(1_000_000_000u64), // 1 second
                ],
            },
        };
        let _guard = use_retry_policy(policy);

        panic!("Fail now");
    }

    fn trap_after_recorded_effect(&self, authority: String, via_host: bool) -> String {
        let (status, _) = raw_http::request(Method::Post, &authority, "/effect", None, None);
        assert_eq!(status, 200);
        if via_host {
            host_api::trap("terminal compensation failure");
        } else {
            panic!("terminal compensation failure");
        }
        "trap returned".to_string()
    }

    fn explicit_commit(&self, replicas: u8) {
        let now = std::time::SystemTime::now();
        println!("Starting commit with {replicas} replicas at {now:?}");
        oplog_commit(replicas);
        println!("Finished commit");
    }

    async fn fallible_transaction_test(&self) -> u64 {
        fallible_transaction(|tx| {
            golem_rust::boxed(async move {
                tx.transaction_step(1).await?;
                tx.transaction_step(2).await?;
                if tx.transaction_step(3).await? {
                    tx.fail("fail after 3".to_string()).await?
                }
                tx.transaction_step(4).await?;
                Ok(11)
            })
        })
        .await
        .expect("Transaction failed")
    }

    async fn infallible_transaction_test(&self) -> u64 {
        infallible_transaction(|tx| {
            golem_rust::boxed(async move {
                tx.transaction_step(1).await?;
                tx.transaction_step(2).await?;
                if tx.transaction_step(3).await? {
                    return Err("retry after 3".to_string());
                }
                tx.transaction_step(4).await?;
                Ok(11)
            })
        })
        .await
        .unwrap_or_else(|failure| match failure {
            golem_rust::TransactionFailure::RetryExhausted(_) => 0,
            golem_rust::TransactionFailure::RetryPolicyError { .. } => 1,
            golem_rust::TransactionFailure::FailedAndRolledBackPartially { .. } => 2,
            golem_rust::TransactionFailure::FailedAndRolledBackCompletely(_) => 3,
        })
    }

    fn fork_test(&self, input: String) -> String {
        let port = std::env::var("PORT").unwrap_or("9999".to_string());
        let authority = format!("localhost:{port}");
        let self_name = get_self_metadata()
            .expect("self metadata access should be allowed")
            .agent_id
            .uuid
            .to_string();

        let path = format!("/fork-test/step1/{self_name}/{input}");
        println!("Sending GET {path}");

        let (_status, body) = raw_http::request(Method::Get, &authority, &path, None, None);
        let part1_raw = String::from_utf8(body).expect("Invalid response");
        println!("Received {part1_raw}");

        let part1: String = serde_json::from_str(&part1_raw).unwrap();

        let path = match fork().expect("self fork should be allowed") {
            ForkResult::Original(details) => {
                let uuid = details.forked_agent_id.uuid;
                format!("/fork-test/step2/{self_name}/original/{uuid}")
            }
            ForkResult::Forked(details) => {
                let self_name = get_self_metadata()
                    .expect("self metadata access should be allowed")
                    .agent_id
                    .uuid
                    .to_string();
                let uuid = details.forked_agent_id.uuid;
                format!("/fork-test/step2/{self_name}/forked/{uuid}")
            }
        };

        println!("Trying to call {path}");
        let (_status, body) = raw_http::request(Method::Get, &authority, &path, None, None);
        let part2_raw = String::from_utf8(body).expect("Invalid response");
        println!("Received {part2_raw}");

        let part2: String = serde_json::from_str(&part2_raw).unwrap();

        format!("{part1}::{part2}")
    }

    fn get_workers(
        &self,
        agent_type: String,
        filter: Option<AgentAnyFilter>,
        precise: bool,
    ) -> Vec<AgentMetadata> {
        let mut workers: Vec<AgentMetadata> = Vec::new();
        let getter = GetAgents::new(&agent_type, filter.as_ref(), precise);
        while let Some(values) = getter
            .get_next()
            .expect("agent enumeration should be allowed")
        {
            workers.extend(values);
        }
        workers
    }

    fn get_agents_next_result(&self, agent_type: String) -> Result<u64, String> {
        let getter = host_api::GetAgents::new(&agent_type, None, false);
        getter
            .get_next()
            .map(|agents| agents.map_or(0, |agents| agents.len() as u64))
            .map_err(|error| format!("{error:?}"))
    }

    fn get_agents_across_promise(
        &self,
        agent_type: String,
        promise_id: PromiseId,
    ) -> Result<Vec<Vec<String>>, String> {
        let getter = GetAgents::new(&agent_type, None, false);
        let first = getter
            .get_next()
            .map_err(|error| format!("{error:?}"))?
            .ok_or_else(|| "expected first agent page".to_string())?;

        golem_rust::blocking_await_promise(&promise_id);

        let second = getter
            .get_next()
            .map_err(|error| format!("{error:?}"))?
            .ok_or_else(|| "expected second agent page".to_string())?;
        if getter
            .get_next()
            .map_err(|error| format!("{error:?}"))?
            .is_some()
        {
            return Err("expected agent enumeration to be exhausted".to_string());
        }

        Ok(vec![
            first
                .into_iter()
                .map(|metadata| metadata.agent_id.uuid.to_string())
                .collect(),
            second
                .into_iter()
                .map(|metadata| metadata.agent_id.uuid.to_string())
                .collect(),
        ])
    }

    fn get_self_metadata_result(&self) -> Result<String, String> {
        host_api::get_self_metadata()
            .map(|metadata| golem_rust::Uuid::from(metadata.agent_id.uuid).to_string())
            .map_err(|error| format!("{error:?}"))
    }

    fn lookup_agent_id_result(&self, agent_type: String, name: String) -> Result<bool, String> {
        lookup_agent_id(&agent_type, &name).map(|agent_id| agent_id.is_some())
    }

    fn self_fork_result(&self) -> Result<String, String> {
        println!("fork checkpoint");
        host_api::fork()
            .map(|result| match result {
                host_api::ForkResult::Original(_) => "original".to_string(),
                host_api::ForkResult::Forked(_) => "forked".to_string(),
            })
            .map_err(|error| format!("{error:?}"))
    }

    fn get_self_uri(&self) -> AgentMetadata {
        get_self_metadata().expect("self metadata access should be allowed")
    }

    fn get_worker_metadata(&self, agent_id: AgentId) -> Option<AgentMetadata> {
        get_agent_metadata(&agent_id)
    }

    fn update_worker(&self, agent_id: AgentId, component_revision: u64, update_mode: UpdateMode) {
        update_agent(&agent_id, component_revision, update_mode)
            .expect("agent update should be allowed");
    }

    fn generate_idempotency_keys(&self) -> (Uuid, Uuid) {
        let key1 = generate_idempotency_key();
        let key2 = generate_idempotency_key();
        (key1, key2)
    }

    fn list_retry_policy_names(&self) -> Vec<String> {
        let mut names: Vec<String> = get_retry_policies().into_iter().map(|p| p.name).collect();
        names.sort();
        names
    }

    fn get_retry_policy_count(&self) -> u64 {
        get_retry_policies().len() as u64
    }

    fn set_simple_count_retry_policy(&self, name: String, priority: u32, max_retries: u32) {
        let policy = NamedRetryPolicy {
            name,
            priority,
            predicate: RetryPredicate {
                nodes: vec![PredicateNode::PredTrue],
            },
            policy: RetryPolicy {
                nodes: vec![
                    PolicyNode::CountBox(CountBoxConfig {
                        max_retries,
                        inner: 1,
                    }),
                    PolicyNode::Periodic(1_000_000_000u64),
                ],
            },
        };
        set_retry_policy(&policy);
    }

    fn remove_named_retry_policy(&self, name: String) {
        remove_retry_policy(&name);
    }

    fn has_retry_policy(&self, name: String) -> bool {
        get_retry_policy_by_name(&name).is_some()
    }

    fn get_all_tools(&self) -> Vec<ToolSummary> {
        tool_host::get_all_tools()
            .into_iter()
            .map(summarize_tool)
            .collect()
    }

    fn get_tool(&self, name: String) -> Option<ToolSummary> {
        tool_host::get_tool(&name).map(summarize_tool)
    }

    fn record_all_tools(&mut self) -> Vec<ToolSummary> {
        let tools = self.get_all_tools();
        self.recorded_tools = Some(tools.clone());
        tools
    }

    fn get_recorded_tools(&self) -> Vec<ToolSummary> {
        self.recorded_tools
            .clone()
            .expect("record_all_tools must be called before get_recorded_tools")
    }

    fn outbound_agent_rpc_invoke_result(
        &self,
        agent_type: String,
        constructor: String,
        method: String,
    ) -> Result<(), String> {
        let rpc = WasmRpc::upsert(
            &agent_type,
            encode_parameters(vec![SchemaValue::String(constructor)])?,
        )
        .map_err(|error| format!("{error:?}"))?;
        rpc.invoke(&method, encode_parameters(Vec::new())?, None)
            .map(|_| ())
            .map_err(|error| format!("{error:?}"))
    }

    async fn outbound_agent_rpc_async_invoke_and_await_result(
        &self,
        agent_type: String,
        constructor: String,
        method: String,
    ) -> Result<(), String> {
        let rpc = WasmRpc::upsert(
            &agent_type,
            encode_parameters(vec![SchemaValue::String(constructor)])?,
        )
        .map_err(|error| format!("{error:?}"))?;
        rpc.async_invoke_and_await(&method, encode_parameters(Vec::new())?, None)
            .future
            .get()
            .await
            .map(|_| ())
            .map_err(|error| format!("{error:?}"))
    }

    fn outbound_agent_rpc_invoke_and_await_result(
        &self,
        agent_type: String,
        constructor: String,
        method: String,
    ) -> Result<(), String> {
        let rpc = WasmRpc::upsert(
            &agent_type,
            encode_parameters(vec![SchemaValue::String(constructor)])?,
        )
        .map_err(|error| format!("{error:?}"))?;
        rpc.invoke_and_await(&method, encode_parameters(Vec::new())?, None)
            .map(|_| ())
            .map_err(|error| format!("{error:?}"))
    }

    async fn outbound_agent_rpc_create_unkeyed_result(
        &self,
        agent_type: String,
        constructor: String,
    ) -> Result<String, String> {
        let created = WasmRpc::create(
            agent_type,
            encode_parameters(vec![SchemaValue::String(constructor)])?,
            golem_rust::bindings::golem::agent::host::CreateOptions {
                keyed: false,
                agent_config: Vec::new(),
                env: Vec::new(),
            },
        )
        .await
        .map_err(|error| format!("{error:?}"))?;
        Ok(golem_rust::Uuid::from(created.agent_id.uuid).to_string())
    }

    fn outbound_agent_rpc_schedule_result(
        &self,
        agent_type: String,
        constructor: String,
        method: String,
    ) -> Result<(), String> {
        let rpc = WasmRpc::upsert(
            &agent_type,
            encode_parameters(vec![SchemaValue::String(constructor)])?,
        )
        .map_err(|error| format!("{error:?}"))?;
        rpc.schedule_invocation(
            Datetime {
                seconds: 0,
                nanoseconds: 0,
            },
            &method,
            encode_parameters(Vec::new())?,
            None,
        )
        .map(|_| ())
        .map_err(|error| format!("{error:?}"))
    }

    fn outbound_agent_rpc_schedule_cancelable_result(
        &self,
        agent_type: String,
        constructor: String,
        method: String,
    ) -> Result<(), String> {
        let rpc = WasmRpc::upsert(
            &agent_type,
            encode_parameters(vec![SchemaValue::String(constructor)])?,
        )
        .map_err(|error| format!("{error:?}"))?;
        rpc.schedule_cancelable_invocation(
            Datetime {
                seconds: 0,
                nanoseconds: 0,
            },
            &method,
            encode_parameters(Vec::new())?,
            None,
        )
        .map(|_| ())
        .map_err(|error| format!("{error:?}"))
    }

    fn keep_agent_rpc(
        &mut self,
        agent_type: String,
        constructor: String,
        method: String,
    ) -> Result<(), String> {
        let rpc = WasmRpc::upsert(
            &agent_type,
            encode_parameters(vec![SchemaValue::String(constructor)])?,
        )
        .map_err(|error| format!("{error:?}"))?;
        rpc.invoke_and_await(&method, encode_parameters(Vec::new())?, None)
            .map_err(|error| format!("{error:?}"))?;
        self.kept_rpc = Some(rpc);
        Ok(())
    }

    fn kept_agent_rpc_invoke_and_await_result(&self, method: String) -> Result<(), String> {
        self.kept_rpc()
            .invoke_and_await(&method, encode_parameters(Vec::new())?, None)
            .map(|_| ())
            .map_err(|error| format!("{error:?}"))
    }

    async fn kept_agent_rpc_async_invoke_and_await_result(
        &self,
        method: String,
    ) -> Result<(), String> {
        self.kept_rpc()
            .async_invoke_and_await(&method, encode_parameters(Vec::new())?, None)
            .future
            .get()
            .await
            .map(|_| ())
            .map_err(|error| format!("{error:?}"))
    }

    fn kept_agent_rpc_invoke_result(&self, method: String) -> Result<(), String> {
        self.kept_rpc()
            .invoke(&method, encode_parameters(Vec::new())?, None)
            .map(|_| ())
            .map_err(|error| format!("{error:?}"))
    }

    fn kept_agent_rpc_schedule_result(&self, method: String) -> Result<(), String> {
        self.kept_rpc()
            .schedule_invocation(
                // 2100-01-01, so the scheduled call never comes due during a test.
                Datetime {
                    seconds: 4_102_444_800,
                    nanoseconds: 0,
                },
                &method,
                encode_parameters(Vec::new())?,
                None,
            )
            .map(|_| ())
            .map_err(|error| format!("{error:?}"))
    }

    async fn kept_agent_rpc_invoke_and_await_u64_stream_result(
        &self,
        method: String,
    ) -> Result<Vec<u64>, String> {
        let result = self
            .kept_rpc()
            .invoke_and_await(&method, encode_parameters(Vec::new())?, None)
            .map_err(|error| format!("{error:?}"))?
            .result
            .ok_or_else(|| format!("{method} returned no value"))?;
        let value =
            golem_rust::decode_schema_value(result).map_err(|error| format!("{error:?}"))?;
        AgentStream::<u64>::from_value(&value)
            .map_err(|error| format!("{error:?}"))?
            .collect()
            .await
            .map_err(|error| format!("{error:?}"))
    }

    fn read_own_oplog_result(&self) -> Result<(), String> {
        let metadata = get_self_metadata().map_err(|error| format!("{error:?}"))?;
        let agent_id = wire_agent_id(metadata.agent_id);
        GetOplog::new(agent_id, 0)
            .get_next()
            .map(|_| ())
            .map_err(|error| match error {
                OplogReadError::PermissionDenied => "permission denied".to_string(),
                OplogReadError::InternalError(message) => message,
            })
    }

    fn search_own_oplog_result(&self, query: String) -> Result<(), String> {
        let agent_id = wire_agent_id(
            get_self_metadata()
                .map_err(|error| format!("{error:?}"))?
                .agent_id,
        );
        SearchOplog::new(agent_id, &query)
            .get_next()
            .map(|_| ())
            .map_err(|error| format!("{error:?}"))
    }

    fn search_own_oplog_to_end(&self, query: String, max_batches: u64) -> Result<u64, String> {
        let agent_id = wire_agent_id(
            get_self_metadata()
                .map_err(|error| format!("{error:?}"))?
                .agent_id,
        );
        let search = SearchOplog::new(agent_id, &query);
        for batch in 0..max_batches {
            if search
                .get_next()
                .map_err(|error| format!("{error:?}"))?
                .is_none()
            {
                return Ok(batch);
            }
        }
        Err(format!(
            "the search did not end after {max_batches} batches"
        ))
    }

    fn open_kept_oplog_search(&mut self, query: String) -> Result<(), String> {
        let agent_id = wire_agent_id(
            get_self_metadata()
                .map_err(|error| format!("{error:?}"))?
                .agent_id,
        );
        let search = SearchOplog::new(agent_id, &query);
        search.get_next().map_err(|error| format!("{error:?}"))?;
        self.kept_search = Some(search);
        Ok(())
    }

    fn continue_kept_oplog_search(&self, max_batches: u64) -> Result<Vec<u64>, String> {
        let search = self
            .kept_search
            .as_ref()
            .ok_or("open_kept_oplog_search must be called first")?;
        let mut found = Vec::new();
        for _ in 0..max_batches {
            match search.get_next().map_err(|error| format!("{error:?}"))? {
                Some(batch) => found.extend(batch.into_iter().map(|(index, _)| index)),
                None => return Ok(found),
            }
        }
        Err(format!(
            "the search did not end after {max_batches} batches"
        ))
    }

    fn read_oplog_result(&self, agent_id: AgentId) -> Result<(), String> {
        GetOplog::new(wire_agent_id(agent_id), 0)
            .get_next()
            .map(|_| ())
            .map_err(|error| format!("{error:?}"))
    }

    fn search_oplog_result(
        &mut self,
        agent_id: AgentId,
        query: String,
    ) -> Result<Option<Vec<u64>>, String> {
        let search = SearchOplog::new(wire_agent_id(agent_id), &query);
        let batch = search.get_next().map_err(|error| format!("{error:?}"))?;
        self.kept_search = Some(search);
        Ok(batch.map(|entries| entries.into_iter().map(|(index, _)| index).collect()))
    }

    fn enrich_forged_failed_update_result(
        &self,
        entry_index: u64,
        divergence_index: Option<u64>,
    ) -> Result<(), String> {
        use gated_host_bindings::golem::api::oplog;

        let metadata = host_api::get_self_metadata().map_err(|error| format!("{error:?}"))?;
        let divergence = divergence_index.map(|oplog_index| oplog::ReplayDivergence {
            oplog_index,
            expected: oplog::ReplayStep::HostCall(oplog::ReplayHostCall {
                function_name: "golem::api::get_self_metadata".to_string(),
                durable_function_type: oplog::WrappedFunctionType::ReadLocal,
                parent_start_index: None,
                invocation_id: None,
                observational_owner: None,
                carries_request: true,
                request: None,
            }),
            actual: oplog::ReplayStep::EndOfHistory,
            mismatched: vec![],
        });
        let entry = oplog::OplogEntry::FailedUpdate(oplog::FailedUpdateParameters {
            timestamp: golem_rust::wasip3::clocks::system_clock::now(),
            target_revision: metadata.component_revision,
            details: None,
            snapshot_assisted_details: None,
            update_attempt_index: None,
            failure_class: oplog::UpdateFailureClass::Divergence,
            divergence,
        });
        oplog::enrich_oplog_entries(
            metadata.environment_id,
            metadata.agent_id,
            vec![(entry_index, entry)],
            metadata.component_revision,
        )
        .map(|_| ())
    }

    fn get_config_value_result(&self, key: Vec<String>) -> Result<(), String> {
        let expected =
            try_into_schema_graph::<Option<String>>().map_err(|error| format!("{error:?}"))?;
        let expected =
            golem_rust::encode_schema_graph(&expected).map_err(|error| format!("{error:?}"))?;
        agent_host::get_config_value(&key, &expected)
            .map(|_| ())
            .map_err(|error| format!("{error:?}"))
    }

    fn create_webhook_result(&self, promise_id: PromiseId) -> Result<(), String> {
        agent_host::create_webhook((&promise_id).into())
            .map(|_| ())
            .map_err(|error| format!("{error:?}"))
    }

    fn get_agent_metadata_result(&self, agent_id: AgentId) -> Result<bool, String> {
        Ok(host_api::get_agent_metadata(wire_agent_id(agent_id)).is_some())
    }

    fn update_agent_result(&self, agent_id: AgentId, revision: u64) -> Result<(), String> {
        host_api::update_agent(
            wire_agent_id(agent_id),
            revision,
            host_api::UpdateMode::Automatic,
        )
        .map_err(|error| format!("{error:?}"))
    }

    fn fork_agent_result(&self, source_agent_id: AgentId, oplog_index: u64) -> Result<(), String> {
        host_api::fork_agent(wire_agent_id(source_agent_id), oplog_index)
            .map(|_| ())
            .map_err(|error| format!("{error:?}"))
    }

    fn revert_agent_result(&self, agent_id: AgentId, oplog_index: u64) -> Result<(), String> {
        host_api::revert_agent(
            wire_agent_id(agent_id),
            host_api::RevertAgentTarget::RevertToOplogIndex(oplog_index),
        )
        .map_err(|error| format!("{error:?}"))
    }

    fn revert_last_invocations_result(&self, agent_id: AgentId, count: u64) -> Result<(), String> {
        host_api::revert_agent(
            wire_agent_id(agent_id),
            host_api::RevertAgentTarget::RevertLastInvocations(count),
        )
        .map_err(|error| format!("{error:?}"))
    }

    async fn tool_rpc_invoke_result(
        &self,
        tool_name: String,
        command_path: Vec<String>,
        input: String,
    ) -> Result<(), String> {
        tool_host::ToolRpc::create(&tool_name)
            .map_err(|error| format!("{error:?}"))?
            .invoke(&command_path, encode_tool_input(input)?, None)
            .map(|_| ())
            .map_err(|error| format!("{error:?}"))
    }

    async fn tool_rpc_async_invoke_and_await_result(
        &self,
        tool_name: String,
        command_path: Vec<String>,
        input: String,
    ) -> Result<(), String> {
        tool_host::ToolRpc::create(&tool_name)
            .map_err(|error| format!("{error:?}"))?
            .async_invoke_and_await(&command_path, encode_tool_input(input)?, None, None, None)
            .get()
            .await
            .map(|_| ())
            .map_err(|error| format!("{error:?}"))
    }

    async fn tool_rpc_invoke_and_await_result(
        &self,
        tool_name: String,
        command_path: Vec<String>,
        input: String,
    ) -> Result<(), String> {
        tool_host::ToolRpc::create(&tool_name)
            .map_err(|error| format!("{error:?}"))?
            .invoke_and_await(command_path, encode_tool_input(input)?, None, None, None)
            .await
            .map(|_| ())
            .map_err(|error| format!("{error:?}"))
    }

    async fn tool_rpc_collect_stdout(
        &self,
        tool_name: String,
        checkpoint: Option<String>,
    ) -> (Result<(), String>, Result<Vec<u8>, String>) {
        let (stdout_target, mut stdout) = tool_host::create_output();
        let result = tool_host::ToolRpc::create(&tool_name)
            .expect("tool RPC creation failed")
            .async_invoke_and_await(
                &[],
                encode_tool_input(String::new()).expect("encode empty tool input"),
                None,
                Some(stdout_target),
                None,
            );
        if let Some(checkpoint) = checkpoint {
            let port = std::env::var("MCP_STDOUT_CHECKPOINT_PORT")
                .expect("MCP_STDOUT_CHECKPOINT_PORT is configured");
            raw_http::request_async(
                Method::Get,
                &format!("localhost:{port}"),
                &format!("/checkpoint/{checkpoint}"),
                None,
                None,
            )
            .await;
        }
        let read = async move {
            let mut bytes = Vec::new();
            while let Some(item) = stdout.next().await {
                match item {
                    Ok(chunk) => bytes.extend(chunk),
                    Err(error) => return Err(format!("{error:?}")),
                }
            }
            Ok(bytes)
        };
        let (result, stdout) = (result.get(), read).join().await;
        (
            result.map(|_| ()).map_err(|error| format!("{error:?}")),
            stdout,
        )
    }

    async fn tool_rpc_cancel_and_collect_stdout(
        &self,
        tool_name: String,
    ) -> (Result<(), String>, Result<Vec<u8>, String>) {
        let (stdout_target, mut stdout) = tool_host::create_output();
        let result = tool_host::ToolRpc::create(&tool_name)
            .expect("tool RPC creation failed")
            .async_invoke_and_await(
                &[],
                encode_tool_input(String::new()).expect("encode empty tool input"),
                None,
                Some(stdout_target),
                None,
            );
        let port = std::env::var("MCP_STDOUT_CHECKPOINT_PORT")
            .expect("MCP_STDOUT_CHECKPOINT_PORT is configured");
        let response = wasi_fetch::Client::new()
            .get(&format!("http://localhost:{port}/checkpoint/{tool_name}"))
            .send()
            .await
            .expect("MCP cancellation checkpoint request failed");
        assert_eq!(response.status().as_u16(), 200);
        let _ = response.into_body().bytes().await;
        result.cancel();
        let read = async move {
            let mut bytes = Vec::new();
            while let Some(item) = stdout.next().await {
                match item {
                    Ok(chunk) => bytes.extend(chunk),
                    Err(error) => return Err(format!("{error:?}")),
                }
            }
            Ok(bytes)
        };
        let (result, stdout) = (result.get(), read).join().await;
        // Make the observed stream terminal part of a subsequent durable claim.
        // Returning it alone does not validate what reconstruction recomputes.
        let _ = tool_host::ToolRpc::create(&tool_name)
            .expect("tool RPC creation failed")
            .invoke_and_await(
                vec![format!("observed-{stdout:?}")],
                encode_tool_input(String::new()).expect("encode empty tool input"),
                None,
                None,
                None,
            )
            .await;
        (
            result.map(|_| ()).map_err(|error| format!("{error:?}")),
            stdout,
        )
    }
}

#[golem_operation(compensation = compensation_step)]
fn transaction_step(step: u64) -> Result<bool, String> {
    println!("Step {step}");
    Ok(remote_call(step))
}

fn compensation_step(_: bool, step: u64) -> Result<(), String> {
    println!("Compensating step {step}");
    remote_call_undo(step);
    Ok(())
}

fn remote_call(param: u64) -> bool {
    wit_bindgen::block_on(remote_call_async(param))
}

async fn remote_call_async(param: u64) -> bool {
    let port = std::env::var("PORT").unwrap_or("9999".to_string());
    let path = format!("/step/{param}");
    println!("Sending GET {path}");
    let (status, body) =
        raw_http::request_async(Method::Get, &format!("localhost:{port}"), &path, None, None).await;
    let body: bool = serde_json::from_slice(&body).expect("Invalid response");
    println!("Received {status} {body}");
    body
}

fn remote_call_undo(param: u64) -> bool {
    let port = std::env::var("PORT").unwrap_or("9999".to_string());
    let path = format!("/step/{param}");
    println!("Sending DEL {path}");
    let (status, body) = raw_http::request(
        Method::Delete,
        &format!("localhost:{port}"),
        &path,
        None,
        None,
    );
    let body: bool = serde_json::from_slice(&body).expect("Invalid response");
    println!("Received {status} {body}");
    body
}
