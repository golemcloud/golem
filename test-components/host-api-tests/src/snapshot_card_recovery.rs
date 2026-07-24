use golem_rust::bindings::golem::agent::host::{Datetime, RpcError, WasmRpc};
use golem_rust::bindings::golem::permissions::{derive, inspect, types, wallet};
use golem_rust::{
    Card, CardId, FromSchema, IntoSchema, PromiseId, SchemaValue, Uuid, agent_definition,
    agent_implementation, await_promise, create_promise, decode_schema_value, derive_card,
    encode_schema_value, install_card,
};

#[agent_definition(snapshotting = "enabled")]
pub trait SnapshotCardRecoveryAgent {
    fn new() -> Self;

    fn install_card_by_id(&self, high_bits: u64, low_bits: u64) -> bool;

    fn derive_card_by_id(&self, high_bits: u64, low_bits: u64) -> bool;
}

pub struct SnapshotCardRecoveryAgentImpl;

#[agent_implementation]
impl SnapshotCardRecoveryAgent for SnapshotCardRecoveryAgentImpl {
    fn new() -> Self {
        Self
    }

    fn install_card_by_id(&self, high_bits: u64, low_bits: u64) -> bool {
        install_card(Card {
            card_id: CardId {
                uuid: Uuid::from_u64_pair(high_bits, low_bits),
            },
        })
        .is_ok()
    }

    fn derive_card_by_id(&self, high_bits: u64, low_bits: u64) -> bool {
        derive_card(Card {
            card_id: CardId {
                uuid: Uuid::from_u64_pair(high_bits, low_bits),
            },
        })
        .is_ok()
    }

    async fn save_snapshot(&self) -> Result<Vec<u8>, String> {
        Ok(Vec::new())
    }

    async fn load_snapshot(&mut self, _bytes: Vec<u8>) -> Result<(), String> {
        Ok(())
    }
}

fn encode_parameters(values: Vec<SchemaValue>) -> golem_rust::schema::wit::wire::SchemaValueTree {
    encode_schema_value(&SchemaValue::Record { fields: values })
        .expect("failed to encode RPC parameters")
}

fn parent_card(high_bits: u64, low_bits: u64) -> golem_rust::schema::wit::wire::PermissionCard {
    wallet::self_wallet()
        .into_iter()
        .find(|card| {
            let id = types::id(card);
            id.uuid.high_bits == high_bits && id.uuid.low_bits == low_bits
        })
        .expect("scope-card test parent is not installed")
}

fn inspect_grant() -> types::PatternGrant {
    types::PatternGrant {
        class: "card".to_string(),
        owner: "*".to_string(),
        recipient: "*".to_string(),
        verb: "inspect".to_string(),
        resource_id: "*".to_string(),
    }
}

fn grant_matches(left: &types::PatternGrant, right: &types::PatternGrant) -> bool {
    left.class == right.class
        && left.owner == right.owner
        && left.recipient == right.recipient
        && left.verb == right.verb
        && left.resource_id == right.resource_id
}

fn derive_scope_card(
    high_bits: u64,
    low_bits: u64,
) -> golem_rust::schema::wit::wire::PermissionCard {
    let parent = parent_card(high_bits, low_bits);
    derive::derive_scope(&[&parent], &[inspect_grant()], &[], &[], &[])
        .expect("scope-card derivation failed")
}

fn scope_card_observation(high_bits: u64, low_bits: u64) -> (bool, bool, bool) {
    let card = wallet::self_wallet().into_iter().find(|card| {
        types::parents(card)
            .iter()
            .any(|parent| parent.uuid.high_bits == high_bits && parent.uuid.low_bits == low_bits)
    });
    let Some(card) = card else {
        return (false, false, false);
    };

    let parent_matches = types::parents(&card)
        .iter()
        .any(|parent| parent.uuid.high_bits == high_bits && parent.uuid.low_bits == low_bits);
    let inspect_matches = inspect::inspect_card(&card)
        .map(|view| {
            view.lower_positive.len() == 1
                && grant_matches(&view.lower_positive[0], &inspect_grant())
                && view.lower_negative.is_empty()
                && view.upper_positive.is_empty()
                && view.upper_negative.is_empty()
        })
        .unwrap_or(false);
    (true, parent_matches, inspect_matches)
}

