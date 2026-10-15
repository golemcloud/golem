use golem_rust::bindings::golem::agent::host::{Datetime, RpcError, WasmRpc};
use golem_rust::golem_schema::CardId;
use golem_rust::permissions::{
    self, Bounds, Holder, Lifespan, PatternGrant, PermissionCard, PermissionError, ScopeCard,
};
use golem_rust::{
    AgentId, ComponentId, FromSchema, GolemReflectError, IntoSchema, ParsedAgentId, PromiseId,
    SchemaValue, agent_client, agent_definition, agent_implementation, await_promise,
    create_promise, decode_schema_value, encode_schema_value,
};
use std::cell::RefCell;
use std::time::Duration;

fn encode_parameters(values: Vec<SchemaValue>) -> golem_rust::schema::wit::wire::SchemaValueTree {
    encode_schema_value(&SchemaValue::Record { fields: values })
        .expect("failed to encode RPC parameters")
}

/// Raw `golem:permissions` calls through this component's own bindings.
///
/// The typed `golem_rust::permissions` API never hands out a host handle or a
/// card id. These tests need both: a raw handle for the `golem:agent/host`
/// invocation variants that the typed clients never offer, and card ids to
/// report to the test harness.
mod raw {
    use crate::golem_host_api::gated_host_bindings::golem::permissions::{
        derive, inspect, revoke, types, wallet,
    };
    use golem_rust::golem_schema::CardId;
    use golem_rust::schema::wit::wire;
    use golem_rust::{ComponentId, Uuid};

    pub use golem_rust::schema::wit::wire::PermissionCard as Card;
    pub use types::PermissionError;

    fn card_id(id: wire::CardId) -> CardId {
        CardId::new(Uuid::from(id.uuid))
    }

    pub fn id(card: &Card) -> CardId {
        card_id(types::id(card))
    }

    pub fn parents(card: &Card) -> Vec<CardId> {
        types::parents(card).into_iter().map(card_id).collect()
    }

    pub fn view(card: &Card) -> Result<types::CardView, PermissionError> {
        inspect::inspect_card(card)
    }

    fn is_template(card: &Card) -> bool {
        view(card).is_ok_and(|view| {
            view.lower_positive
                .iter()
                .chain(&view.lower_negative)
                .chain(&view.upper_positive)
                .chain(&view.upper_negative)
                .any(|grant| grant.owner.contains('?'))
        })
    }

    pub fn is_inspect_grant(grant: &types::PatternGrant) -> bool {
        grant.class == "card" && grant.verb == "inspect" && grant.resource_id == "*"
    }

    /// The caller's initial card, which is the root of every card derived
    /// from the wallet.
    pub fn template() -> Card {
        wallet::self_wallet()
            .into_iter()
            .find(is_template)
            .expect("scope-card test parent is not installed")
    }

    fn inspect_grant(card: &Card) -> types::PatternGrant {
        view(card)
            .expect("failed to inspect parent card")
            .lower_positive
            .into_iter()
            .find(is_inspect_grant)
            .expect("parent card does not grant card inspection")
    }

    pub fn derive_scope_card() -> Card {
        let parent = template();
        let grant = inspect_grant(&parent);
        derive::derive_scope(&[&parent], &[grant], &[], &[], &[])
            .expect("scope-card derivation failed")
    }

    pub fn derive_scope(parents: &[&Card]) -> Result<Card, PermissionError> {
        derive::derive_scope(parents, &[], &[], &[], &[])
    }

    pub fn derive_from_wallet() -> Card {
        let grant = inspect_grant(&template());
        derive::derive_from_wallet(&[grant], &[], &[], &[], None)
            .expect("wallet card derivation failed")
    }

    pub fn derive_child(parent: &Card) -> Card {
        let grant = inspect_grant(parent);
        derive::derive(parent, &[grant], &[], &[], &[], None)
            .expect("persistent card derivation failed")
    }

