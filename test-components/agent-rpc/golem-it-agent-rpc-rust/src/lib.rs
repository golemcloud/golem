use bytes::Bytes;
use golem_rust::agentic::{AgentStream, spawn_local};
use golem_rust::bindings::golem::agent::host::{Datetime, RpcError, WasmRpc};
use golem_rust::{
    FromSchema, IntoSchema, PromiseId, SchemaValue, Uuid, agent_definition, agent_implementation,
    encode_schema_value,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn encode_single_parameter<T: IntoSchema>(
    value: T,
) -> golem_rust::schema::wit::wire::SchemaValueTree {
    encode_schema_value(&SchemaValue::Record {
        fields: vec![value.to_value()],
    })
    .expect("failed to encode RPC parameter")
}

#[derive(Debug, Clone, IntoSchema, FromSchema)]
pub enum State {
    Initial,
    Ongoing,
}

#[derive(Debug, Clone, IntoSchema, FromSchema)]
pub struct Payload {
    pub field1: String,
    pub field2: Uuid,
    pub field3: State,
}

#[agent_definition]
pub trait RustParent {
    fn new(name: String) -> Self;

    async fn spawn_child(&self, data: String) -> Uuid;
    async fn call_ts_agent(&self, name: String) -> f64;
    fn inspect_missing_rpc_type(&self) -> String;
}

struct RustParentImpl {
    _name: String,
}

#[agent_implementation]
impl RustParent for RustParentImpl {
    fn new(name: String) -> Self {
        Self { _name: name }
    }

    async fn spawn_child(&self, data: String) -> Uuid {
        let uuid = Uuid::new_v4();
        let payload = Payload {
            field1: data,
            field2: uuid,
            field3: State::Initial,
        };
        let mut child = RustChildClient::get_(uuid.clone());
        child.set(payload).await;
        uuid
    }

    async fn call_ts_agent(&self, name: String) -> f64 {
        let client = SimpleChildAgentClient::get(name);
        client.value().await
    }

    fn inspect_missing_rpc_type(&self) -> String {
        let constructor = encode_schema_value(&SchemaValue::Record { fields: Vec::new() })
            .expect("failed to encode empty RPC constructor");
        match WasmRpc::create("MissingReflectedType", constructor, None, Vec::new()) {
            Ok(_) => "unexpected success".to_string(),
            Err(error) => format!("{error:?}"),
        }
    }
}

#[agent_definition]
pub trait RustChild {
    fn new(id: Uuid) -> Self;
    fn set(&mut self, payload: Payload);
    fn get(&self) -> Option<Payload>;
}

struct RustChildImpl {
    _id: Uuid,
    payload: Option<Payload>,
}

#[agent_implementation]
impl RustChild for RustChildImpl {
    fn new(id: Uuid) -> Self {
        Self {
            _id: id,
            payload: None,
        }
    }

    fn set(&mut self, payload: Payload) {
        self.payload = Some(payload);
    }

    fn get(&self) -> Option<Payload> {
        self.payload.clone()
    }
}

#[agent_definition]
pub trait SimpleChildAgent {
    fn new(name: String) -> Self;
    fn value(&self) -> f64;
}
// implemented in `golem-it-agent-rpc`

#[agent_definition]
pub trait Counter {
    fn new(id: String) -> Self;
    fn get_value(&self) -> String;
}

struct CounterImpl {
    id: String,
}

#[agent_implementation]
impl Counter for CounterImpl {
    fn new(id: String) -> Self {
        Self { id }
    }

    fn get_value(&self) -> String {
        format!("counter-{}", self.id)
    }
}

// -- Scheduled invocation agents --

fn datetime_200ms_from_now() -> Datetime {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards");
    let scheduled = now + Duration::from_millis(200);
    Datetime {
        seconds: scheduled.as_secs() as i64,
        nanoseconds: scheduled.subsec_nanos(),
    }
}

#[agent_definition]
pub trait ScheduledInvocationServer {
    fn new(name: String) -> Self;
    fn inc_global_by(&mut self, value: u64);
    fn get_global_value(&self) -> u64;
}

struct ScheduledInvocationServerImpl {
    _name: String,
    global: u64,
}

#[agent_implementation]
impl ScheduledInvocationServer for ScheduledInvocationServerImpl {
    fn new(name: String) -> Self {
        Self {
            _name: name,
            global: 0,
        }
    }

    fn inc_global_by(&mut self, value: u64) {
        self.global += value;
    }

    fn get_global_value(&self) -> u64 {
        self.global
    }
}

#[agent_definition]
pub trait ScheduledInvocationClient {
    fn new(name: String) -> Self;

    /// Schedule inc_global_by on the server agent 200ms in the future
    fn test1(&self, server_agent_name: String);

    /// Schedule inc_global_by on the server agent 200ms in the future, then cancel it
    fn test2(&self, server_agent_name: String);

    /// Schedule inc_global_by on self 200ms in the future
    fn test3(&mut self);

    fn inc_global_by(&mut self, value: u64);
    fn get_global_value(&self) -> u64;
}

struct ScheduledInvocationClientImpl {
    _name: String,
    global: u64,
}

#[agent_implementation]
impl ScheduledInvocationClient for ScheduledInvocationClientImpl {
    fn new(name: String) -> Self {
        Self {
            _name: name,
            global: 0,
        }
    }

    fn test1(&self, server_agent_name: String) {
        let mut server = ScheduledInvocationServerClient::get(server_agent_name);
        let scheduled_for = datetime_200ms_from_now();
        server
            .schedule_inc_global_by(1, scheduled_for)
            .expect("failed to schedule invocation");
    }

    fn test2(&self, server_agent_name: String) {
        let mut server = ScheduledInvocationServerClient::get(server_agent_name);
        let scheduled_for = datetime_200ms_from_now();
        let token = server
            .schedule_cancelable_inc_global_by(1, scheduled_for)
            .expect("failed to schedule cancelable invocation");
        token.cancel();
    }

    fn test3(&mut self) {
        let mut self_client = ScheduledInvocationClientClient::get(self._name.clone());
        let scheduled_for = datetime_200ms_from_now();
        self_client
            .schedule_inc_global_by(1, scheduled_for)
            .expect("failed to schedule invocation");
    }

    fn inc_global_by(&mut self, value: u64) {
        self.global += value;
    }

    fn get_global_value(&self) -> u64 {
        self.global
    }
}

fn agent_stream<T: IntoSchema + 'static>(values: Vec<T>) -> AgentStream<T> {
    let (mut writer, stream) = AgentStream::new();
    spawn_local(async move {
        let _ = writer.write_all(values).await;
    });
    stream
}

