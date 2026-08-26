use golem_rust::bindings::golem::agent::host::{Datetime, RpcError, WasmRpc};
use golem_rust::bindings::golem::permissions::{derive, inspect, revoke, types, wallet};
use golem_rust::schema::wit::wire::{AgentId as WireAgentId, PermissionCard};
use golem_rust::{
    CardId, ComponentId, FromSchema, IntoSchema, PromiseId, SchemaValue, agent_definition,
    agent_implementation, await_promise, create_promise, decode_schema_value, encode_schema_value,
};

fn encode_parameters(values: Vec<SchemaValue>) -> golem_rust::schema::wit::wire::SchemaValueTree {
    encode_schema_value(&SchemaValue::Record { fields: values })
        .expect("failed to encode RPC parameters")
}

fn parent_card() -> golem_rust::schema::wit::wire::PermissionCard {
    wallet::self_wallet()
        .into_iter()
        .find(|card| types::is_polymorphic(card))
        .expect("scope-card test parent is not installed")
}

fn is_inspect_grant(grant: &types::PatternGrant) -> bool {
    grant.class == "card" && grant.verb == "inspect" && grant.resource_id == "*"
}

fn retained_inspect_grant(card: &PermissionCard) -> types::PatternGrant {
    inspect::inspect_card(card)
        .expect("failed to inspect parent card")
        .lower_positive
        .into_iter()
        .find(is_inspect_grant)
        .expect("parent card does not grant card inspection")
}

fn derive_scope_card() -> golem_rust::schema::wit::wire::PermissionCard {
    let parent = parent_card();
    let grant = retained_inspect_grant(&parent);
    derive::derive_scope(&[&parent], &[grant], &[], &[], &[]).expect("scope-card derivation failed")
}

fn derive_persistent_card(parent: &PermissionCard) -> PermissionCard {
    let grant = retained_inspect_grant(parent);
    derive::derive(parent, &[grant], &[], &[], &[], None)
        .expect("persistent card derivation failed")
}

fn derive_persistent_card_from_wallet() -> PermissionCard {
    let parent = parent_card();
    let grant = retained_inspect_grant(&parent);
    derive::derive_from_wallet(&[grant], &[], &[], &[], None)
        .expect("wallet card derivation failed")
}

fn card_id(card: &PermissionCard) -> CardId {
    types::id(card).into()
}

fn card_has_id(card: &PermissionCard, card_id: &CardId) -> bool {
    &CardId::from(types::id(card)) == card_id
}

fn agent_holder(component_id: ComponentId, agent_id: String) -> types::Holder {
    types::Holder::Agent(WireAgentId {
        component_id: component_id.into(),
        agent_id,
    })
}

fn scope_card_observation(scope_card_id: &CardId, root_card_id: &CardId) -> (bool, bool, bool) {
    let card = wallet::self_wallet()
        .into_iter()
        .find(|card| card_has_id(card, scope_card_id));
    let Some(card) = card else {
        return (false, false, false);
    };

    let parent_matches = types::parents(&card)
        .iter()
        .any(|parent| &CardId::from(parent) == root_card_id);
    let inspect_matches = inspect::inspect_card(&card)
        .map(|view| {
            view.lower_positive.len() == 1
                && is_inspect_grant(&view.lower_positive[0])
                && view.lower_negative.is_empty()
                && view.upper_positive.is_empty()
                && view.upper_negative.is_empty()
        })
        .unwrap_or(false);
    (true, parent_matches, inspect_matches)
}

fn scope_card_rpc(target: String) -> WasmRpc {
    WasmRpc::create(
        "ScopeCardAgent",
        encode_parameters(vec![target.to_value()]),
        None,
        Vec::new(),
    )
    .expect("failed to create scope-card RPC client")
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

    async fn invoke_and_await_scope(&self, target: String) -> (bool, bool, bool, CardId);

    async fn async_invoke_and_await_scope(&self, target: String) -> (bool, bool, bool, CardId);

    async fn invoke_scope_after_promise(
        &self,
        target: String,
        release: PromiseId,
    ) -> (bool, bool, CardId);

    async fn invoke_and_await_repeated_scope_inspection(
        &self,
        target: String,
        repetitions: u32,
    ) -> bool;

    fn invoke_scope_is_denied(&self, target: String) -> bool;

    fn persistent_scope_is_denied(&self, target: String) -> bool;

    fn schedule_scope(&self, target: String);

    fn schedule_cancelable_scope(&self, target: String);

    fn inspect_scope(&self, scope_card_id: CardId, root_card_id: CardId) -> (bool, bool, bool);

    fn create_release_promise(&self) -> PromiseId;

    async fn await_release(&self, release: PromiseId) -> bool;

    async fn inspect_scope_after_promise(
        &self,
        scope_card_id: CardId,
        root_card_id: CardId,
        release: PromiseId,
    ) -> (bool, bool);

    fn derive_and_install_chain(
        &self,
        component_id: ComponentId,
        caller_agent_id: String,
        target_agent_id: String,
    ) -> (CardId, CardId);

    async fn derive_and_install_after_promise(
        &self,
        component_id: ComponentId,
        target_agent_id: String,
        release: PromiseId,
    ) -> CardId;

    async fn derive_before_promise(&self, release: PromiseId) -> CardId;

    fn derive_from_wallet_is_denied(&self) -> bool;

    fn wallet_card_count(&self) -> u32;

    fn authorize_repeatedly(&self, repetitions: u32) -> bool;

    fn inspect_repeatedly(&self, repetitions: u32) -> bool;

    fn revoke_card_by_id(&self, card_id: CardId) -> u32;

    async fn revoke_card_after_promise_is_denied(
        &self,
        card_id: CardId,
        release: PromiseId,
    ) -> bool;

    fn has_card(&self, card_id: CardId) -> bool;
}