    pub fn install(
        card: Card,
        component_id: ComponentId,
        agent_id: String,
    ) -> Result<(), PermissionError> {
        let agent_id = wire::AgentId {
            component_id: wire::ComponentId {
                uuid: component_id.uuid.into(),
            },
            agent_id,
        };
        wallet::install_card(card, &types::Holder::Agent(agent_id))
    }

    pub fn revoke(card: Card) -> Result<u32, PermissionError> {
        revoke::revoke_card(card)
    }

    pub fn wallet_card(card_id: &CardId) -> Option<Card> {
        wallet::self_wallet()
            .into_iter()
            .find(|card| &id(card) == card_id)
    }

    /// The wallet cards derived from `root`, such as a scope card lent to
    /// this invocation by an agent whose initial card is `root`.
    pub fn children_of(root: &CardId) -> Vec<Card> {
        wallet::self_wallet()
            .into_iter()
            .filter(|card| parents(card).contains(root))
            .collect()
    }
}

fn is_template_card(card: &PermissionCard) -> bool {
    let view = card.inspect().expect("failed to inspect wallet card");
    view.lower_positive
        .iter()
        .chain(&view.lower_negative)
        .chain(&view.upper_positive)
        .chain(&view.upper_negative)
        .any(|grant| grant.owner.contains('?'))
}

fn parent_card() -> PermissionCard {
    permissions::wallet()
        .into_iter()
        .find(is_template_card)
        .expect("scope-card test parent is not installed")
}

fn is_inspect_grant(grant: &PatternGrant) -> bool {
    grant.class == "card" && grant.verb == "inspect" && grant.resource_id == "*"
}

fn retained_inspect_grant(card: &PermissionCard) -> PatternGrant {
    card.inspect()
        .expect("failed to inspect parent card")
        .lower_positive
        .into_iter()
        .find(is_inspect_grant)
        .expect("parent card does not grant card inspection")
}

fn lower_positive(grant: PatternGrant) -> Bounds {
    Bounds {
        lower_positive: vec![grant],
        ..Bounds::default()
    }
}

fn derive_scope_card() -> ScopeCard {
    let parent = parent_card();
    let grant = retained_inspect_grant(&parent);
    permissions::derive_scope(&[(&parent).into()], &lower_positive(grant))
        .expect("scope-card derivation failed")
}

fn derive_persistent_card_from_wallet() -> PermissionCard {
    let parent = parent_card();
    let grant = retained_inspect_grant(&parent);
    permissions::derive_from_wallet(&lower_positive(grant), None)
        .expect("wallet card derivation failed")
}

fn wallet_card(card_id: &permissions::CardId) -> Option<PermissionCard> {
    permissions::wallet()
        .into_iter()
        .find(|card| &card.id() == card_id)
}

fn agent_holder(component_id: ComponentId, agent_id: String) -> Holder {
    Holder::Agent(AgentId::new(component_id, agent_id))
}

/// Observes the scope card `scope_card_id` in this agent's wallet: whether
/// it is present, names `root_card_id` as its parent, and grants only card
/// inspection.
fn scope_card_observation(scope_card_id: &CardId, root_card_id: &CardId) -> (bool, bool, bool) {
    let Some(card) = raw::wallet_card(scope_card_id) else {
        return (false, false, false);
    };
    (
        true,
        raw::parents(&card).contains(root_card_id),
        grants_only_inspection(&card),
    )
}

fn grants_only_inspection(card: &raw::Card) -> bool {
    raw::view(card).is_ok_and(|view| {
        view.lower_positive.len() == 1
            && raw::is_inspect_grant(&view.lower_positive[0])
            && view.lower_negative.is_empty()
            && view.upper_positive.is_empty()
            && view.upper_negative.is_empty()
    })
}