fn scope_card_rpc(target: String) -> WasmRpc {
    WasmRpc::new(
        "ScopeCardAgent",
        encode_parameters(vec![target.to_value()]),
        None,
        Vec::new(),
    )
}

fn decode_scope_observation(
    value: golem_rust::schema::wit::wire::SchemaValueTree,
) -> (bool, bool, bool) {
    let value = decode_schema_value(value).expect("failed to decode scope-card observation");
    <(bool, bool, bool) as FromSchema>::from_value(&value).expect("invalid scope-card observation")
}

#[agent_definition]
pub trait ScopeCardAgent {
    fn new(name: String) -> Self;

    fn install_parent(&self, high_bits: u64, low_bits: u64) -> bool;

    async fn invoke_and_await_scope(
        &self,
        target: String,
        high_bits: u64,
        low_bits: u64,
    ) -> (bool, bool, bool);

    async fn async_invoke_and_await_scope(
        &self,
        target: String,
        high_bits: u64,
        low_bits: u64,
    ) -> (bool, bool, bool);

    async fn invoke_scope_after_promise(
        &self,
        target: String,
        high_bits: u64,
        low_bits: u64,
        release: PromiseId,
    ) -> (bool, bool);

    fn invoke_scope_is_denied(&self, target: String, high_bits: u64, low_bits: u64) -> bool;

    fn persistent_scope_is_denied(&self, target: String, high_bits: u64, low_bits: u64) -> bool;

    fn schedule_scope(&self, target: String, high_bits: u64, low_bits: u64);

    fn schedule_cancelable_scope(&self, target: String, high_bits: u64, low_bits: u64);

    fn inspect_scope(&self, high_bits: u64, low_bits: u64) -> (bool, bool, bool);

    fn create_release_promise(&self) -> PromiseId;

    async fn inspect_scope_after_promise(
        &self,
        high_bits: u64,
        low_bits: u64,
        release: PromiseId,
    ) -> (bool, bool);

    fn has_scope(&self, high_bits: u64, low_bits: u64) -> bool;
}

pub struct ScopeCardAgentImpl {
    _name: String,
}

#[agent_implementation]
impl ScopeCardAgent for ScopeCardAgentImpl {
    fn new(name: String) -> Self {
        Self { _name: name }
    }

    fn install_parent(&self, high_bits: u64, low_bits: u64) -> bool {
        install_card(Card {
            card_id: CardId {
                uuid: Uuid::from_u64_pair(high_bits, low_bits),
            },
        })
        .is_ok()
    }

    async fn invoke_and_await_scope(
        &self,
        target: String,
        high_bits: u64,
        low_bits: u64,
    ) -> (bool, bool, bool) {
        let scope_card = derive_scope_card(high_bits, low_bits);
        let invocation = scope_card_rpc(target)
            .invoke_and_await(
                "inspect_scope",
                encode_parameters(vec![high_bits.to_value(), low_bits.to_value()]),
                Some(&scope_card),
            )
            .expect("scope-card invoke-and-await failed");
        decode_scope_observation(
            invocation
                .result
                .expect("scope-card observation result is missing"),
        )
    }