pub struct ScopeCardAgentImpl {
    _name: String,
}

#[agent_implementation]
impl ScopeCardAgent for ScopeCardAgentImpl {
    fn new(name: String) -> Self {
        Self { _name: name }
    }

    async fn invoke_and_await_scope(&self, target: String) -> (bool, bool, bool, CardId) {
        let scope_card = derive_scope_card();
        let scope_card_id = card_id(&scope_card);
        let root_card_id = CardId::from(
            types::parents(&scope_card)
                .into_iter()
                .next()
                .expect("scope card has no root"),
        );
        let invocation = scope_card_rpc(target)
            .invoke_and_await(
                "inspect_scope",
                encode_parameters(vec![scope_card_id.to_value(), root_card_id.to_value()]),
                Some(&scope_card),
            )
            .expect("scope-card invoke-and-await failed");
        let (present, parent_matches, inspect_matches) = decode_scope_observation(
            invocation
                .result
                .expect("scope-card observation result is missing"),
        );
        (present, parent_matches, inspect_matches, scope_card_id)
    }

    async fn async_invoke_and_await_scope(&self, target: String) -> (bool, bool, bool, CardId) {
        let scope_card = derive_scope_card();
        let scope_card_id = card_id(&scope_card);
        let root_card_id = CardId::from(
            types::parents(&scope_card)
                .into_iter()
                .next()
                .expect("scope card has no root"),
        );
        let invocation = scope_card_rpc(target).async_invoke_and_await(
            "inspect_scope",
            encode_parameters(vec![scope_card_id.to_value(), root_card_id.to_value()]),
            Some(&scope_card),
        );
        let value = decode_schema_value(
            invocation
                .future
                .get()
                .await
                .expect("scope-card async invocation failed")
                .expect("scope-card async observation result is missing"),
        )
        .expect("failed to decode async scope-card observation");
        let (present, parent_matches, inspect_matches) =
            <(bool, bool, bool) as FromSchema>::from_value(&value)
                .expect("invalid async scope-card observation");
        (present, parent_matches, inspect_matches, scope_card_id)
    }