fn agent_error_stream() -> AgentStream<u32> {
    let (mut writer, output) = golem_rust::schema::wit::new_schema_value_stream();
    spawn_local(async move {
        let first = encode_schema_value(&SchemaValue::U32(1))
            .expect("failed to encode value before producer error");
        if writer.write_one(first).await.is_some() {
            return;
        }
        let _ = writer
            .write_one(golem_rust::schema::wit::wire::SchemaValueTree {
                value_nodes: Vec::new(),
                root: 0,
            })
            .await;
    });
    AgentStream::from_raw(output)
}

#[derive(IntoSchema, FromSchema)]
pub struct NestedStreamInput {
    pub labels: AgentStream<String>,
    pub values: Option<AgentStream<u32>>,
}

#[derive(IntoSchema, FromSchema)]
pub struct NestedStreamItem {
    pub label: String,
    pub values: AgentStream<u32>,
}

#[derive(Debug, Clone, IntoSchema, FromSchema)]
pub struct StreamingRpcReport {
    pub input_only: Vec<u32>,
    pub output_only: Vec<u32>,
    pub simultaneous: Vec<u32>,
    pub nested_labels: Vec<String>,
    pub nested_values: Vec<u32>,
    pub nested_item_labels: Vec<String>,
    pub nested_item_values: Vec<Vec<u32>>,
    pub first_sibling: Vec<String>,
    pub second_sibling: Vec<u32>,
    pub after_consumer_drop: u64,
}

#[agent_definition]
pub trait StreamingRpcTarget {
    fn new(name: String) -> Self;