    async fn async_invoke_and_await_scope(
        &self,
        target: String,
        high_bits: u64,
        low_bits: u64,
    ) -> (bool, bool, bool) {
        let scope_card = derive_scope_card(high_bits, low_bits);
        let invocation = scope_card_rpc(target).async_invoke_and_await(
            "inspect_scope",
            encode_parameters(vec![high_bits.to_value(), low_bits.to_value()]),
            Some(&scope_card),
        );
        loop {
            let pollable = invocation.future.subscribe();
            golem_rust::agentic::await_pollable(pollable).await;
            if let Some(result) = invocation.future.get() {
                let value = decode_schema_value(
                    result
                        .expect("scope-card async invocation failed")
                        .expect("scope-card async observation result is missing"),
                )
                .expect("failed to decode async scope-card observation");
                break <(bool, bool, bool) as FromSchema>::from_value(&value)
                    .expect("invalid async scope-card observation");
            }
        }
    }

    async fn invoke_scope_after_promise(
        &self,
        target: String,
        high_bits: u64,
        low_bits: u64,
        release: PromiseId,
    ) -> (bool, bool) {
        let scope_card = derive_scope_card(high_bits, low_bits);
        let invocation = scope_card_rpc(target)
            .invoke_and_await(
                "inspect_scope_after_promise",
                encode_parameters(vec![
                    high_bits.to_value(),
                    low_bits.to_value(),
                    release.to_value(),
                ]),
                Some(&scope_card),
            )
            .expect("replay scope-card invocation failed");
        let value = decode_schema_value(
            invocation
                .result
                .expect("replay observation result is missing"),
        )
        .expect("failed to decode replay observation");
        <(bool, bool) as FromSchema>::from_value(&value).expect("invalid replay observation")
    }

    fn invoke_scope_is_denied(&self, target: String, high_bits: u64, low_bits: u64) -> bool {
        let scope_card = derive_scope_card(high_bits, low_bits);
        matches!(
            scope_card_rpc(target).invoke(
                "inspect_scope",
                encode_parameters(vec![high_bits.to_value(), low_bits.to_value()]),
                Some(&scope_card),
            ),
            Err(RpcError::Denied(_))
        )
    }

    fn persistent_scope_is_denied(&self, target: String, high_bits: u64, low_bits: u64) -> bool {
        let parent = parent_card(high_bits, low_bits);
        matches!(
            scope_card_rpc(target).invoke_and_await(
                "inspect_scope",
                encode_parameters(vec![high_bits.to_value(), low_bits.to_value()]),
                Some(&parent),
            ),
            Err(RpcError::Denied(_))
        )
    }

    fn schedule_scope(&self, target: String, high_bits: u64, low_bits: u64) {
        let scope_card = derive_scope_card(high_bits, low_bits);
        scope_card_rpc(target).schedule_invocation(
            Datetime {
                seconds: 0,
                nanoseconds: 0,
            },
            "inspect_scope",
            encode_parameters(vec![high_bits.to_value(), low_bits.to_value()]),
            Some(&scope_card),
        );
    }

    fn schedule_cancelable_scope(&self, target: String, high_bits: u64, low_bits: u64) {
        let scope_card = derive_scope_card(high_bits, low_bits);
        scope_card_rpc(target).schedule_cancelable_invocation(
            Datetime {
                seconds: 0,
                nanoseconds: 0,
            },
            "inspect_scope",
            encode_parameters(vec![high_bits.to_value(), low_bits.to_value()]),
            Some(&scope_card),
        );
    }

    fn inspect_scope(&self, high_bits: u64, low_bits: u64) -> (bool, bool, bool) {
        scope_card_observation(high_bits, low_bits)
    }

    fn create_release_promise(&self) -> PromiseId {
        create_promise()
    }

    async fn inspect_scope_after_promise(
        &self,
        high_bits: u64,
        low_bits: u64,
        release: PromiseId,
    ) -> (bool, bool) {
        let before = scope_card_observation(high_bits, low_bits).0;
        await_promise(&release).await;
        let after = scope_card_observation(high_bits, low_bits).0;
        (before, after)
    }

    fn has_scope(&self, high_bits: u64, low_bits: u64) -> bool {
        scope_card_observation(high_bits, low_bits).0
    }
}
