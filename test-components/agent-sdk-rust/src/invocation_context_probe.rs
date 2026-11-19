use golem_rust::agentic::{
    Caller, InvocationContext, Principal, SnapshotRestoreContext, spawn_local,
};
use golem_rust::golem_agentic::golem::agent::host::WasmRpc;
use golem_rust::golem_agentic::golem::agent::invocation_context;
use golem_rust::schema::SchemaValue;
use golem_rust::schema::wit::wire::InvocationOwner;
use golem_rust::{agent_definition, agent_implementation};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::rc::Rc;

/// What `golem:agent/invocation-context` answered at one point of the agent's life.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Observation {
    invocation_id: Option<String>,
    original: String,
    immediate: String,
}

/// `"<kind> <pair text>"`, or `"anonymous"`. It never includes claims.
fn principal_text(principal: &Principal) -> String {
    principal.to_string()
}

fn caller_text(caller: &Caller) -> String {
    format!(
        "original={} immediate={} tokens={}",
        principal_text(caller.original()),
        principal_text(caller.immediate()),
        caller.tokens().len()
    )
}

/// The hex blake3 of the `original` principal's claims JSON text, and the text's length in
/// bytes, as `<hex>:<length>`. A principal without claims counts as the empty text.
fn claims_digest(caller: &Caller) -> String {
    let claims = match caller.original() {
        Principal::User(user) => user.claims_json(),
        _ => "",
    };
    format!(
        "{}:{}",
        blake3::hash(claims.as_bytes()).to_hex(),
        claims.len()
    )
}

