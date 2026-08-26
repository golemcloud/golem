use crate::raw_http;
use crate::raw_http::Method;
use golem_rust::bindings::golem::agent::host::{Datetime, WasmRpc};
use golem_rust::bindings::golem::api::oplog::{GetOplog, OplogReadError, SearchOplog};
use golem_rust::bindings::golem::tool::host as tool_host;
use golem_rust::retry::{
    CountBoxConfig, NamedRetryPolicy, PolicyNode, PredicateNode, RetryPolicy, RetryPredicate,
    get_retry_policies, get_retry_policy_by_name, remove_retry_policy, set_retry_policy,
    use_retry_policy,
};
use golem_rust::schema::{IntoTypedSchemaValue, try_into_schema_graph};
use golem_rust::wasip3::http::{client, types};
use golem_rust::wasip3::{wit_future, wit_stream};
use golem_rust::{
    AgentAnyFilter, AgentId, AgentMetadata, Checkpoint, CheckpointResultExt, ComponentId,
    ForkResult, FromSchema, GetAgents, IntoSchema, PromiseId, SchemaValue, Transaction, UpdateMode,
    Uuid, agent_definition, agent_implementation, atomically, atomically_async,
    encode_schema_value, encode_typed_schema_value, fallible_transaction, fork,
    generate_idempotency_key, get_agent_metadata, get_oplog_index, get_self_metadata,
    golem_operation, infallible_transaction, oplog_commit, resolve_agent_id,
    resolve_agent_id_strict, resolve_component_id, set_oplog_index, update_agent,
    use_idempotence_mode,
};
use serde::{Deserialize, Serialize};
use std::future::poll_fn;
use std::pin::pin;
use std::task::Poll;

mod gated_host_bindings {
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

#[derive(Clone, IntoSchema, FromSchema, Serialize, Deserialize)]
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
    let (high_bits, low_bits) = agent_id.component_id.uuid.as_u64_pair();
    golem_rust::schema::wit::wire::AgentId {
        component_id: golem_rust::schema::wit::wire::ComponentId {
            uuid: golem_rust::schema::wit::wire::Uuid {
                high_bits,
                low_bits,
            },
        },
        agent_id: agent_id.agent_id,
    }
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
    fn fail_with_custom_max_retries(&self, max_retries: u64);
    fn explicit_commit(&self, replicas: u8);
    fn atomic_region(&self);
    async fn atomic_region_async(&self);
    fn idempotence_flag(&self, enabled: bool);
    async fn fallible_transaction_test(&self) -> u64;
    async fn infallible_transaction_test(&self) -> u64;
    fn fork_test(&self, input: String) -> String;

    fn checkpoint_test(&self) -> u64;
    async fn checkpoint_test_async(&self) -> u64;
    fn jump(&self) -> u64;
    fn get_workers(
        &self,
        component_id: ComponentId,
        filter: Option<AgentAnyFilter>,
        precise: bool,
    ) -> Vec<AgentMetadata>;
    fn get_agents_next_result(&self, component_id: ComponentId) -> Result<u64, String>;
    fn get_self_metadata_result(&self) -> Result<String, String>;
    fn resolve_agent_id_strict_result(
        &self,
        component_reference: String,
        agent_name: String,
    ) -> Result<bool, String>;
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
    fn read_own_oplog_result(&self) -> Result<(), String>;
    fn search_own_oplog_result(&self, query: String) -> Result<(), String>;
    fn read_oplog_result(&self, agent_id: AgentId) -> Result<(), String>;
    fn search_oplog_result(&self, agent_id: AgentId, query: String) -> Result<(), String>;
    fn get_config_value_result(&self, key: Vec<String>) -> Result<(), String>;
    fn create_webhook_result(&self, promise_id: PromiseId) -> Result<(), String>;
    fn get_agent_metadata_result(&self, agent_id: AgentId) -> Result<bool, String>;
    fn update_agent_result(&self, agent_id: AgentId, revision: u64) -> Result<(), String>;
    fn fork_agent_result(
        &self,
        source_agent_id: AgentId,
        target_agent_id: AgentId,
        oplog_index: u64,
    ) -> Result<(), String>;
    fn revert_agent_result(&self, agent_id: AgentId, oplog_index: u64) -> Result<(), String>;
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
}