    async fn consume(&self, input: AgentStream<u32>) -> Vec<u32>;
    async fn consume_strings(&self, input: AgentStream<String>) -> Vec<String>;
    async fn drop_input(&self, input: AgentStream<u32>) -> u64;
    async fn hold_input(&self, input: AgentStream<u32>) -> u64;
    fn produce(&self, values: Vec<u32>) -> AgentStream<u32>;
    fn transform(&self, input: AgentStream<u32>) -> AgentStream<u32>;
    async fn consume_bytes(&self, input: AgentStream<u8>) -> Vec<u8>;
    fn produce_bytes(&self, values: Vec<u8>) -> AgentStream<u8>;
    fn produce_byte_then_wait(&self) -> AgentStream<u8>;
    fn produce_many_bytes(&self, count: u32) -> AgentStream<u8>;
    fn transform_bytes(&self, input: AgentStream<u8>) -> AgentStream<u8>;
    fn transform_binary(&self, input: AgentStream<Bytes>) -> AgentStream<Bytes>;
    async fn consume_binary_chunks(&self, input: AgentStream<Bytes>) -> Vec<u64>;
    async fn consume_nested(&self, input: NestedStreamInput) -> (Vec<String>, Vec<u32>);
    fn produce_scalar_and_stream(&self) -> (String, AgentStream<u32>);
    fn produce_nested_items(&self) -> AgentStream<NestedStreamItem>;
    fn produce_nested_siblings(
        &self,
    ) -> (AgentStream<NestedStreamItem>, AgentStream<NestedStreamItem>);
    fn produce_siblings(&self) -> (AgentStream<String>, AgentStream<u32>);
    fn produce_sibling_error(&self) -> (AgentStream<u32>, AgentStream<u32>);
    fn produce_error(&self) -> AgentStream<u32>;
    fn ping(&self) -> u64;
    fn increment_scalar(&mut self) -> u64;
    fn noop(&self);
}

struct StreamingRpcTargetImpl {
    _name: String,
    scalar: u64,
}

#[agent_implementation]
impl StreamingRpcTarget for StreamingRpcTargetImpl {
    fn new(name: String) -> Self {
        Self {
            _name: name,
            scalar: 0,
        }
    }

    async fn consume(&self, input: AgentStream<u32>) -> Vec<u32> {
        input
            .collect()
            .await
            .expect("failed to consume input stream")
    }

    async fn consume_strings(&self, input: AgentStream<String>) -> Vec<String> {
        input
            .collect()
            .await
            .expect("failed to consume string input stream")
    }

    async fn drop_input(&self, input: AgentStream<u32>) -> u64 {
        drop(input);
        42
    }

    async fn hold_input(&self, input: AgentStream<u32>) -> u64 {
        let _input = input;
        std::future::pending().await
    }

    fn produce(&self, values: Vec<u32>) -> AgentStream<u32> {
        agent_stream(values)
    }

    fn transform(&self, mut input: AgentStream<u32>) -> AgentStream<u32> {
        let (mut writer, output) = AgentStream::new();
        spawn_local(async move {
            while let Some(value) = input
                .next()
                .await
                .expect("failed to consume transform input")
            {
                writer
                    .write_one(value * 10)
                    .await
                    .expect("failed to write transformed value");
            }
        });
        output
    }

    async fn consume_bytes(&self, input: AgentStream<u8>) -> Vec<u8> {
        input
            .collect()
            .await
            .expect("failed to consume byte input stream")
    }

    fn produce_bytes(&self, values: Vec<u8>) -> AgentStream<u8> {
        agent_stream(values)
    }

    fn produce_byte_then_wait(&self) -> AgentStream<u8> {
        let (mut writer, output) = AgentStream::new();
        spawn_local(async move {
            writer
                .write_one(1)
                .await
                .expect("failed to write byte before waiting");
            std::future::pending::<()>().await;
        });
        output
    }

    fn produce_many_bytes(&self, count: u32) -> AgentStream<u8> {
        agent_stream((0..count).map(|value| value as u8).collect())
    }

