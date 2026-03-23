use golem_rust::bindings::golem::api::host::{
    get_agent_metadata, get_oplog_index, get_self_metadata, resolve_agent_id,
    resolve_agent_id_strict, resolve_component_id, set_oplog_index, update_agent,
    AgentAnyFilter, AgentMetadata, ComponentId as HostComponentId, GetAgents, UpdateMode,
};
use golem_rust::{
    agent_definition, agent_implementation, generate_idempotency_key, golem_operation, Schema,
    atomically, get_promise, fork, oplog_commit,
    fallible_transaction, infallible_transaction,
    use_retry_policy, RetryPolicy, use_idempotence_mode, use_persistence_level,
    with_persistence_level, PersistenceLevel,
    ForkResult, PromiseId, Transaction, Uuid,
};
use golem_wasi_http::{Client, Response};
use serde::{Deserialize, Serialize};

#[derive(Clone, Schema, Serialize, Deserialize)]
pub struct ResolveComponentResult {
    pub component_found: bool,
    pub worker_found: bool,
    pub strict_worker_found: bool,
}

#[agent_definition]
pub trait GolemHostApi {
    fn new(name: String) -> Self;

    fn resolve_component(&self) -> ResolveComponentResult;

    fn create_promise(&self) -> PromiseId;
    fn await_promise(&self, promise_id: PromiseId) -> Vec<u8>;
    fn poll_promise(&self, promise_id: PromiseId) -> Option<Vec<u8>>;

    fn fail_with_custom_max_retries(&self, max_retries: u64);
    fn explicit_commit(&self, replicas: u8);
    fn atomic_region(&self);
    fn idempotence_flag(&self, enabled: bool);
    fn persist_nothing(&self);
    fn fallible_transaction_test(&self) -> u64;
    fn infallible_transaction_test(&self) -> u64;
    fn fork_test(&self, input: String) -> String;

    fn jump(&self) -> u64;
    fn get_workers(&self, component_id: HostComponentId, filter: Option<AgentAnyFilter>, precise: bool) -> Vec<AgentMetadata>;
    fn get_self_uri(&self) -> AgentMetadata;
    fn get_worker_metadata(&self, agent_id: golem_rust::golem_wasm::AgentId) -> Option<AgentMetadata>;
    fn update_worker(&self, agent_id: golem_rust::golem_wasm::AgentId, component_revision: u64, update_mode: UpdateMode);
    fn generate_idempotency_keys(&self) -> (Uuid, Uuid);
}

pub struct GolemHostApiImpl {
    _name: String,
}

