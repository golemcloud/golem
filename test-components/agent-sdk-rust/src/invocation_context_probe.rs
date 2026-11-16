use golem_rust::agentic::{Principal, SnapshotRestoreContext};
use golem_rust::golem_agentic::golem::agent::host::WasmRpc;
use golem_rust::golem_agentic::golem::agent::invocation_context;
use golem_rust::schema::SchemaValue;
use golem_rust::schema::wit::wire::InvocationOwner;
use golem_rust::{agent_definition, agent_implementation};
use serde::{Deserialize, Serialize};

/// What `golem:agent/invocation-context` answered at one point of the agent's life.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Observation {
    invocation_id: Option<String>,
    original: String,
    immediate: String,
}

fn principal_text(principal: &Principal) -> String {
    match principal {
        Principal::Oidc(oidc) => format!("oidc:{}", oidc.sub),
        Principal::Agent(agent) => format!("agent:{}", agent.agent_id.agent_id),
        Principal::GolemUser(user) => format!(
            "golem-user:{}",
            golem_rust::Uuid::from(user.account_id.uuid.clone())
        ),
        Principal::Anonymous => "anonymous".to_string(),
    }
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
        original: principal_text(&caller.original),
        immediate: principal_text(&caller.immediate),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Report<'a> {
    constructor: &'a Observation,
    constructor_principal: &'a str,
    saved: &'a Option<Observation>,
    loaded: &'a Option<Observation>,
    method: Observation,
    method_principal: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedState {
    constructor: Observation,
    constructor_principal: String,
    saved: Observation,
}

#[agent_definition(snapshotting = "enabled")]
pub trait InvocationContextProbe {
    fn new(name: String, principal: Principal) -> Self;

    /// Returns the observations of the constructor, of the last snapshot save and load, and of
    /// this method, as JSON, together with the principals the SDK injected.
    fn report(&self, principal: Principal) -> String;

    /// Awaits `report` on the probe named `peer`, then triggers another `report` on it. Returns
    /// the awaited report and the invocation ID from the metadata of the triggered call.
    async fn report_of_peer(&self, peer: String) -> String;
}

struct InvocationContextProbeImpl {
    constructor: Observation,
    constructor_principal: String,
    saved: Option<Observation>,
    loaded: Option<Observation>,
}

#[agent_implementation]
impl InvocationContextProbe for InvocationContextProbeImpl {
    fn new(_name: String, principal: Principal) -> Self {
        Self {
            constructor: observe(),
            constructor_principal: principal_text(&principal),
            saved: None,
            loaded: None,
        }
    }

    fn report(&self, principal: Principal) -> String {
        serde_json::to_string(&Report {
            constructor: &self.constructor,
            constructor_principal: &self.constructor_principal,
            saved: &self.saved,
            loaded: &self.loaded,
            method: observe(),
            method_principal: principal_text(&principal),
        })
        .expect("report should serialize")
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

    async fn save_snapshot(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec(&SavedState {
            constructor: self.constructor.clone(),
            constructor_principal: self.constructor_principal.clone(),
            saved: observe(),
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
            constructor_principal: state.constructor_principal,
            saved: Some(state.saved),
            loaded: Some(observe()),
        })
    }
}