    fn transform_bytes(&self, mut input: AgentStream<u8>) -> AgentStream<u8> {
        let (mut writer, output) = AgentStream::new();
        spawn_local(async move {
            while let Some(value) = input
                .next()
                .await
                .expect("failed to consume byte transform input")
            {
                writer
                    .write_one(value)
                    .await
                    .expect("failed to write transformed byte");
            }
        });
        output
    }

    fn transform_binary(&self, mut input: AgentStream<Bytes>) -> AgentStream<Bytes> {
        let (mut writer, output) = AgentStream::new();
        spawn_local(async move {
            while let Some(value) = input
                .next()
                .await
                .expect("failed to consume binary transform input")
            {
                writer
                    .write_one(value)
                    .await
                    .expect("failed to write transformed binary value");
            }
        });
        output
    }

    async fn consume_binary_chunks(&self, input: AgentStream<Bytes>) -> Vec<u64> {
        input
            .collect()
            .await
            .expect("failed to consume binary input")
            .into_iter()
            .map(|chunk| chunk.len() as u64)
            .collect()
    }

    async fn consume_nested(&self, input: NestedStreamInput) -> (Vec<String>, Vec<u32>) {
        let labels = input
            .labels
            .collect()
            .await
            .expect("failed to consume nested labels");
        let values = match input.values {
            Some(values) => values
                .collect()
                .await
                .expect("failed to consume nested values"),
            None => Vec::new(),
        };
        (labels, values)
    }

    fn produce_scalar_and_stream(&self) -> (String, AgentStream<u32>) {
        ("metadata".to_string(), agent_stream(vec![11, 12]))
    }

    fn produce_nested_items(&self) -> AgentStream<NestedStreamItem> {
        let (mut writer, output) = AgentStream::new();
        spawn_local(async move {
            writer
                .write_one(NestedStreamItem {
                    label: "first".to_string(),
                    values: agent_stream(vec![1, 2]),
                })
                .await
                .expect("failed to write first nested stream item");
            writer
                .write_one(NestedStreamItem {
                    label: "second".to_string(),
                    values: agent_stream(vec![3, 4, 5]),
                })
                .await
                .expect("failed to write second nested stream item");
        });
        output
    }

    fn produce_nested_siblings(
        &self,
    ) -> (AgentStream<NestedStreamItem>, AgentStream<NestedStreamItem>) {
        let (mut left_writer, left) = AgentStream::new();
        let (mut right_writer, right) = AgentStream::new();
        spawn_local(async move {
            left_writer
                .write_one(NestedStreamItem {
                    label: "left".to_string(),
                    values: agent_stream(vec![1, 2]),
                })
                .await
                .expect("failed to write left nested sibling");
        });
        spawn_local(async move {
            right_writer
                .write_one(NestedStreamItem {
                    label: "right".to_string(),
                    values: agent_stream(vec![10, 20, 30]),
                })
                .await
                .expect("failed to write right nested sibling");
        });
        (left, right)
    }

    fn produce_siblings(&self) -> (AgentStream<String>, AgentStream<u32>) {
        (
            agent_stream(vec!["a".to_string(), "b".to_string()]),
            agent_stream((0..64).collect()),
        )
    }

    fn produce_sibling_error(&self) -> (AgentStream<u32>, AgentStream<u32>) {
        (agent_error_stream(), agent_stream((0..64).collect()))
    }

    fn produce_error(&self) -> AgentStream<u32> {
        agent_error_stream()
    }

    fn ping(&self) -> u64 {
        42
    }

    fn increment_scalar(&mut self) -> u64 {
        self.scalar += 1;
        self.scalar
    }

    fn noop(&self) {}
}

#[agent_definition]
pub trait StreamingRpcCaller {
    fn new(name: String) -> Self;

    async fn run(&self) -> StreamingRpcReport;
    fn create_input_gate(&self) -> PromiseId;
    async fn recover_input_after_caller_crash(&self, gate: PromiseId) -> Vec<u32>;
    async fn call_producer_error(&self) -> Vec<u32>;
    async fn call_stream_free(&self) -> u64;
}