fn observe() -> Observation {
    let caller = invocation_context::caller();
    Observation {
        invocation_id: invocation_context::invocation_id().map(|id| {
            let owner = match id.owner {
                InvocationOwner::Agent(agent_id) => agent_id.agent_id,
                InvocationOwner::Tool(name) => format!("tool:{name}"),
            };
            format!("{owner}.{}", golem_rust::Uuid::from(id.id))
        }),
        original: principal_text(&Principal::from(caller.original)),
        immediate: principal_text(&Principal::from(caller.immediate)),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Report<'a> {
    constructor: &'a Observation,
    constructor_caller: &'a str,
    saved: &'a Option<Observation>,
    loaded: &'a Option<Observation>,
    method: Observation,
    method_caller: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedState {
    constructor: Observation,
    constructor_caller: String,
    saved: Observation,
    claims_digests: Vec<String>,
    spawned: Vec<Observation>,
    after_return: Vec<Observation>,
}

#[agent_definition(snapshotting = "enabled")]
pub trait InvocationContextProbe {
    fn new(name: String, ctx: InvocationContext) -> Self;

    /// Returns the observations of the constructor, of the last snapshot save and load, and of
    /// this method, as JSON, together with the callers that the SDK handed to the constructor
    /// and to this method.
    fn report(&self, ctx: InvocationContext) -> String;

    /// The caller of this call: `original=<text> immediate=<text> tokens=<count>`, where each
    /// text is `<kind> <pair text>` or `anonymous`.
    fn caller_summary(&self, ctx: InvocationContext) -> String;

    /// Records the digest of the original caller's claims text: its hex blake3 and its length
    /// in bytes, as `<hex>:<length>`. Returns the recorded entry.
    fn record_caller_claims_digest(&mut self, ctx: InvocationContext) -> String;

    /// Every digest that `record_caller_claims_digest` recorded, oldest first.
    fn recorded_caller_claims_digests(&self) -> Vec<String>;

    /// Awaits `report` on the probe named `peer`, then triggers another `report` on it. Returns
    /// the awaited report and the invocation ID from the metadata of the triggered call.
    async fn report_of_peer(&self, peer: String) -> String;

    /// Spawns a future before the method makes any host call. The future generates an
    /// idempotency key, which is a durable host call, and observes the invocation context.
    /// Returns its observation and keeps it.
    async fn report_from_spawned_future(&mut self) -> String;

    /// Returns every observation `report_from_spawned_future` kept, in order.
    fn spawned_future_reports(&self) -> String;

    /// Spawns a future and returns at once. After the method's result is returned, the future
    /// waits, generates an idempotency key and observes the invocation context, and keeps its
    /// observation.
    async fn observe_after_return(&self);

    /// Returns every observation the futures of `observe_after_return` kept, in order.
    fn after_return_reports(&self) -> String;
}

struct InvocationContextProbeImpl {
    constructor: Observation,
    constructor_caller: String,
    saved: Option<Observation>,
    loaded: Option<Observation>,
    claims_digests: Vec<String>,
    spawned: Vec<Observation>,
    after_return: Rc<RefCell<Vec<Observation>>>,
}

#[agent_implementation]
impl InvocationContextProbe for InvocationContextProbeImpl {
    fn new(_name: String, ctx: InvocationContext) -> Self {
        Self {
            constructor: observe(),
            constructor_caller: caller_text(ctx.caller()),
            saved: None,
            loaded: None,
            claims_digests: Vec::new(),
            spawned: Vec::new(),
            after_return: Rc::default(),
        }
    }

    fn report(&self, ctx: InvocationContext) -> String {
        serde_json::to_string(&Report {
            constructor: &self.constructor,
            constructor_caller: &self.constructor_caller,
            saved: &self.saved,
            loaded: &self.loaded,
            method: observe(),
            method_caller: caller_text(ctx.caller()),
        })
        .expect("report should serialize")
    }

    fn caller_summary(&self, ctx: InvocationContext) -> String {
        caller_text(ctx.caller())
    }

    fn record_caller_claims_digest(&mut self, ctx: InvocationContext) -> String {
        let digest = claims_digest(ctx.caller());
        self.claims_digests.push(digest.clone());
        digest
    }

    fn recorded_caller_claims_digests(&self) -> Vec<String> {
        self.claims_digests.clone()
    }

    async fn report_of_peer(&self, peer: String) -> String {
        let client = InvocationContextProbeClient::get(peer.clone());
        let report: serde_json::Value =
            serde_json::from_str(&client.report().await).expect("peer report should be JSON");
        let rpc = WasmRpc::new(
            "InvocationContextProbe",
            golem_rust::encode_schema_value(&SchemaValue::Record {
                fields: vec![SchemaValue::String(peer)],
            })
            .expect("constructor should encode"),
            None,
            Vec::new(),
        );
        let input = golem_rust::encode_schema_value(&SchemaValue::Record { fields: vec![] })
            .expect("input should encode");
        let triggered = golem_rust::InvocationMetadata::from(
            rpc.invoke("report", input, None)
                .expect("triggering report should succeed"),
        )
        .invocation_id
        .to_string();
        serde_json::json!({ "report": report, "triggered": triggered }).to_string()
    }

    async fn report_from_spawned_future(&mut self) -> String {
        let (sender, receiver) = futures::channel::oneshot::channel();
        spawn_local(async move {
            golem_rust::generate_idempotency_key();
            let _ = sender.send(observe());
        });
        let observation = receiver.await.expect("the spawned future should report");
        self.spawned.push(observation.clone());
        serde_json::to_string(&observation).expect("observation should serialize")
    }

    fn spawned_future_reports(&self) -> String {
        serde_json::to_string(&self.spawned).expect("observations should serialize")
    }

    async fn observe_after_return(&self) {
        let after_return = self.after_return.clone();
        spawn_local(async move {
            golem_rust::wasip3::clocks::monotonic_clock::wait_for(10_000_000).await;
            golem_rust::generate_idempotency_key();
            after_return.borrow_mut().push(observe());
        });
    }

    fn after_return_reports(&self) -> String {
        serde_json::to_string(&*self.after_return.borrow()).expect("observations should serialize")
    }

    async fn save_snapshot(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec(&SavedState {
            constructor: self.constructor.clone(),
            constructor_caller: self.constructor_caller.clone(),
            saved: observe(),
            claims_digests: self.claims_digests.clone(),
            spawned: self.spawned.clone(),
            after_return: self.after_return.borrow().clone(),
        })
        .map_err(|error| error.to_string())
    }

    async fn load_snapshot(
        bytes: Vec<u8>,
        _context: SnapshotRestoreContext,
    ) -> Result<Self, String> {
        let state: SavedState =
            serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        Ok(Self {
            constructor: state.constructor,
            constructor_caller: state.constructor_caller,
            saved: Some(state.saved),
            loaded: Some(observe()),
            claims_digests: state.claims_digests,
            spawned: state.spawned,
            after_return: Rc::new(RefCell::new(state.after_return)),
        })
    }
}
