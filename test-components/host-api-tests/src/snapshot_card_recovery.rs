use golem_rust::bindings::golem::agent::host::{Datetime, RpcError, WasmRpc};
use golem_rust::golem_schema::CardId;
use golem_rust::permissions::{
    self, Bounds, Holder, Lifespan, PatternGrant, PermissionCard, PermissionError,
    PolymorphicPermissionCard, ScopeCard, Timestamp, WalletCard,
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
    pub use types::{PatternGrant, PermissionError, WalletCard};

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

    pub fn is_inspect_grant(grant: &types::PatternGrant) -> bool {
        grant.class == "card" && grant.verb == "inspect" && grant.resource_id == "*"
    }

    /// The cards of the caller's wallet with the kind that `self-wallet`
    /// labels each with: `concrete`, `template` or `scope`.
    pub fn labelled_wallet() -> Vec<(&'static str, Card)> {
        wallet::self_wallet()
            .into_iter()
            .map(|card| match card {
                WalletCard::Concrete(card) => ("concrete", card),
                WalletCard::Template(card) => ("template", card),
                WalletCard::Scope(card) => ("scope", card),
            })
            .collect()
    }

    fn wallet_cards() -> impl Iterator<Item = Card> {
        labelled_wallet().into_iter().map(|(_, card)| card)
    }

    /// The caller's initial card, which is the root of every card derived
    /// from the wallet.
    pub fn template() -> Card {
        labelled_wallet()
            .into_iter()
            .find_map(|(kind, card)| (kind == "template").then_some(card))
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

    /// Derives a child of `parent` that retains its card inspection only for `recipient`.
    pub fn derive_child_for_recipient(parent: &Card, recipient: &str) -> Card {
        let grant = types::PatternGrant {
            recipient: recipient.to_string(),
            ..inspect_grant(parent)
        };
        derive::derive(parent, &[grant], &[], &[], &[], None)
            .expect("recipient-restricted card derivation failed")
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

    /// Derives a child of `parent` that retains its card inspection and lives for `lifespan`, an
    /// ISO-8601 duration.
    pub fn derive_child_with_lifespan(parent: &Card, lifespan: Option<String>) -> Card {
        let grant = inspect_grant(parent);
        let lifespan = lifespan.map(|iso_8601| types::Duration { iso_8601 });
        derive::derive(parent, &[grant], &[], &[], &[], lifespan.as_ref())
            .expect("persistent card derivation failed")
    }

    /// Derives a card from the wallet that grants exactly `grants` in its lower bound.
    pub fn derive_from_wallet_granting(
        grants: &[types::PatternGrant],
    ) -> Result<Card, PermissionError> {
        derive::derive_from_wallet(grants, &[], &[], &[], None)
    }

    /// Derives a child of `parent` that retains none of its grants.
    pub fn try_derive_empty_child(parent: &Card) -> Result<Card, PermissionError> {
        derive::derive(parent, &[], &[], &[], &[], None)
    }

    pub fn error_kind(error: &PermissionError) -> &'static str {
        match error {
            PermissionError::NotPermitted(_) => "not-permitted",
            PermissionError::LowerBoundTooBroad(_) => "lower-bound-too-broad",
            PermissionError::UpperBoundTooBroad(_) => "upper-bound-too-broad",
            PermissionError::WalletMultiSourceRequired(_) => "wallet-multi-source-required",
            PermissionError::CardRevoked(_) => "card-revoked",
            PermissionError::CardExpired(_) => "card-expired",
            PermissionError::InstallRecipientMismatch(_) => "install-recipient-mismatch",
            PermissionError::UnknownResourceClass(_) => "unknown-resource-class",
            PermissionError::InvalidPattern(_) => "invalid-pattern",
            PermissionError::InvalidVerb(_) => "invalid-verb",
            PermissionError::InvalidOwner(_) => "invalid-owner",
            PermissionError::InvalidRecipient(_) => "invalid-recipient",
        }
    }

    /// `ok`, or the kind of the permission error.
    pub fn outcome<T>(result: &Result<T, PermissionError>) -> String {
        match result {
            Ok(_) => "ok".to_string(),
            Err(error) => error_kind(error).to_string(),
        }
    }

    pub fn wallet_card(card_id: &CardId) -> Option<Card> {
        wallet_cards().find(|card| &id(card) == card_id)
    }

    /// The wallet cards derived from `root`, such as a scope card lent to
    /// this invocation by an agent whose initial card is `root`.
    pub fn children_of(root: &CardId) -> Vec<Card> {
        wallet_cards()
            .filter(|card| parents(card).contains(root))
            .collect()
    }
}

/// The agent's initial card, a template card.
fn parent_card() -> PolymorphicPermissionCard {
    permissions::wallet()
        .into_iter()
        .find_map(|card| match card {
            WalletCard::Template(card) => Some(card),
            _ => None,
        })
        .expect("scope-card test parent is not installed")
}

fn is_inspect_grant(grant: &PatternGrant) -> bool {
    grant.class == "card" && grant.verb == "inspect" && grant.resource_id == "*"
}

fn retained_inspect_grant(card: &PolymorphicPermissionCard) -> PatternGrant {
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

fn wallet_card(card_id: &permissions::CardId) -> Option<WalletCard> {
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

    /// Reads the wallet `reads` times with no other host call between the
    /// reads, keeps the card count of each read in agent state and returns
    /// them.
    fn remember_wallet_card_counts(&self, reads: u32) -> Vec<u64>;

    /// Reads the wallet version `reads` times with no other host call between
    /// the reads, keeps the generation of each read in agent state and returns
    /// them.
    fn remember_wallet_generations(&self, reads: u32) -> Vec<u64>;

    /// The observations kept by the last `remember_wallet_*` call.
    fn remembered_wallet_observations(&self) -> Vec<u64>;

    /// Makes the card operation `operation` twice and keeps the outcome of each call in agent
    /// state: `ok`, or the kind of the permission error. Between the two calls the agent only logs
    /// `CARD_DECISION_GAP_MARKER`, so no recorded entry separates the two calls. `install-card`
    /// inspects and then installs cards restricted to a foreign recipient on `target_agent_id` of
    /// `component_id`.
    fn remember_card_decisions_around_gap(
        &self,
        operation: String,
        component_id: ComponentId,
        target_agent_id: String,
    ) -> Vec<String>;

    /// The outcomes kept by the last `remember_card_decisions_around_gap` call.
    fn remembered_card_decisions(&self) -> Vec<String>;

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

    /// This agent's wallet; see [`labelled_wallet`].
    fn labelled_wallet(&self) -> LabelledWallet;

    /// Lends a scope card derived from this agent's initial card to the
    /// `ScopeCardAgent` `target` and returns the target's
    /// [`labelled_wallet`] during that call.
    async fn lend_scope_to_wallet_lister(&self, target: String) -> LabelledWallet;

    /// Derives a scope card and lends it to the `ScopeCardAgent` `target`.
    /// The parents are, in this order, the concrete wallet cards that expire
    /// at `parent_expiries`, then the initial card. Returns the earliest of
    /// `parent_expiries`, the expiry of the scope card, and the target's
    /// [`lent_scope_expiry`].
    async fn expiring_scope_expiries(
        &self,
        target: String,
        parent_expiries: Vec<Expiry>,
    ) -> (Option<Expiry>, Option<Expiry>, (bool, Option<Expiry>));

    /// Whether the typed wallet lists a scope card lent to this invocation,
    /// and the expiry of that scope card.
    fn lent_scope_expiry(&self) -> (bool, Option<Expiry>);

    /// Derives a card from the wallet and a child of it that lives for `child_lifespan`, an
    /// ISO-8601 duration, and keeps both handles in agent state. Returns the ids of the parent
    /// and the child.
    fn keep_derived_cards(&self, child_lifespan: Option<String>) -> (CardId, CardId);

    /// Revokes the kept parent card and returns the number of revoked cards.
    fn revoke_kept_parent(&self) -> u32;

    /// Uses the kept child as `inspect-card` and as the parent of `derive`, and returns the
    /// outcome of each: `ok`, or the kind of the permission error.
    fn use_kept_child(&self) -> Vec<String>;

    /// Installs the kept child on `target_agent_id` and returns the outcome.
    fn install_kept_child(&self, component_id: ComponentId, target_agent_id: String) -> String;

    /// The id of the kept child, read through the `id` accessor.
    fn kept_child_id(&self) -> CardId;

    /// Derives a card from the wallet and sends it to the `ScopeCardAgent` `target`, which
    /// uses it; returns the outcomes that `use_received_card` reports.
    async fn send_derived_card(&self, target: String) -> Vec<String>;

    /// Uses a received card as `inspect` and as the parent of `derive`, and returns the outcome
    /// of each.
    fn use_received_card(&self, card: PermissionCard) -> Vec<String>;
}

/// The cards that `keep_derived_cards` keeps across invocations.
#[derive(Default)]
struct KeptCards {
    parent: Option<raw::Card>,
    child: Option<raw::Card>,
}

fn typed_outcome<T>(result: &Result<T, PermissionError>) -> String {
    match result {
        Ok(_) => "ok".to_string(),
        Err(PermissionError::CardRevoked(_)) => "card-revoked".to_string(),
        Err(PermissionError::CardExpired(_)) => "card-expired".to_string(),
        Err(PermissionError::NotPermitted(_)) => "not-permitted".to_string(),
        Err(error) => format!("{error:?}"),
    }
}

/// The cards of a wallet as the raw `self-wallet` labels them, each with its
/// id and parents, and the kinds the typed `permissions::wallet` reports for
/// the same wallet, in order.
type LabelledWallet = (Vec<(String, CardId, Vec<CardId>)>, Vec<String>);

/// A card expiry as seconds and nanoseconds since the Unix epoch.
type Expiry = (i64, u32);

fn labelled_wallet() -> LabelledWallet {
    let listed = raw::labelled_wallet()
        .into_iter()
        .map(|(kind, card)| (kind.to_string(), raw::id(&card), raw::parents(&card)))
        .collect();
    let typed_kinds = permissions::wallet()
        .iter()
        .map(|card| {
            match card {
                WalletCard::Concrete(_) => "concrete",
                WalletCard::Template(_) => "template",
                WalletCard::Scope(_) => "scope",
            }
            .to_string()
        })
        .collect();
    (listed, typed_kinds)
}

fn expiry(timestamp: Timestamp) -> Expiry {
    (timestamp.seconds, timestamp.nanoseconds)
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
    wallet_observations: RefCell<Vec<u64>>,
    card_decisions: RefCell<Vec<String>>,
    kept_cards: RefCell<KeptCards>,
}

/// A recipient that no agent of the test environment matches, so installing a card restricted to
/// it on any agent is a recipient mismatch.
const CARD_DECISION_FOREIGN_RECIPIENT: &str = "nobody@example.com/*/*/*/*";

/// The message that `remember_card_decisions_around_gap` logs between its two calls.
const CARD_DECISION_GAP_MARKER: &str = "card-decision-gap";

#[agent_implementation]
impl ScopeCardAgent for ScopeCardAgentImpl {
    fn new(name: String) -> Self {
        Self {
            _name: name,
            kept_scope_card: RefCell::new(None),
            wallet_observations: RefCell::new(Vec::new()),
            card_decisions: RefCell::new(Vec::new()),
            kept_cards: RefCell::new(KeptCards::default()),
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

    fn remember_wallet_card_counts(&self, reads: u32) -> Vec<u64> {
        let counts = (0..reads)
            .map(|_| permissions::wallet().len() as u64)
            .collect::<Vec<_>>();
        *self.wallet_observations.borrow_mut() = counts.clone();
        counts
    }

    fn remember_wallet_generations(&self, reads: u32) -> Vec<u64> {
        let generations = (0..reads)
            .map(|_| permissions::wallet_version().generation)
            .collect::<Vec<_>>();
        *self.wallet_observations.borrow_mut() = generations.clone();
        generations
    }

    fn remembered_wallet_observations(&self) -> Vec<u64> {
        self.wallet_observations.borrow().clone()
    }

    fn remember_card_decisions_around_gap(
        &self,
        operation: String,
        component_id: ComponentId,
        target_agent_id: String,
    ) -> Vec<String> {
        let mut decide: Box<dyn FnMut() -> String> = match operation.as_str() {
            "derive-scope" => {
                let scope = raw::derive_scope_card();
                Box::new(move || raw::outcome(&raw::derive_scope(&[&scope])))
            }
            "inspect-card" => {
                let template = raw::template();
                Box::new(move || raw::outcome(&raw::view(&template)))
            }
            "derive" => {
                let template = raw::template();
                Box::new(move || raw::outcome(&raw::try_derive_empty_child(&template)))
            }
            "derive-from-wallet" => {
                let grants = ["inspect", "derive"].map(|verb| raw::PatternGrant {
                    class: "card".to_string(),
                    owner: "*".to_string(),
                    recipient: "*".to_string(),
                    verb: verb.to_string(),
                    resource_id: "*".to_string(),
                });
                Box::new(move || raw::outcome(&raw::derive_from_wallet_granting(&grants)))
            }
            "install-card" => {
                let template = raw::template();
                let mut cards = vec![
                    raw::derive_child_for_recipient(&template, CARD_DECISION_FOREIGN_RECIPIENT),
                    raw::derive_child_for_recipient(&template, CARD_DECISION_FOREIGN_RECIPIENT),
                ];
                Box::new(move || {
                    let card = cards.remove(0);
                    // The inspection applies a pending card event at its authority boundary,
                    // before any recorded entry of this call.
                    raw::view(&card).expect("failed to inspect the card to install");
                    raw::outcome(&raw::install(
                        card,
                        component_id.clone(),
                        target_agent_id.clone(),
                    ))
                })
            }
            operation => panic!("unknown card operation {operation}"),
        };
        let first = decide();
        golem_rust::bindings::wasi::logging::logging::log(
            golem_rust::bindings::wasi::logging::logging::Level::Info,
            "",
            CARD_DECISION_GAP_MARKER,
        );
        let second = decide();
        let outcomes = vec![first, second];
        *self.card_decisions.borrow_mut() = outcomes.clone();
        outcomes
    }

    fn remembered_card_decisions(&self) -> Vec<String> {
        self.card_decisions.borrow().clone()
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
        let Some(WalletCard::Concrete(installed)) = wallet_card(&card_id) else {
            panic!("the installed card is missing from the wallet or is not concrete");
        };
        let revoked = installed.revoke();
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

    fn labelled_wallet(&self) -> LabelledWallet {
        labelled_wallet()
    }

    async fn lend_scope_to_wallet_lister(&self, target: String) -> LabelledWallet {
        let scope_card = derive_scope_card();
        ScopeCardAgentClient::get(target)
            .with_scope_card(&scope_card)
            .labelled_wallet()
            .await
    }

    async fn expiring_scope_expiries(
        &self,
        target: String,
        parent_expiries: Vec<Expiry>,
    ) -> (Option<Expiry>, Option<Expiry>, (bool, Option<Expiry>)) {
        let template = parent_card();
        let grant = retained_inspect_grant(&template);
        let mut concrete = permissions::wallet()
            .into_iter()
            .filter_map(|card| match card {
                WalletCard::Concrete(card) => Some(card),
                _ => None,
            })
            .collect::<Vec<_>>();
        let expiring = parent_expiries
            .iter()
            .map(|wanted| {
                let position = concrete
                    .iter()
                    .position(|card| card.expires_at().map(expiry) == Some(*wanted))
                    .expect("no concrete wallet card expires at a requested parent expiry");
                concrete.swap_remove(position)
            })
            .collect::<Vec<_>>();
        let earliest = parent_expiries.iter().copied().min();
        let mut parents = expiring.iter().map(Into::into).collect::<Vec<_>>();
        parents.push((&template).into());
        let scope_card = permissions::derive_scope(&parents, &lower_positive(grant))
            .expect("scope-card derivation failed");
        let lent = ScopeCardAgentClient::get(target)
            .with_scope_card(&scope_card)
            .lent_scope_expiry()
            .await;
        (earliest, scope_card.expires_at().map(expiry), lent)
    }

    fn lent_scope_expiry(&self) -> (bool, Option<Expiry>) {
        let scope_card = permissions::wallet()
            .into_iter()
            .find_map(|card| match card {
                WalletCard::Scope(card) => Some(card),
                _ => None,
            });
        match scope_card {
            Some(scope_card) => (true, scope_card.expires_at().map(expiry)),
            None => (false, None),
        }
    }

    fn keep_derived_cards(&self, child_lifespan: Option<String>) -> (CardId, CardId) {
        let parent = raw::derive_from_wallet();
        let child = raw::derive_child_with_lifespan(&parent, child_lifespan);
        let ids = (raw::id(&parent), raw::id(&child));
        *self.kept_cards.borrow_mut() = KeptCards {
            parent: Some(parent),
            child: Some(child),
        };
        ids
    }

    fn revoke_kept_parent(&self) -> u32 {
        let parent = self
            .kept_cards
            .borrow_mut()
            .parent
            .take()
            .expect("no kept parent card");
        raw::revoke(parent).expect("failed to revoke the kept parent card")
    }

    fn use_kept_child(&self) -> Vec<String> {
        let kept = self.kept_cards.borrow();
        let child = kept.child.as_ref().expect("no kept child card");
        vec![
            raw::outcome(&raw::view(child)),
            raw::outcome(&raw::try_derive_empty_child(child)),
        ]
    }

    fn install_kept_child(&self, component_id: ComponentId, target_agent_id: String) -> String {
        let child = self
            .kept_cards
            .borrow_mut()
            .child
            .take()
            .expect("no kept child card");
        raw::outcome(&raw::install(child, component_id, target_agent_id))
    }

    fn kept_child_id(&self) -> CardId {
        let kept = self.kept_cards.borrow();
        raw::id(kept.child.as_ref().expect("no kept child card"))
    }

    async fn send_derived_card(&self, target: String) -> Vec<String> {
        let card = derive_persistent_card_from_wallet();
        ScopeCardAgentClient::get(target)
            .use_received_card(card)
            .await
    }

    fn use_received_card(&self, card: PermissionCard) -> Vec<String> {
        vec![
            typed_outcome(&card.inspect()),
            typed_outcome(&card.derive(&Bounds::default(), None)),
        ]
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