struct StreamingRpcCallerImpl {
    name: String,
}

#[agent_implementation]
impl StreamingRpcCaller for StreamingRpcCallerImpl {
    fn new(name: String) -> Self {
        Self { name }
    }

    async fn run(&self) -> StreamingRpcReport {
        let mut target = StreamingRpcTargetClient::get(self.name.clone());

        let input_only = target.consume(agent_stream(vec![1, 2, 3])).await;
        let output_only = target
            .produce(vec![4, 5, 6])
            .await
            .collect()
            .await
            .expect("failed to consume output-only stream");
        let simultaneous = target
            .transform(agent_stream(vec![7, 8, 9]))
            .await
            .collect()
            .await
            .expect("failed to consume simultaneous input/output stream");

        let (nested_labels, nested_values) = target
            .consume_nested(NestedStreamInput {
                labels: agent_stream(vec!["left".to_string(), "right".to_string()]),
                values: Some(agent_stream(vec![10, 11])),
            })
            .await;

        let mut nested_items = target.produce_nested_items().await;
        let mut nested_item_labels = Vec::new();
        let mut nested_item_values = Vec::new();
        while let Some(item) = nested_items
            .next()
            .await
            .expect("failed to consume outer nested-item stream")
        {
            nested_item_labels.push(item.label);
            nested_item_values.push(
                item.values
                    .collect()
                    .await
                    .expect("failed to consume stream inside streamed item"),
            );
        }

        let (first, second) = target.produce_siblings().await;
        let first_sibling = first
            .collect()
            .await
            .expect("failed to consume first sibling stream");
        let second_sibling = second
            .collect()
            .await
            .expect("failed to consume second sibling stream");

        let mut dropped = target.produce(vec![20, 21, 22]).await;
        assert_eq!(dropped.next().await.unwrap(), Some(20));
        drop(dropped);
        let after_consumer_drop = target.ping().await;

        StreamingRpcReport {
            input_only,
            output_only,
            simultaneous,
            nested_labels,
            nested_values,
            nested_item_labels,
            nested_item_values,
            first_sibling,
            second_sibling,
            after_consumer_drop,
        }
    }

    fn create_input_gate(&self) -> PromiseId {
        golem_rust::create_promise()
    }

    async fn recover_input_after_caller_crash(&self, gate: PromiseId) -> Vec<u32> {
        let target = StreamingRpcTargetClient::get(self.name.clone());
        let (mut writer, input) = AgentStream::new();
        spawn_local(async move {
            writer
                .write_one(1)
                .await
                .expect("failed to write input before caller recovery gate");
            golem_rust::await_promise(&gate).await;
            writer
                .write_all(vec![2, 3])
                .await
                .expect("failed to write input after caller recovery gate");
        });
        target
            .transform(input)
            .await
            .collect()
            .await
            .expect("failed to collect transformed input after caller recovery")
    }

    async fn call_producer_error(&self) -> Vec<u32> {
        let mut target = StreamingRpcTargetClient::get(self.name.clone());
        target
            .produce_error()
            .await
            .collect()
            .await
            .expect("producer stream must fail")
    }

    async fn call_stream_free(&self) -> u64 {
        let mut target = StreamingRpcTargetClient::get(self.name.clone());
        target.increment_scalar().await
    }
}

// -- RPC test agents (replacing old caller/counters components) --

#[agent_definition]
pub trait RpcCounter {
    fn new(name: String) -> Self;
    fn inc_by(&mut self, value: u64);
    fn get_value(&self) -> u64;
    fn get_args(&self) -> Vec<String>;
    fn get_env(&self) -> Vec<(String, String)>;
}

struct RpcCounterImpl {
    _name: String,
    value: u64,
}

#[agent_implementation]
impl RpcCounter for RpcCounterImpl {
    fn new(name: String) -> Self {
        Self {
            _name: name,
            value: 0,
        }
    }

    fn inc_by(&mut self, value: u64) {
        self.value += value;
    }

    fn get_value(&self) -> u64 {
        self.value
    }

    fn get_args(&self) -> Vec<String> {
        std::env::args().collect()
    }