/// Observes a scope card lent to this invocation by an agent whose initial
/// card is `root_card_id`: whether this agent's wallet lists it, whether it
/// grants only card inspection, and its id.
fn lent_scope_card_observation(root_card_id: &CardId) -> (bool, bool, Option<CardId>) {
    match raw::children_of(root_card_id).first() {
        Some(card) => (true, grants_only_inspection(card), Some(raw::id(card))),
        None => (false, false, None),
    }
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

    async fn invoke_and_await_scope(&self, target: String) -> (bool, bool, bool, CardId);

    async fn async_invoke_and_await_scope(&self, target: String) -> (bool, bool, bool, CardId);

    async fn invoke_scope_after_promise(
        &self,
        target: String,
        release: PromiseId,
    ) -> (bool, bool, CardId);

    async fn repeated_scope_inspection_through_default_scope_card(
        &self,
        target: String,
        repetitions: u32,
    ) -> bool;

    fn invoke_scope_is_denied(&self, target: String) -> bool;

    fn persistent_scope_is_denied(&self, target: String) -> bool;

    async fn persistent_async_scope_is_denied(&self, target: String) -> bool;

    fn schedule_scope(&self, target: String);

    fn schedule_cancelable_scope(&self, target: String);

    fn inspect_scope(&self, scope_card_id: CardId, root_card_id: CardId) -> (bool, bool, bool);

    /// Observes a scope card lent to this invocation by an agent whose
    /// initial card is `root_card_id`; see [`lent_scope_card_observation`].
    fn inspect_lent_scope(&self, root_card_id: CardId) -> (bool, bool, Option<CardId>);

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

    async fn derive_tool_cards_before_promise(
        &self,
        owner: String,
        release: PromiseId,
    ) -> Vec<String>;

    fn derive_from_wallet_denial(&self) -> Option<String>;

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

    /// Runs the shared permission-card scenario through the typed API and
    /// returns each named check with its outcome.
    async fn typed_card_scenario(
        &self,
        component_id: ComponentId,
        caller_agent_id: String,
        target_agent_id: String,
        target_name: String,
        mismatched_recipient: String,
    ) -> Vec<(String, bool)>;

    /// Uses a card after it was moved out by decoding; must trap.
    fn use_moved_card(&self) -> String;

    /// Derives a scope card, keeps it in agent state together with a client
    /// whose default scope card it is, and returns whether a call through
    /// that client observed it.
    async fn keep_scope_card(&self, target: String) -> bool;

    /// Uses the scope card kept by `keep_scope_card` in a later invocation.
    async fn stale_scope_card_observations(&self, target: String) -> Vec<(String, bool)>;

    /// Lends a scope card that grants only the read of [`LENT_ENV`] by the
    /// agents of the environment `env_owner` to a `SealedScopeAgent`
    /// (`sealed`) or a `ScopeCardAgent` target. Returns the target's
    /// [`lent_scope_observation`], whether requesting the inspected template
    /// grant, which has an owner slot, is an invalid pattern, and the id of
    /// this agent's initial card.
    async fn lend_env_scope(
        &self,
        target: String,
        sealed: bool,
        env_owner: String,
    ) -> (Option<CardId>, Option<String>, bool, CardId);

    /// The id of a scope card lent by an agent whose initial card is
    /// `root_card_id`, if the wallet lists it, and the value of [`LENT_ENV`]
    /// visible to this agent.
    fn lent_scope_observation(&self, root_card_id: CardId) -> (Option<CardId>, Option<String>);

    /// Lends a scope card to a `SealedScopeAgent` target, which lists it;
    /// see `SealedScopeAgent::observe_lent_scope`.
    async fn lend_scope_to_observer(&self, target: String) -> (bool, bool);
}

/// A method-only client of `ScopeCardAgent`, whose awaited calls return RPC
/// errors instead of trapping.
#[agent_client]
pub trait ScopeCardInspector {
    fn inspect_lent_scope(&self, root_card_id: CardId) -> (bool, bool, Option<CardId>);
}