pub struct GolemHostApiImpl {
    _name: String,
    recorded_tools: Option<Vec<ToolSummary>>,
}

fn summarize_tool(tool: tool_host::RegisteredTool) -> ToolSummary {
    let (name, summary, aliases) = tool
        .definition
        .commands
        .nodes
        .first()
        .map(|node| {
            (
                node.name.clone(),
                node.doc.summary.clone(),
                node.aliases.clone(),
            )
        })
        .unwrap_or_default();
    let uuid = tool.implemented_by.uuid;
    (
        name,
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
        }
    }

    fn resolve_component(&self) -> ResolveComponentResult {
        let component_id = resolve_component_id("component-resolve-target");
        let worker_id = resolve_agent_id("component-resolve-target", "counter-1");
        let strict_worker_id = resolve_agent_id_strict("component-resolve-target", "counter-2");
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

    fn explicit_commit(&self, replicas: u8) {
        let now = std::time::SystemTime::now();
        println!("Starting commit with {replicas} replicas at {now:?}");
        oplog_commit(replicas);
        println!("Finished commit");
    }

    fn atomic_region(&self) {
        let now = std::time::SystemTime::now();
        println!("Starting atomic region at {now:?}");

        atomically(|| {
            remote_side_effect("1");
            remote_side_effect("2");

            let decision = remote_call(1);
            if decision {
                panic!("crash 1");
            }

            remote_side_effect("3");
        });

        println!("Finished atomic region");

        remote_side_effect("4");

        atomically(|| {
            remote_side_effect("5");
            let decision = remote_call(2);
            if decision {
                panic!("crash 2");
            }
        });

        remote_side_effect("6");
    }

    async fn atomic_region_async(&self) {
        let now = std::time::SystemTime::now();
        println!("Starting atomic region at {now:?}");

        atomically_async(|| async {
            remote_side_effect("1");
            remote_side_effect("2");

            let decision = remote_call(1);
            if decision {
                panic!("crash 1");
            }

            remote_side_effect("3");
        })
        .await;

        println!("Finished atomic region");

        remote_side_effect("4");

        atomically_async(|| async {
            remote_side_effect("5");
            let decision = remote_call(2);
            if decision {
                panic!("crash 2");
            }
        })
        .await;

        remote_side_effect("6");
    }

    fn idempotence_flag(&self, enabled: bool) {
        let _guard = use_idempotence_mode(enabled);

        let port = std::env::var("PORT").unwrap_or("9999".to_string());

        wit_bindgen::block_on(async move {
            // The side-effect POST is kept open across the crash below: the
            // test server records the event as soon as the request arrives but
            // delays its first response (per the
            // `x-test-first-response-delay-ms` header), so the durable send is
            // still incomplete (`Start` without `End`) when the worker
            // crashes. On the retry the idempotence flag decides whether the
            // incomplete remote write may be re-executed (idempotence on) or
            // must fail the invocation (idempotence off).
            let headers = types::Fields::from_list(&[(
                "x-test-first-response-delay-ms".to_string(),
                b"30000".to_vec(),
            )])
            .unwrap();
            let (mut body_writer, body_reader) = wit_stream::new();
            let (trailers_writer, trailers_reader) =
                wit_future::new::<Result<Option<types::Fields>, types::ErrorCode>>(|| Ok(None));
            let (request, _transmitted) =
                types::Request::new(headers, Some(body_reader), trailers_reader, None);
            request.set_method(&types::Method::Post).unwrap();
            request.set_path_with_query(Some("/side-effect")).unwrap();
            request.set_scheme(Some(&types::Scheme::Http)).unwrap();
            request
                .set_authority(Some(&format!("localhost:{port}")))
                .unwrap();

            let mut send = pin!(client::send(request));
            let mut send_result = None;

            // Drive the send only until the request body and trailers have been
            // fully handed over to the host's HTTP client: at that point the
            // server has received the request (and recorded the side effect),
            // while the send itself — and with it the durable `End` — is still
            // pending on the delayed response.
            {
                let mut write_body = pin!(async {
                    let leftover = body_writer.write_all(b"1".to_vec()).await;
                    assert!(leftover.is_empty());
                    drop(body_writer);
                    let _ = trailers_writer.write(Ok(None)).await;
                });
                poll_fn(|cx| {
                    if send_result.is_none()
                        && let Poll::Ready(result) = send.as_mut().poll(cx)
                    {
                        send_result = Some(result);
                    }
                    write_body.as_mut().poll(cx)
                })
                .await;
            }

            // Race the pending send against a short timer. The test server
            // delays only the *first* `/side-effect` response — by far longer
            // than this timer — so on the first attempt the timer wins and the
            // send is still incomplete when the worker crashes below. When the
            // incomplete send is re-executed on a retry (idempotence on), the
            // server responds immediately and the send wins the race, so the
            // retry completes without crashing. The timer is a `ReadLocal`
            // durable call, which — unlike a remote call — may interleave with
            // the open send scope without making the incomplete send
            // unreplayable.
            if send_result.is_none() {
                let mut timer = pin!(golem_rust::wasip3::clocks::monotonic_clock::wait_for(
                    2_000_000_000
                ));
                poll_fn(|cx| {
                    if let Poll::Ready(result) = send.as_mut().poll(cx) {
                        send_result = Some(result);
                        return Poll::Ready(());
                    }
                    timer.as_mut().poll(cx)
                })
                .await;
            }

            // The crash happens inside an atomic region: a guest panic is a
            // deterministic trap that is only retried when it is inside an
            // atomic region.
            let send_ready = send_result.is_some();
            atomically_async(|| async move {
                if !send_ready {
                    panic!("crash 1");
                }
            })
            .await;

            let response = match send_result {
                Some(result) => result,
                None => send.await,
            }
            .expect("HTTP request failed");
            let status = response.get_status_code();
            let (result_writer, result_reader) =
                wit_future::new::<Result<(), types::ErrorCode>>(|| Ok(()));
            let (body_stream, trailers) = types::Response::consume_body(response, result_reader);
            let body = body_stream.collect().await;
            if let Err(err) = trailers.await {
                panic!("Error: {err:?}")
            }
            result_writer
                .write(Ok(()))
                .await
                .expect("failed to acknowledge response body");

            println!(
                "Received response from remote side-effect: {} {}",
                status,
                String::from_utf8(body).unwrap()
            );
        });
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
                let _ = tx.transaction_step(1).await;
                let _ = tx.transaction_step(2).await;
                if tx.transaction_step(3).await.unwrap() {
                    panic!("crash after 3");
                }
                let _ = tx.transaction_step(4).await;
                11
            })
        })
        .await
    }

    fn fork_test(&self, input: String) -> String {
        let port = std::env::var("PORT").unwrap_or("9999".to_string());
        let authority = format!("localhost:{port}");
        let self_name = get_self_metadata()
            .expect("self metadata access should be allowed")
            .agent_id
            .agent_id;

        let path = format!("/fork-test/step1/{self_name}/{input}");
        println!("Sending GET {path}");

        let (_status, body) = raw_http::request(Method::Get, &authority, &path, None, None);
        let part1_raw = String::from_utf8(body).expect("Invalid response");
        println!("Received {part1_raw}");

        let part1: String = serde_json::from_str(&part1_raw).unwrap();

        let path = match fork().expect("self fork should be allowed") {
            ForkResult::Original(details) => {
                let uuid: golem_rust::Uuid = Into::into(details.forked_phantom_id);
                format!("/fork-test/step2/{self_name}/original/{uuid}")
            }
            ForkResult::Forked(details) => {
                let self_name = get_self_metadata()
                    .expect("self metadata access should be allowed")
                    .agent_id
                    .agent_id;
                let uuid: golem_rust::Uuid = Into::into(details.forked_phantom_id);
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

    fn checkpoint_test(&self) -> u64 {
        let mut state = 0;
        println!("started: {state}");
        state += 1;
        let cp = Checkpoint::new();
        state += 1;
        println!("second: {state}");
        let call_result: Result<(), String> = if remote_call(state) {
            Err("reverting".to_string())
        } else {
            Ok(())
        };
        call_result.unwrap_or_revert(&cp);
        state += 1;
        println!("third: {state}");
        cp.assert_or_revert(!remote_call(state));
        state += 1;
        println!("fourth: {state}");
        state
    }

    async fn checkpoint_test_async(&self) -> u64 {
        let mut state = 0u64;
        println!("started: {state}");
        state += 1;
        let cp = Checkpoint::new();
        state += 1;
        println!("second: {state}");
        cp.run_or_revert_async(|| async {
            if remote_call(state) {
                Err::<(), String>("reverting".to_string())
            } else {
                Ok(())
            }
        })
        .await;
        state += 1;
        println!("third: {state}");
        cp.assert_or_revert(!remote_call(state));
        state += 1;
        println!("fourth: {state}");
        state
    }

    fn jump(&self) -> u64 {
        let mut state = 0;
        println!("started: {state}");
        state += 1;
        let state1 = get_oplog_index();
        state += 1;
        println!("second: {state}");
        if remote_call(state) {
            set_oplog_index(state1);
        }
        state += 1;
        println!("third: {state}");
        let state2 = get_oplog_index();
        state += 1;
        println!("fourth: {state}");
        if remote_call(state) {
            set_oplog_index(state2);
        }
        state += 1;
        println!("fifth: {state}");
        state
    }

    fn get_workers(
        &self,
        component_id: ComponentId,
        filter: Option<AgentAnyFilter>,
        precise: bool,
    ) -> Vec<AgentMetadata> {
        let mut workers: Vec<AgentMetadata> = Vec::new();
        let getter = GetAgents::new(component_id, filter.as_ref(), precise);
        while let Some(values) = getter
            .get_next()
            .expect("agent enumeration should be allowed")
        {
            workers.extend(values);
        }
        workers
    }

    fn get_agents_next_result(&self, component_id: ComponentId) -> Result<u64, String> {
        let (high_bits, low_bits) = component_id.uuid.as_u64_pair();
        let getter = host_api::GetAgents::new(
            golem_rust::schema::wit::wire::ComponentId {
                uuid: golem_rust::schema::wit::wire::Uuid {
                    high_bits,
                    low_bits,
                },
            },
            None,
            false,
        );
        getter
            .get_next()
            .map(|agents| agents.map_or(0, |agents| agents.len() as u64))
            .map_err(|error| format!("{error:?}"))
    }

    fn get_self_metadata_result(&self) -> Result<String, String> {
        host_api::get_self_metadata()
            .map(|metadata| metadata.agent_id.agent_id)
            .map_err(|error| format!("{error:?}"))
    }

    fn resolve_agent_id_strict_result(
        &self,
        component_reference: String,
        agent_name: String,
    ) -> Result<bool, String> {
        Ok(host_api::resolve_agent_id_strict(&component_reference, &agent_name).is_some())
    }

    fn self_fork_result(&self) -> Result<String, String> {
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
        let rpc = WasmRpc::create(
            &agent_type,
            encode_parameters(vec![SchemaValue::String(constructor)])?,
            None,
            Vec::new(),
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
        let rpc = WasmRpc::create(
            &agent_type,
            encode_parameters(vec![SchemaValue::String(constructor)])?,
            None,
            Vec::new(),
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
        let rpc = WasmRpc::create(
            &agent_type,
            encode_parameters(vec![SchemaValue::String(constructor)])?,
            None,
            Vec::new(),
        )
        .map_err(|error| format!("{error:?}"))?;
        rpc.invoke_and_await(&method, encode_parameters(Vec::new())?, None)
            .map(|_| ())
            .map_err(|error| format!("{error:?}"))
    }

    fn outbound_agent_rpc_schedule_result(
        &self,
        agent_type: String,
        constructor: String,
        method: String,
    ) -> Result<(), String> {
        let rpc = WasmRpc::create(
            &agent_type,
            encode_parameters(vec![SchemaValue::String(constructor)])?,
            None,
            Vec::new(),
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
        let rpc = WasmRpc::create(
            &agent_type,
            encode_parameters(vec![SchemaValue::String(constructor)])?,
            None,
            Vec::new(),
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

    fn read_own_oplog_result(&self) -> Result<(), String> {
        let metadata = get_self_metadata().map_err(|error| format!("{error:?}"))?;
        let agent_id = wire_agent_id(metadata.agent_id);
        GetOplog::new(&agent_id, 0)
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
        SearchOplog::new(&agent_id, &query)
            .get_next()
            .map(|_| ())
            .map_err(|error| format!("{error:?}"))
    }

    fn read_oplog_result(&self, agent_id: AgentId) -> Result<(), String> {
        GetOplog::new(&wire_agent_id(agent_id), 0)
            .get_next()
            .map(|_| ())
            .map_err(|error| format!("{error:?}"))
    }

    fn search_oplog_result(&self, agent_id: AgentId, query: String) -> Result<(), String> {
        SearchOplog::new(&wire_agent_id(agent_id), &query)
            .get_next()
            .map(|_| ())
            .map_err(|error| format!("{error:?}"))
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
        agent_host::create_webhook(&(&promise_id).into())
            .map(|_| ())
            .map_err(|error| format!("{error:?}"))
    }

    fn get_agent_metadata_result(&self, agent_id: AgentId) -> Result<bool, String> {
        Ok(host_api::get_agent_metadata(&wire_agent_id(agent_id)).is_some())
    }

    fn update_agent_result(&self, agent_id: AgentId, revision: u64) -> Result<(), String> {
        host_api::update_agent(
            &wire_agent_id(agent_id),
            revision,
            host_api::UpdateMode::Automatic,
        )
        .map_err(|error| format!("{error:?}"))
    }

    fn fork_agent_result(
        &self,
        source_agent_id: AgentId,
        target_agent_id: AgentId,
        oplog_index: u64,
    ) -> Result<(), String> {
        host_api::fork_agent(
            &wire_agent_id(source_agent_id),
            &wire_agent_id(target_agent_id),
            oplog_index,
        )
        .map_err(|error| format!("{error:?}"))
    }

    fn revert_agent_result(&self, agent_id: AgentId, oplog_index: u64) -> Result<(), String> {
        host_api::revert_agent(
            &wire_agent_id(agent_id),
            host_api::RevertAgentTarget::RevertToOplogIndex(oplog_index),
        )
        .map_err(|error| format!("{error:?}"))
    }

    async fn tool_rpc_invoke_result(
        &self,
        tool_name: String,
        command_path: Vec<String>,
        input: String,
    ) -> Result<(), String> {
        tool_host::ToolRpc::new(&tool_name)
            .invoke(command_path, encode_tool_input(input)?, None)
            .await
            .map(|_| ())
            .map_err(|error| format!("{error:?}"))
    }

    async fn tool_rpc_async_invoke_and_await_result(
        &self,
        tool_name: String,
        command_path: Vec<String>,
        input: String,
    ) -> Result<(), String> {
        tool_host::ToolRpc::new(&tool_name)
            .async_invoke_and_await(command_path, encode_tool_input(input)?, None)
            .await
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
        tool_host::ToolRpc::new(&tool_name)
            .invoke_and_await(command_path, encode_tool_input(input)?, None)
            .await
            .map(|_| ())
            .map_err(|error| format!("{error:?}"))
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

fn remote_side_effect(message: &str) {
    let port = std::env::var("PORT").unwrap_or("9999".to_string());
    println!("Sending POST /side-effect");
    let (status, _body) = raw_http::request(
        Method::Post,
        &format!("localhost:{port}"),
        "/side-effect",
        Some(message.as_bytes()),
        None,
    );
    println!("Received {status}");
}