    fn get_env(&self) -> Vec<(String, String)> {
        std::env::vars().collect()
    }
}

#[derive(Debug, Clone, IntoSchema, FromSchema)]
pub enum TimelineNode {
    Leaf,
}

#[agent_definition]
pub trait RpcGlobalState {
    fn new(name: String) -> Self;
    fn inc_global_by(&mut self, value: u64);
    fn get_global_value(&self) -> u64;
    fn bug_wasm_rpc_i32(&self, node: TimelineNode) -> TimelineNode;
    fn bug_golem1265(&self, s: String) -> Result<(), String>;
}

struct RpcGlobalStateImpl {
    _name: String,
    global: u64,
}

#[agent_implementation]
impl RpcGlobalState for RpcGlobalStateImpl {
    fn new(name: String) -> Self {
        Self {
            _name: name,
            global: 0,
        }
    }

    fn inc_global_by(&mut self, value: u64) {
        self.global += value;
    }

    fn get_global_value(&self) -> u64 {
        self.global
    }

    fn bug_wasm_rpc_i32(&self, node: TimelineNode) -> TimelineNode {
        node
    }

    fn bug_golem1265(&self, s: String) -> Result<(), String> {
        log::info!("Got {s}");
        Ok(())
    }
}

#[agent_definition]
pub trait RpcCaller {
    fn new(name: String) -> Self;

    /// test1: Create 3 counter agents, increment them, return their values
    async fn test1(&self) -> Vec<(String, u64)>;

    /// test2: Use a persistent counter agent, increment on each call
    async fn test2(&mut self) -> u64;

    /// test3: Use a global state agent, increment on each call
    async fn test3(&self) -> u64;

    /// test4: Get args and env from a counter agent (context inheritance)
    async fn test4(&self) -> (Vec<String>, Vec<(String, String)>);

    /// test5: Create 3 counter agents in separate workers, increment them independently, return values
    async fn test5(&self) -> Vec<u64>;

    /// bug-wasm-rpc-i32: Pass a variant through RPC
    async fn bug_wasm_rpc_i32(&self, node: TimelineNode) -> TimelineNode;

    /// bug-golem1265: Pass a string through RPC and get Result back
    async fn bug_golem1265(&self, s: String) -> Result<(), String>;
}

struct RpcCallerImpl {
    name: String,
    counter_name: Option<String>,
}

#[agent_implementation]
impl RpcCaller for RpcCallerImpl {
    fn new(name: String) -> Self {
        Self {
            name,
            counter_name: None,
        }
    }

    async fn test1(&self) -> Vec<(String, u64)> {
        let counter_prefix = format!("{}_test1", self.name);

        let mut counter1 = RpcCounterClient::get(format!("{counter_prefix}_counter1"));
        let mut counter2 = RpcCounterClient::get(format!("{counter_prefix}_counter2"));
        let mut counter3 = RpcCounterClient::get(format!("{counter_prefix}_counter3"));

        counter1.inc_by(1).await;
        counter1.inc_by(1).await;
        counter1.inc_by(1).await;

        counter2.inc_by(2).await;
        counter2.inc_by(1).await;

        counter3.inc_by(3).await;

        let value1 = counter1.get_value().await;
        let value2 = counter2.get_value().await;
        let value3 = counter3.get_value().await;

        vec![
            (format!("{counter_prefix}_counter3"), value3),
            (format!("{counter_prefix}_counter2"), value2),
            (format!("{counter_prefix}_counter1"), value1),
        ]
    }

    async fn test2(&mut self) -> u64 {
        let counter_name = match &self.counter_name {
            Some(n) => n.clone(),
            None => {
                let n = format!("{}_test2_counter", self.name);
                self.counter_name = Some(n.clone());
                n
            }
        };
        let mut counter = RpcCounterClient::get(counter_name);
        counter.inc_by(1).await;
        counter.get_value().await
    }

    async fn test3(&self) -> u64 {
        let mut global = RpcGlobalStateClient::get(format!("{}_test3", self.name));
        global.inc_global_by(1).await;
        global.get_global_value().await
    }