/// A scope card kept across invocations, with the client whose default it is
/// and its accessor values at the time it was derived.
struct KeptScopeCard {
    scope_card: ScopeCard,
    client: ScopeCardInspectorClient,
    /// A scope card derived through the raw bindings in the same invocation,
    /// for the raw synchronous call.
    raw_scope_card: raw::Card,
    root_card_id: CardId,
    id: permissions::CardId,
    parents: Vec<permissions::CardId>,
    expires_at: Option<permissions::Timestamp>,
}

pub struct ScopeCardAgentImpl {
    _name: String,
    kept_scope_card: RefCell<Option<KeptScopeCard>>,
}

#[agent_implementation]
impl ScopeCardAgent for ScopeCardAgentImpl {
    fn new(name: String) -> Self {
        Self {
            _name: name,
            kept_scope_card: RefCell::new(None),
        }
    }

    async fn invoke_and_await_scope(&self, target: String) -> (bool, bool, bool, CardId) {
        let scope_card = raw::derive_scope_card();
        let scope_card_id = raw::id(&scope_card);
        let root_card_id = raw::id(&raw::template());
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
        let scope_card = raw::derive_scope_card();
        let scope_card_id = raw::id(&scope_card);
        let root_card_id = raw::id(&raw::template());
        let invocation = scope_card_rpc(target).async_invoke_and_await(
            "inspect_scope",
            encode_parameters(vec![scope_card_id.to_value(), root_card_id.to_value()]),
            Some(&scope_card),
        );
        let (present, parent_matches, inspect_matches) = decode_scope_observation(
            invocation
                .future
                .get()
                .await
                .expect("scope-card async invoke-and-await failed")
                .expect("scope-card observation result is missing"),
        );
        (present, parent_matches, inspect_matches, scope_card_id)
    }