#[agent_implementation]
impl GolemHostApi for GolemHostApiImpl {
    fn new(name: String) -> Self {
        Self { _name: name }
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

    fn poll_promise(&self, promise_id: PromiseId) -> Option<Vec<u8>> {
        get_promise(&promise_id).get()
    }

    fn fail_with_custom_max_retries(&self, max_retries: u64) {
        let _guard = use_retry_policy(RetryPolicy {
            max_attempts: max_retries as u32,
            min_delay: std::time::Duration::from_secs(1),
            max_delay: std::time::Duration::from_secs(1),
            multiplier: 1.0,
            max_jitter_factor: None,
        });

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

    fn idempotence_flag(&self, enabled: bool) {
        let _guard = use_idempotence_mode(enabled);

        let future_response = send_remote_side_effect_raw("1");

        atomically(|| {
            let _guard = use_persistence_level(PersistenceLevel::PersistNothing);
            let decision = remote_call(1);
            if decision {
                panic!("crash 1");
            }
        });

        let incoming_response = get_incoming_response_raw(&future_response);
        let body = read_response_body_raw(&incoming_response);

        println!(
            "Received response from remote side-effect: {} {}",
            incoming_response.status(),
            String::from_utf8(body).unwrap()
        );
    }

    fn persist_nothing(&self) {
        remote_side_effect("1");

        with_persistence_level(PersistenceLevel::PersistNothing, || {
            remote_side_effect("2");
        });

        remote_side_effect("3");

        atomically(|| {
            let decision = remote_call(1);
            if decision {
                panic!("crash 1");
            }
        });

        remote_side_effect("4");
    }

    fn fallible_transaction_test(&self) -> u64 {
        fallible_transaction(|tx| {
            tx.transaction_step(1)?;
            tx.transaction_step(2)?;
            if tx.transaction_step(3)? {
                tx.fail("fail after 3".to_string())?
            }
            tx.transaction_step(4)?;
            Ok(11)
        })
        .expect("Transaction failed")
    }

    fn infallible_transaction_test(&self) -> u64 {
        infallible_transaction(|tx| {
            let _ = tx.transaction_step(1);
            let _ = tx.transaction_step(2);
            if tx.transaction_step(3).unwrap() {
                panic!("crash after 3");
            }
            let _ = tx.transaction_step(4);
            11
        })
    }

    fn fork_test(&self, input: String) -> String {
        let port = std::env::var("PORT").unwrap_or("9999".to_string());
        let self_name = get_self_metadata().agent_id.agent_id;
        let client = Client::builder().build().unwrap();

        let url = format!("http://localhost:{port}/fork-test/step1/{self_name}/{input}");
        println!("Sending GET {url}");

        let response: Response = client.get(&url).send().expect("Request failed");
        let part1_raw = response.text().expect("Invalid response");
        println!("Received {part1_raw}");

        let part1: String = serde_json::from_str(&part1_raw).unwrap();

        let url = match fork() {
            ForkResult::Original(details) => {
                let uuid: golem_rust::Uuid = Into::into(details.forked_phantom_id);
                format!("http://localhost:{port}/fork-test/step2/{self_name}/original/{uuid}")
            }
            ForkResult::Forked(details) => {
                let self_name = get_self_metadata().agent_id.agent_id;
                let uuid: golem_rust::Uuid = Into::into(details.forked_phantom_id);
                format!("http://localhost:{port}/fork-test/step2/{self_name}/forked/{uuid}")
            }
        };

        println!("Trying to call {url}");
        let response2: Response = client.get(&url).send().expect("Request failed");
        let part2_raw = response2.text().expect("Invalid response");
        println!("Received {part2_raw}");

        let part2: String = serde_json::from_str(&part2_raw).unwrap();

        format!("{part1}::{part2}")
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
        component_id: HostComponentId,
        filter: Option<AgentAnyFilter>,
        precise: bool,
    ) -> Vec<AgentMetadata> {
        let mut workers: Vec<AgentMetadata> = Vec::new();
        let getter = GetAgents::new(component_id, filter.as_ref(), precise);
        loop {
            match getter.get_next() {
                Some(values) => {
                    workers.extend(values);
                }
                None => break,
            }
        }
        workers
    }

    fn get_self_uri(&self) -> AgentMetadata {
        get_self_metadata()
    }

    fn get_worker_metadata(&self, agent_id: golem_rust::golem_wasm::AgentId) -> Option<AgentMetadata> {
        get_agent_metadata(&agent_id)
    }

    fn update_worker(
        &self,
        agent_id: golem_rust::golem_wasm::AgentId,
        component_revision: u64,
        update_mode: UpdateMode,
    ) {
        update_agent(&agent_id, component_revision, update_mode);
    }

    fn generate_idempotency_keys(&self) -> (Uuid, Uuid) {
        let key1 = generate_idempotency_key();
        let key2 = generate_idempotency_key();
        (key1, key2)
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
    let port = std::env::var("PORT").unwrap_or("9999".to_string());
    let client = Client::builder().build().unwrap();
    let url = format!("http://localhost:{port}/step/{param}");
    println!("Sending GET {url}");
    let response: Response = client.get(&url).send().expect("Request failed");
    let status = response.status();
    let body = response.json::<bool>().expect("Invalid response");
    println!("Received {status} {body}");
    body
}

fn remote_call_undo(param: u64) -> bool {
    let port = std::env::var("PORT").unwrap_or("9999".to_string());
    let client = Client::builder().build().unwrap();
    let url = format!("http://localhost:{port}/step/{param}");
    println!("Sending DEL {url}");
    let response: Response = client.delete(&url).send().expect("Request failed");
    let status = response.status();
    let body = response.json::<bool>().expect("Invalid response");
    println!("Received {status} {body}");
    body
}

fn remote_side_effect(message: &str) {
    let port = std::env::var("PORT").unwrap_or("9999".to_string());
    let client = Client::builder().build().unwrap();
    let url = format!("http://localhost:{port}/side-effect");
    println!("Sending POST {url}");
    let response: Response = client
        .post(&url)
        .body(message.to_string())
        .send()
        .expect("Request failed");
    let status = response.status();
    println!("Received {status}");
}

fn send_remote_side_effect_raw(
    message: &str,
) -> wasi::http::types::FutureIncomingResponse {
    let port = std::env::var("PORT").unwrap_or("9999".to_string());

    let headers = wasi::http::types::Fields::new();
    let request = wasi::http::types::OutgoingRequest::new(headers);
    request
        .set_method(&wasi::http::types::Method::Post)
        .unwrap();
    request.set_path_with_query(Some("/side-effect")).unwrap();
    request
        .set_scheme(Some(&wasi::http::types::Scheme::Http))
        .unwrap();
    request
        .set_authority(Some(&format!("localhost:{port}")))
        .unwrap();

    let request_body = request.body().unwrap();
    let request_body_stream = request_body.write().unwrap();
    request_body_stream.write(message.as_bytes()).unwrap();
    drop(request_body_stream);
    wasi::http::types::OutgoingBody::finish(request_body, None).unwrap();

    let options = wasi::http::types::RequestOptions::new();
    options.set_connect_timeout(Some(5000000000)).unwrap();
    options.set_first_byte_timeout(Some(5000000000)).unwrap();
    options
        .set_between_bytes_timeout(Some(5000000000))
        .unwrap();

    wasi::http::outgoing_handler::handle(request, Some(options)).unwrap()
}

fn get_incoming_response_raw(
    future_incoming_response: &wasi::http::types::FutureIncomingResponse,
) -> wasi::http::types::IncomingResponse {
    match future_incoming_response.get() {
        Some(Ok(Ok(incoming_response))) => {
            println!("Got incoming response");
            incoming_response
        }
        Some(Ok(Err(err))) => {
            println!("Returned with error code: {err:?}");
            panic!("Error: {:?}", err)
        }
        Some(Err(err)) => {
            println!("Returned with error: {err:?}");
            panic!("Error: {:?}", err)
        }
        None => {
            println!("No incoming response yet, polling");
            let pollable = future_incoming_response.subscribe();
            let _ = wasi::io::poll::poll(&[&pollable]);
            get_incoming_response_raw(future_incoming_response)
        }
    }
}

fn read_response_body_raw(
    incoming_response: &wasi::http::types::IncomingResponse,
) -> Vec<u8> {
    let response_body = incoming_response.consume().unwrap();
    let response_body_stream = response_body.stream().unwrap();
    let mut body = Vec::new();

    let mut eof = false;
    while !eof {
        match response_body_stream.read(u64::MAX) {
            Ok(mut body_chunk) => {
                body.append(&mut body_chunk);
            }
            Err(wasi::io::streams::StreamError::Closed) => {
                eof = true;
            }
            Err(err) => panic!("Error: {:?}", err),
        }
    }
    body
}