    async fn test4(&self) -> (Vec<String>, Vec<(String, String)>) {
        let counter = RpcCounterClient::get(format!("{}_test4_counter", self.name));
        let args = counter.get_args().await;
        let env = counter.get_env().await;
        (args, env)
    }

    async fn test5(&self) -> Vec<u64> {
        let counter_prefix = format!("{}_test5", self.name);

        let mut counter1 = RpcCounterClient::get(format!("{counter_prefix}_counter1"));
        let mut counter2 = RpcCounterClient::get(format!("{counter_prefix}_counter2"));
        let mut counter3 = RpcCounterClient::get(format!("{counter_prefix}_counter3"));

        counter1.inc_by(1).await;
        counter1.inc_by(1).await;
        counter1.inc_by(1).await;

        counter2.inc_by(2).await;
        counter2.inc_by(1).await;

        counter3.inc_by(3).await;

        let value1 = counter1.get_value().await;
        let value2 = counter2.get_value().await;
        let value3 = counter3.get_value().await;

        vec![value1, value2, value3]
    }

    async fn bug_wasm_rpc_i32(&self, node: TimelineNode) -> TimelineNode {
        let global = RpcGlobalStateClient::get(format!("{}_bug32", self.name));
        global.bug_wasm_rpc_i32(node).await
    }

    async fn bug_golem1265(&self, s: String) -> Result<(), String> {
        let global = RpcGlobalStateClient::get(format!("{}_bug1265", self.name));
        global.bug_golem1265(s).await
    }
}

#[agent_definition]
pub trait RpcBlockingCounter {
    fn new(name: String) -> Self;
    fn inc_by(&mut self, value: u64);
    fn get_value(&self) -> u64;
    /// Creates a promise and returns its ID so the test can complete it later
    fn create_promise(&self) -> PromiseId;
    /// Blocks on a previously created promise
    fn await_promise(&self, promise_id: PromiseId);
}

struct RpcBlockingCounterImpl {
    _name: String,
    value: u64,
}

#[agent_implementation]
impl RpcBlockingCounter for RpcBlockingCounterImpl {
    fn new(name: String) -> Self {
        Self {
            _name: name,
            value: 0,
        }
    }

    fn inc_by(&mut self, value: u64) {
        self.value += value;
    }

    fn get_value(&self) -> u64 {
        self.value
    }

    fn create_promise(&self) -> PromiseId {
        golem_rust::create_promise()
    }

    fn await_promise(&self, promise_id: PromiseId) {
        golem_rust::blocking_await_promise(&promise_id);
    }
}

// -- RPC auth parity test agent --

/// Mirror of the WIT `rpc-error` variant so it can be returned from an agent
/// method and pattern-matched in integration tests.
#[derive(Debug, Clone, IntoSchema, FromSchema)]
pub enum RpcCallOutcome {
    Ok,
    Denied { details: String },
    NotFound { details: String },
    ProtocolError { details: String },
    RemoteInternalError { details: String },
}

impl From<RpcError> for RpcCallOutcome {
    fn from(e: RpcError) -> Self {
        match e {
            RpcError::Denied(details) => Self::Denied { details },
            RpcError::NotFound(details) => Self::NotFound { details },
            RpcError::ProtocolError(details) => Self::ProtocolError { details },
            RpcError::RemoteInternalError(details) => Self::RemoteInternalError { details },
            RpcError::RemoteAgentError(_) => Self::RemoteInternalError {
                details: "remote agent error".to_string(),
            },
        }
    }
}

/// Agent used to test RPC authorization parity (local vs remote path).
/// All methods return `RpcCallOutcome` so integration tests can do typed assertions
/// on the exact error variant rather than string matching.
#[agent_definition]
pub trait RpcAuthTester {
    fn new(name: String) -> Self;

    /// Attempt to call `inc_by(1)` on an `RpcCounter` agent with the given name.
    /// Returns `RpcCallOutcome::Ok` on success or a typed denial/error on failure.
    async fn try_call_counter(&self, counter_name: String) -> RpcCallOutcome;
}

struct RpcAuthTesterImpl {
    _name: String,
}

// -- Cancel test agents --