    async fn invoke_scope_after_promise(
        &self,
        target: String,
        release: PromiseId,
    ) -> (bool, bool, CardId) {
        let scope_card = raw::derive_scope_card();
        let scope_card_id = raw::id(&scope_card);
        let root_card_id = raw::id(&raw::template());
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

    async fn repeated_scope_inspection_through_default_scope_card(
        &self,
        target: String,
        repetitions: u32,
    ) -> bool {
        let scope_card = derive_scope_card();
        let mut client = ScopeCardAgentClient::get(target);
        client.set_default_scope_card(&scope_card);
        client.inspect_repeatedly(repetitions).await
    }

    fn invoke_scope_is_denied(&self, target: String) -> bool {
        let scope_card = raw::derive_scope_card();
        matches!(
            scope_card_rpc(target).invoke(
                "inspect_scope",
                encode_parameters(vec![]),
                Some(&scope_card)
            ),
            Err(RpcError::Denied(_))
        )
    }

    fn persistent_scope_is_denied(&self, target: String) -> bool {
        let parent = raw::template();
        matches!(
            scope_card_rpc(target).invoke_and_await(
                "inspect_scope",
                encode_parameters(vec![]),
                Some(&parent),
            ),
            Err(RpcError::Denied(_))
        )
    }

    async fn persistent_async_scope_is_denied(&self, target: String) -> bool {
        let parent = raw::template();
        let invocation = scope_card_rpc(target).async_invoke_and_await(
            "inspect_scope",
            encode_parameters(vec![]),
            Some(&parent),
        );
        matches!(invocation.future.get().await, Err(RpcError::Denied(_)))
    }

    fn schedule_scope(&self, target: String) {
        let scope_card = raw::derive_scope_card();
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
        let scope_card = raw::derive_scope_card();
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

    fn inspect_lent_scope(&self, root_card_id: CardId) -> (bool, bool, Option<CardId>) {
        lent_scope_card_observation(&root_card_id)
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
        let parent = raw::derive_from_wallet();
        let child = raw::derive_child(&parent);
        let parent_id = raw::id(&parent);
        let child_id = raw::id(&child);
        raw::install(child, component_id.clone(), target_agent_id)
            .expect("failed to install derived card");
        raw::install(parent, component_id, caller_agent_id).expect("failed to install parent card");
        (parent_id, child_id)
    }

    async fn derive_and_install_after_promise(
        &self,
        component_id: ComponentId,
        target_agent_id: String,
        release: PromiseId,
    ) -> CardId {
        let card = raw::derive_from_wallet();
        let id = raw::id(&card);
        await_promise(&release).await;
        raw::install(card, component_id, target_agent_id).expect("failed to install replayed card");
        id
    }

    async fn derive_before_promise(&self, release: PromiseId) -> CardId {
        let card = raw::derive_from_wallet();
        let id = raw::id(&card);
        await_promise(&release).await;
        id
    }

    async fn derive_tool_cards_before_promise(
        &self,
        owner: String,
        release: PromiseId,
    ) -> Vec<String> {
        let grant =
            |resource: &str| PatternGrant::new("tool", owner.clone(), "*", "invoke", resource);
        let parent = parent_card();
        let bounds = Bounds {
            lower_positive: vec![grant(". -p=**")],
            lower_negative: vec![grant(". -p=/private/**")],
            upper_positive: vec![grant(". -p=**")],
            upper_negative: vec![grant(". -p=/private/**")],
        };
        let concrete_parent =
            permissions::derive_from_wallet(&lower_positive(grant("*")), None).unwrap();
        let explicit = concrete_parent.derive(&bounds, None).unwrap();
        let wallet = permissions::derive_from_wallet(&bounds, None).unwrap();
        let scope = permissions::derive_scope(&[(&parent).into()], &bounds).unwrap();
        let mut resources = Vec::new();
        for view in [
            explicit.inspect().unwrap(),
            wallet.inspect().unwrap(),
            scope.inspect().unwrap(),
        ] {
            for grants in [
                view.lower_positive,
                view.lower_negative,
                view.upper_positive,
                view.upper_negative,
            ] {
                resources.extend(grants.into_iter().map(|grant| grant.resource_id));
            }
        }
        await_promise(&release).await;
        resources
    }

    fn derive_from_wallet_denial(&self) -> Option<String> {
        let grant = PatternGrant::new("card", "*", "*", "inspect", "*");
        permissions::derive_from_wallet(&lower_positive(grant), None)
            .err()
            .map(|error| format!("{error:?}"))
    }

    fn wallet_card_count(&self) -> u32 {
        permissions::wallet().len() as u32
    }

    fn authorize_repeatedly(&self, repetitions: u32) -> bool {
        let parent = parent_card();
        let grant = retained_inspect_grant(&parent);
        (0..repetitions).all(|_| {
            parent.inspect().is_ok()
                && permissions::derive_from_wallet(&lower_positive(grant.clone()), None).is_ok()
        })
    }

    fn inspect_repeatedly(&self, repetitions: u32) -> bool {
        let parent = parent_card();
        (0..repetitions).all(|_| parent.inspect().is_ok())
    }

    fn revoke_card_by_id(&self, card_id: CardId) -> u32 {
        raw::revoke(raw::wallet_card(&card_id).expect("card to revoke is not installed"))
            .expect("card revocation failed")
    }

    async fn revoke_card_after_promise_is_denied(
        &self,
        card_id: CardId,
        release: PromiseId,
    ) -> bool {
        let card = raw::wallet_card(&card_id).expect("card to revoke is not installed");
        await_promise(&release).await;
        raw::revoke(card).is_err()
    }

    fn has_card(&self, card_id: CardId) -> bool {
        raw::wallet_card(&card_id).is_some()
    }

    async fn typed_card_scenario(
        &self,
        component_id: ComponentId,
        caller_agent_id: String,
        target_agent_id: String,
        target_name: String,
        mismatched_recipient: String,
    ) -> Vec<(String, bool)> {
        let mut checks = Vec::new();
        let mut check = |name: &str, passed: bool| checks.push((name.to_string(), passed));
        let template = parent_card();
        let grant = retained_inspect_grant(&template);
        let target = ScopeCardAgentClient::get(target_name);

        let card = permissions::derive_from_wallet(&lower_positive(grant.clone()), None)
            .expect("wallet derivation failed");
        let card_id = card.id();
        let view = card.inspect().expect("inspection failed");
        check(
            "inspect returns the requested grants",
            view.lower_positive == vec![grant.clone()]
                && view.lower_negative.is_empty()
                && view.upper_positive.is_empty()
                && view.upper_negative.is_empty(),
        );
        check(
            "a card without lifespan has no expiry",
            card.expires_at().is_none(),
        );
        check(
            "a wallet-derived card names its wallet parent",
            card.parents() == vec![template.id()],
        );

        let lifespan = Lifespan::try_from(Duration::from_secs(3600)).unwrap();
        let child = card
            .derive(&lower_positive(grant.clone()), Some(&lifespan))
            .expect("child derivation failed");
        check(
            "a child with lifespan has an expiry",
            child.expires_at().is_some(),
        );
        check(
            "a child names its parent",
            child.parents() == vec![card_id.clone()],
        );
        let template_child = template.derive(&lower_positive(grant.clone()), None);
        check(
            "the wallet's template card derives a resolved child",
            template_child.as_ref().is_ok_and(|child| {
                child.parents() == vec![template.id()]
                    && child
                        .inspect()
                        .is_ok_and(|view| view.lower_positive == vec![grant.clone()])
            }),
        );

        let scope =
            permissions::derive_scope(&[(&template).into()], &lower_positive(grant.clone()))
                .expect("scope derivation failed");
        let (present, inspect_matches, scope_id) = target
            .with_scope_card(&scope)
            .inspect_lent_scope(raw::id(&raw::template()))
            .await;
        check(
            "the callee observes the per-call scope card",
            present && inspect_matches,
        );
        check(
            "the scope card ends with the invocation",
            match scope_id {
                Some(scope_id) => !target.has_card(scope_id).await,
                None => false,
            },
        );

        let target_cards = target.wallet_card_count().await;
        child
            .install(&agent_holder(component_id.clone(), target_agent_id.clone()))
            .expect("install failed");
        check(
            "the target holds the installed card",
            target.wallet_card_count().await == target_cards + 1,
        );
        card.install(&agent_holder(component_id.clone(), caller_agent_id))
            .expect("self install failed");
        let revoked = wallet_card(&card_id)
            .expect("installed card is missing from the wallet")
            .revoke();
        check("revoke cuts the card and its child", revoked == Ok(2));
        check(
            "the target loses the revoked child",
            target.wallet_card_count().await == target_cards,
        );
        check(
            "the caller loses the revoked card",
            wallet_card(&card_id).is_none(),
        );

        let broader = PatternGrant {
            verb: "install".to_string(),
            ..grant.clone()
        };
        check(
            "a lower bound beyond the wallet is too broad",
            matches!(
                permissions::derive_from_wallet(&lower_positive(broader), None),
                Err(PermissionError::LowerBoundTooBroad(_))
            ),
        );
        let mismatched = permissions::derive_from_wallet(
            &lower_positive(PatternGrant {
                recipient: mismatched_recipient,
                ..grant
            }),
            None,
        )
        .expect("mismatched-recipient derivation failed");
        let mismatch = mismatched.install(&agent_holder(component_id, target_agent_id));
        check(
            "installing on a non-recipient agent is a recipient mismatch",
            matches!(mismatch, Err(PermissionError::InstallRecipientMismatch(_))),
        );

        checks
    }

    async fn keep_scope_card(&self, target: String) -> bool {
        let scope_card = derive_scope_card();
        let root_card_id = raw::id(&raw::template());
        let target_id = ScopeCardAgentClient::get(target).get_agent_id();
        let mut client = ScopeCardInspectorClient::bind(&ParsedAgentId::new(target_id))
            .expect("failed to bind the scope-card inspector");
        client.set_default_scope_card(&scope_card);
        let observed = client
            .inspect_lent_scope(root_card_id.clone())
            .await
            .map(|invocation| invocation.value);
        *self.kept_scope_card.borrow_mut() = Some(KeptScopeCard {
            raw_scope_card: raw::derive_scope_card(),
            root_card_id,
            id: scope_card.id(),
            parents: scope_card.parents(),
            expires_at: scope_card.expires_at(),
            scope_card,
            client,
        });
        matches!(observed, Ok((true, true, Some(_))))
    }

    async fn stale_scope_card_observations(&self, target: String) -> Vec<(String, bool)> {
        let KeptScopeCard {
            scope_card,
            client,
            raw_scope_card,
            root_card_id,
            id,
            parents,
            expires_at,
        } = self
            .kept_scope_card
            .borrow_mut()
            .take()
            .expect("no scope card was kept");
        let mut checks = Vec::new();
        checks.push(("id is unchanged".to_string(), scope_card.id() == id));
        checks.push((
            "parents are unchanged".to_string(),
            scope_card.parents() == parents,
        ));
        checks.push((
            "expiry is unchanged".to_string(),
            scope_card.expires_at() == expires_at,
        ));
        checks.push((
            "inspect still works".to_string(),
            scope_card.inspect().is_ok(),
        ));
        checks.push((
            "the stale default scope card is denied".to_string(),
            matches!(
                client.inspect_lent_scope(root_card_id.clone()).await,
                Err(GolemReflectError::Denied(_))
            ),
        ));
        checks.push((
            "a stale per-call scope card is denied".to_string(),
            matches!(
                client
                    .with_scope_card(&scope_card)
                    .inspect_lent_scope(root_card_id.clone())
                    .await,
                Err(GolemReflectError::Denied(_))
            ),
        ));
        checks.push((
            "a stale scope card on a raw synchronous call is denied".to_string(),
            matches!(
                scope_card_rpc(target).invoke_and_await(
                    "inspect_scope",
                    encode_parameters(vec![
                        raw::id(&raw_scope_card).to_value(),
                        root_card_id.to_value()
                    ]),
                    Some(&raw_scope_card),
                ),
                Err(RpcError::Denied(_))
            ),
        ));
        checks.push((
            "a stale scope card is not permitted as a scope parent".to_string(),
            matches!(
                permissions::derive_scope(&[(&scope_card).into()], &Bounds::default()),
                Err(PermissionError::NotPermitted(_))
            ),
        ));
        drop(client);
        drop(scope_card);
        drop(raw_scope_card);
        checks.push((
            "dropping the stale scope card does not trap".to_string(),
            true,
        ));
        checks
    }

    async fn lend_env_scope(
        &self,
        target: String,
        sealed: bool,
        env_owner: String,
    ) -> (Option<CardId>, Option<String>, bool, CardId) {
        let parent = parent_card();
        let template_grant = parent
            .inspect()
            .expect("failed to inspect parent card")
            .lower_positive
            .into_iter()
            .find(|grant| grant.class == "env" && grant.resource_id == LENT_ENV)
            .expect("parent card does not grant the lent environment variable");
        let slot_grant_is_invalid = matches!(
            permissions::derive_scope(&[(&parent).into()], &lower_positive(template_grant.clone())),
            Err(PermissionError::InvalidPattern(_))
        );
        let grant = PatternGrant {
            owner: format!("{env_owner}/*/*"),
            ..template_grant
        };
        let scope_card = permissions::derive_scope(&[(&parent).into()], &lower_positive(grant))
            .expect("scope-card derivation failed");
        let root_card_id = raw::id(&raw::template());
        let (listed, value) = if sealed {
            SealedScopeAgentClient::get(target)
                .with_scope_card(&scope_card)
                .lent_scope_observation(root_card_id.clone())
                .await
        } else {
            ScopeCardAgentClient::get(target)
                .with_scope_card(&scope_card)
                .lent_scope_observation(root_card_id.clone())
                .await
        };
        (listed, value, slot_grant_is_invalid, root_card_id)
    }

    fn lent_scope_observation(&self, root_card_id: CardId) -> (Option<CardId>, Option<String>) {
        lent_scope_observation(&root_card_id)
    }

    async fn lend_scope_to_observer(&self, target: String) -> (bool, bool) {
        let scope_card = derive_scope_card();
        SealedScopeAgentClient::get(target)
            .with_scope_card(&scope_card)
            .observe_lent_scope(raw::id(&raw::template()))
            .await
    }

    fn use_moved_card(&self) -> String {
        let card = derive_persistent_card_from_wallet();
        // Decoding takes the handle out of `card`, as sending it to another agent does.
        let _decoded = PermissionCard::from_value(&card.to_value()).expect("card decodes");
        format!("{:?}", card.id())
    }
}

/// The environment variable that `ScopeCardAgent::lend_env_scope` lends the
/// read of through a scope card.
const LENT_ENV: &str = "LENT_ENV";

/// Reads the environment through the host on every call; the std
/// environment is read once and cached by the guest.
fn lent_scope_observation(root_card_id: &CardId) -> (Option<CardId>, Option<String>) {
    let listed = raw::children_of(root_card_id).first().map(raw::id);
    let value = golem_rust::wasip3::cli::environment::get_environment()
        .into_iter()
        .find(|(name, _)| name == LENT_ENV)
        .map(|(_, value)| value);
    (listed, value)
}

/// A scope-card callee whose provisioning decides whether it may inspect
/// cards.
#[agent_definition]
pub trait SealedScopeAgent {
    fn new(name: String) -> Self;

    /// The id of a scope card lent by an agent whose initial card is
    /// `root_card_id`, if the wallet lists it, and the value of [`LENT_ENV`]
    /// visible to this agent.
    fn lent_scope_observation(&self, root_card_id: CardId) -> (Option<CardId>, Option<String>);

    /// Whether the wallet lists a scope card lent by an agent whose initial
    /// card is `root_card_id`, and whether every such handle is refused as a
    /// `derive-scope` parent.
    fn observe_lent_scope(&self, root_card_id: CardId) -> (bool, bool);
}

pub struct SealedScopeAgentImpl {
    _name: String,
}

#[agent_implementation]
impl SealedScopeAgent for SealedScopeAgentImpl {
    fn new(name: String) -> Self {
        Self { _name: name }
    }

    fn lent_scope_observation(&self, root_card_id: CardId) -> (Option<CardId>, Option<String>) {
        lent_scope_observation(&root_card_id)
    }

    fn observe_lent_scope(&self, root_card_id: CardId) -> (bool, bool) {
        let listed = raw::children_of(&root_card_id);
        let parent_refused = listed.iter().all(|card| {
            matches!(
                raw::derive_scope(&[card]),
                Err(raw::PermissionError::NotPermitted(_))
            )
        });
        (!listed.is_empty(), parent_refused)
    }
}

#[agent_definition(ephemeral)]
pub trait EphemeralScopeCardAgent {
    fn new(name: String) -> Self;

    fn revoke_card_by_id(&self, card_id: CardId) -> u32;
}

pub struct EphemeralScopeCardAgentImpl {
    _name: String,
}

#[agent_implementation]
impl EphemeralScopeCardAgent for EphemeralScopeCardAgentImpl {
    fn new(name: String) -> Self {
        Self { _name: name }
    }

    fn revoke_card_by_id(&self, card_id: CardId) -> u32 {
        raw::revoke(raw::wallet_card(&card_id).expect("card to revoke is not installed"))
            .expect("card revocation failed")
    }
}