    async fn invoke_scope_after_promise(
        &self,
        target: String,
        release: PromiseId,
    ) -> (bool, bool, CardId) {
        let scope_card = derive_scope_card();
        let scope_card_id = card_id(&scope_card);
        let root_card_id = CardId::from(
            types::parents(&scope_card)
                .into_iter()
                .next()
                .expect("scope card has no root"),
        );
        let invocation = scope_card_rpc(target)
            .invoke_and_await(
                "inspect_scope_after_promise",
                encode_parameters(vec![
                    scope_card_id.to_value(),
                    root_card_id.to_value(),
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
        let (before, after) =
            <(bool, bool) as FromSchema>::from_value(&value).expect("invalid replay observation");
        (before, after, scope_card_id)
    }

    async fn invoke_and_await_repeated_scope_inspection(
        &self,
        target: String,
        repetitions: u32,
    ) -> bool {
        let scope_card = derive_scope_card();
        let invocation = scope_card_rpc(target)
            .invoke_and_await(
                "inspect_repeatedly",
                encode_parameters(vec![repetitions.to_value()]),
                Some(&scope_card),
            )
            .expect("repeated scope-card inspection failed");
        let value = decode_schema_value(
            invocation
                .result
                .expect("repeated scope-card inspection result is missing"),
        )
        .expect("failed to decode repeated scope-card inspection");
        bool::from_value(&value).expect("invalid repeated scope-card inspection result")
    }

    fn invoke_scope_is_denied(&self, target: String) -> bool {
        let scope_card = derive_scope_card();
        matches!(
            scope_card_rpc(target).invoke(
                "inspect_scope",
                encode_parameters(vec![]),
                Some(&scope_card),
            ),
            Err(RpcError::Denied(_))
        )
    }

    fn persistent_scope_is_denied(&self, target: String) -> bool {
        let parent = parent_card();
        matches!(
            scope_card_rpc(target).invoke_and_await(
                "inspect_scope",
                encode_parameters(vec![]),
                Some(&parent),
            ),
            Err(RpcError::Denied(_))
        )
    }

    fn schedule_scope(&self, target: String) {
        let scope_card = derive_scope_card();
        scope_card_rpc(target)
            .schedule_invocation(
                Datetime {
                    seconds: 0,
                    nanoseconds: 0,
                },
                "inspect_scope",
                encode_parameters(vec![]),
                Some(&scope_card),
            )
            .unwrap();
    }

    fn schedule_cancelable_scope(&self, target: String) {
        let scope_card = derive_scope_card();
        scope_card_rpc(target)
            .schedule_cancelable_invocation(
                Datetime {
                    seconds: 0,
                    nanoseconds: 0,
                },
                "inspect_scope",
                encode_parameters(vec![]),
                Some(&scope_card),
            )
            .unwrap();
    }

    fn inspect_scope(&self, scope_card_id: CardId, root_card_id: CardId) -> (bool, bool, bool) {
        scope_card_observation(&scope_card_id, &root_card_id)
    }

    fn create_release_promise(&self) -> PromiseId {
        create_promise()
    }

    async fn await_release(&self, release: PromiseId) -> bool {
        await_promise(&release).await;
        true
    }

    async fn inspect_scope_after_promise(
        &self,
        scope_card_id: CardId,
        root_card_id: CardId,
        release: PromiseId,
    ) -> (bool, bool) {
        let before = scope_card_observation(&scope_card_id, &root_card_id).0;
        await_promise(&release).await;
        let after = scope_card_observation(&scope_card_id, &root_card_id).0;
        (before, after)
    }

    fn derive_and_install_chain(
        &self,
        component_id: ComponentId,
        caller_agent_id: String,
        target_agent_id: String,
    ) -> (CardId, CardId) {
        let parent = derive_persistent_card_from_wallet();
        let child = derive_persistent_card(&parent);
        let parent_id = card_id(&parent);
        let child_id = card_id(&child);
        wallet::install_card(child, &agent_holder(component_id.clone(), target_agent_id))
            .expect("failed to install derived card");
        wallet::install_card(parent, &agent_holder(component_id, caller_agent_id))
            .expect("failed to install parent card");
        (parent_id, child_id)
    }

    async fn derive_and_install_after_promise(
        &self,
        component_id: ComponentId,
        target_agent_id: String,
        release: PromiseId,
    ) -> CardId {
        let card = derive_persistent_card_from_wallet();
        let id = card_id(&card);
        await_promise(&release).await;
        wallet::install_card(card, &agent_holder(component_id, target_agent_id))
            .expect("failed to install replayed card");
        id
    }

    async fn derive_before_promise(&self, release: PromiseId) -> CardId {
        let card = derive_persistent_card_from_wallet();
        let id = card_id(&card);
        await_promise(&release).await;
        id
    }

    fn derive_from_wallet_is_denied(&self) -> bool {
        let grant = types::PatternGrant {
            class: "card".to_string(),
            owner: "*".to_string(),
            recipient: "*".to_string(),
            verb: "inspect".to_string(),
            resource_id: "*".to_string(),
        };
        derive::derive_from_wallet(&[grant], &[], &[], &[], None).is_err()
    }

    fn wallet_card_count(&self) -> u32 {
        wallet::self_wallet().len() as u32
    }

    fn authorize_repeatedly(&self, repetitions: u32) -> bool {
        let parent = parent_card();
        let grant = retained_inspect_grant(&parent);
        (0..repetitions).all(|_| {
            inspect::inspect_card(&parent).is_ok()
                && derive::derive_from_wallet(std::slice::from_ref(&grant), &[], &[], &[], None)
                    .is_ok()
        })
    }

    fn inspect_repeatedly(&self, repetitions: u32) -> bool {
        let parent = parent_card();
        (0..repetitions).all(|_| inspect::inspect_card(&parent).is_ok())
    }

    fn revoke_card_by_id(&self, card_id: CardId) -> u32 {
        let card = wallet::self_wallet()
            .into_iter()
            .find(|card| card_has_id(card, &card_id))
            .expect("card to revoke is not installed");
        revoke::revoke_card(card).expect("card revocation failed")
    }

    async fn revoke_card_after_promise_is_denied(
        &self,
        card_id: CardId,
        release: PromiseId,
    ) -> bool {
        let card = wallet::self_wallet()
            .into_iter()
            .find(|card| card_has_id(card, &card_id))
            .expect("card to revoke is not installed");
        await_promise(&release).await;
        revoke::revoke_card(card).is_err()
    }

    fn has_card(&self, card_id: CardId) -> bool {
        wallet::self_wallet()
            .into_iter()
            .any(|card| card_has_id(&card, &card_id))
    }
}