#[agent_definition]
pub trait CancelTester {
    fn new(name: String) -> Self;

    /// Starts an async RPC call and immediately cancels it, does not call get()
    fn test_cancel_before_await(&self, counter_name: String);

    /// Starts an async RPC call, awaits its completion, then cancels (should be no-op)
    async fn test_cancel_completed(&self, counter_name: String) -> u64;
}

struct CancelTesterImpl {
    _name: String,
}

#[agent_implementation]
impl RpcAuthTester for RpcAuthTesterImpl {
    fn new(name: String) -> Self {
        Self { _name: name }
    }

    async fn try_call_counter(&self, counter_name: String) -> RpcCallOutcome {
        let constructor = encode_single_parameter(counter_name);

        // Connect to the RpcCounter agent in the same component.
        // WasmRpc::new resolves the component_id from the registered agent type.
        let rpc = WasmRpc::new("RpcCounter", constructor, None, Vec::new());

        let arg = encode_single_parameter(1u64);

        match rpc.invoke_and_await("inc_by", arg, None) {
            Ok(_) => RpcCallOutcome::Ok,
            Err(e) => RpcCallOutcome::from(e),
        }
    }
}

#[agent_implementation]
impl CancelTester for CancelTesterImpl {
    fn new(name: String) -> Self {
        Self { _name: name }
    }

    fn test_cancel_before_await(&self, counter_name: String) {
        let constructor_data = encode_single_parameter(counter_name);
        let wasm_rpc = WasmRpc::new("RpcCounter", constructor_data, None, Vec::new());

        let input = encode_single_parameter(1u64);
        let future = wasm_rpc
            .async_invoke_and_await("inc_by", input, None)
            .future;

        // Cancel immediately before polling/awaiting
        future.cancel();
        // Don't call get() - that would trigger retry logic
        // The test verifies from outside that the counter was NOT incremented
    }

    async fn test_cancel_completed(&self, counter_name: String) -> u64 {
        let constructor_data = encode_single_parameter(counter_name.clone());
        let wasm_rpc = WasmRpc::new("RpcCounter", constructor_data, None, Vec::new());

        // First, call inc_by to increment the counter
        let input = encode_single_parameter(5u64);
        let future = wasm_rpc
            .async_invoke_and_await("inc_by", input, None)
            .future;

        // Wait for completion. The P3 Golem WIT replaced `subscribe()`/pollable
        // polling of `future-invoke-result` with an `async get()`, so we just
        // await it directly.
        let _ = future.get().await;

        // Cancel after completion - should be a no-op
        future.cancel();

        // Now get the value through the generated client to verify
        let counter = RpcCounterClient::get(counter_name);
        counter.get_value().await
    }
}

// -- Self-scheduling HTTP poller --
//
// Agent that sends an HTTP POST to a test server on every tick, then
// immediately self-schedules the next tick ~500 ms in the future.
// Used to verify that deleting the environment stops the loop: once the
// environment is gone the scheduler can no longer activate the agent and the
// HTTP server stops receiving pings.

fn datetime_500ms_from_now() -> Datetime {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time went backwards");
    let at = now + Duration::from_millis(500);
    Datetime {
        seconds: at.as_secs() as i64,
        nanoseconds: at.subsec_nanos(),
    }
}

#[agent_definition]
pub trait HttpPollingSelfScheduler {
    fn new(name: String) -> Self;

    /// Ping the test server and schedule the next tick 500 ms later.
    async fn tick(&self, host: String, port: u16);
}

struct HttpPollingSelfSchedulerImpl {
    name: String,
}

#[agent_implementation]
impl HttpPollingSelfScheduler for HttpPollingSelfSchedulerImpl {
    fn new(name: String) -> Self {
        Self { name }
    }

    async fn tick(&self, host: String, port: u16) {
        let _ = wasi_fetch::Client::new()
            .post(&format!("http://{host}:{port}/ping"))
            .send()
            .await;

        let me = HttpPollingSelfSchedulerClient::get(self.name.clone());
        me.schedule_tick(host, port, datetime_500ms_from_now())
            .expect("failed to schedule polling tick");
    }
}
