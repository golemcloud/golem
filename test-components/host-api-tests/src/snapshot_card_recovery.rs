use golem_rust::bindings::golem::agent::host::{Datetime, RpcError, WasmRpc};
use golem_rust::golem_schema::CardId;
use golem_rust::permissions::{
    self, Bounds, Holder, Lifespan, PatternGrant, PermissionCard, PermissionError, ScopeCard,
};
use golem_rust::schema::wit::GuestPermissionCardHandle;
use golem_rust::schema::wit::wire::PermissionCard as RawPermissionCard;
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

/// Lends the host handle of `card` to a raw `WasmRpc` call. The typed agent
/// client attaches scope cards only to awaited calls, so the raw handle is
/// needed to show that the host also accepts the synchronous await variant and
/// rejects the other invocation variants.
fn with_raw_card<R>(card: &PermissionCard, f: impl FnOnce(&RawPermissionCard) -> R) -> R {
    GuestPermissionCardHandle::from_value(&card.to_value())
        .expect("permission card lowers to a card handle")
        .with_handle(f)
        .expect("permission card has already been moved")
}

/// Lends the host handle of a scope card to a raw `WasmRpc` call, for the
/// same reason as [`with_raw_card`].
fn with_raw_scope<R>(scope_card: &ScopeCard, f: impl FnOnce(&RawPermissionCard) -> R) -> R {
    f(scope_card.__raw_handle_for_host_api_tests())
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

fn derive_persistent_card(parent: &PermissionCard) -> PermissionCard {
    let grant = retained_inspect_grant(parent);
    parent
        .derive(&lower_positive(grant), None)
        .expect("persistent card derivation failed")
}

fn derive_persistent_card_from_wallet() -> PermissionCard {
    let parent = parent_card();
    let grant = retained_inspect_grant(&parent);
    permissions::derive_from_wallet(&lower_positive(grant), None)
        .expect("wallet card derivation failed")
}

fn wallet_card(card_id: &CardId) -> Option<PermissionCard> {
    permissions::wallet()
        .into_iter()
        .find(|card| &reveal(card.id()) == card_id)
}

fn agent_holder(component_id: ComponentId, agent_id: String) -> Holder {
    Holder::Agent(AgentId::new(component_id, agent_id))
}

/// Card ids cross agent calls in their schema form; `permissions::CardId`
/// itself has no codec.
fn reveal(id: permissions::CardId) -> CardId {
    id.__reveal_for_tests()
}

fn scope_root_id(scope_card: &ScopeCard) -> CardId {
    reveal(
        scope_card
            .parents()
            .into_iter()
            .next()
            .expect("scope card has no root"),
    )
}

fn scope_card_observation(scope_card_id: &CardId, root_card_id: &CardId) -> (bool, bool, bool) {
    let Some(card) = wallet_card(scope_card_id) else {
        return (false, false, false);
    };

    let parent_matches = card
        .parents()
        .into_iter()
        .any(|parent| &reveal(parent) == root_card_id);
    let inspect_matches = card
        .inspect()
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

    /// Lends a scope card that grants only the read of [`LENT_ENV`] to a
    /// `SealedScopeAgent` (`sealed`) or a `ScopeCardAgent` target, and returns
    /// the target's [`lent_scope_observation`] and the scope card id.
    async fn lend_env_scope(&self, target: String, sealed: bool) -> (bool, Option<String>, CardId);

    /// Whether the wallet lists `scope_card_id`, and the value of
    /// [`LENT_ENV`] visible to this agent.
    fn lent_scope_observation(&self, scope_card_id: CardId) -> (bool, Option<String>);

    /// Lends a scope card to a `SealedScopeAgent` target, which lists it;
    /// see `SealedScopeAgent::observe_lent_scope`.
    async fn lend_scope_to_observer(&self, target: String) -> (bool, bool);
}

/// A method-only client of `ScopeCardAgent`, whose awaited calls return RPC
/// errors instead of trapping.
#[agent_client]
pub trait ScopeCardInspector {
    fn inspect_scope(&self, scope_card_id: CardId, root_card_id: CardId) -> (bool, bool, bool);
}

/// A scope card kept across invocations, with the client whose default it is
/// and its accessor values at the time it was derived.
struct KeptScopeCard {
    scope_card: ScopeCard,
    client: ScopeCardInspectorClient,
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
        let scope_card = derive_scope_card();
        let scope_card_id = reveal(scope_card.id());
        let root_card_id = scope_root_id(&scope_card);
        let invocation = with_raw_scope(&scope_card, |raw| {
            scope_card_rpc(target).invoke_and_await(
                "inspect_scope",
                encode_parameters(vec![scope_card_id.to_value(), root_card_id.to_value()]),
                Some(raw),
            )
        })
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
        let scope_card_id = reveal(scope_card.id());
        let root_card_id = scope_root_id(&scope_card);
        let (present, parent_matches, inspect_matches) = ScopeCardAgentClient::get(target)
            .with_scope_card(&scope_card)
            .inspect_scope(scope_card_id.clone(), root_card_id)
            .await;
        (present, parent_matches, inspect_matches, scope_card_id)
    }

    async fn invoke_scope_after_promise(
        &self,
        target: String,
        release: PromiseId,
    ) -> (bool, bool, CardId) {
        let scope_card = derive_scope_card();
        let scope_card_id = reveal(scope_card.id());
        let root_card_id = scope_root_id(&scope_card);
        let invocation = with_raw_scope(&scope_card, |raw| {
            scope_card_rpc(target).invoke_and_await(
                "inspect_scope_after_promise",
                encode_parameters(vec![
                    scope_card_id.to_value(),
                    root_card_id.to_value(),
                    release.to_value(),
                ]),
                Some(raw),
            )
        })
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
        let scope_card = derive_scope_card();
        with_raw_scope(&scope_card, |raw| {
            matches!(
                scope_card_rpc(target).invoke(
                    "inspect_scope",
                    encode_parameters(vec![]),
                    Some(raw)
                ),
                Err(RpcError::Denied(_))
            )
        })
    }

    fn persistent_scope_is_denied(&self, target: String) -> bool {
        let parent = parent_card();
        with_raw_card(&parent, |raw| {
            matches!(
                scope_card_rpc(target).invoke_and_await(
                    "inspect_scope",
                    encode_parameters(vec![]),
                    Some(raw),
                ),
                Err(RpcError::Denied(_))
            )
        })
    }

    async fn persistent_async_scope_is_denied(&self, target: String) -> bool {
        let parent = parent_card();
        let invocation = with_raw_card(&parent, |raw| {
            scope_card_rpc(target).async_invoke_and_await(
                "inspect_scope",
                encode_parameters(vec![]),
                Some(raw),
            )
        });
        matches!(invocation.future.get().await, Err(RpcError::Denied(_)))
    }

    fn schedule_scope(&self, target: String) {
        let scope_card = derive_scope_card();
        with_raw_scope(&scope_card, |raw| {
            scope_card_rpc(target).schedule_invocation(
                Datetime {
                    seconds: 0,
                    nanoseconds: 0,
                },
                "inspect_scope",
                encode_parameters(vec![]),
                Some(raw),
            )
        })
        .unwrap();
    }

    fn schedule_cancelable_scope(&self, target: String) {
        let scope_card = derive_scope_card();
        with_raw_scope(&scope_card, |raw| {
            scope_card_rpc(target).schedule_cancelable_invocation(
                Datetime {
                    seconds: 0,
                    nanoseconds: 0,
                },
                "inspect_scope",
                encode_parameters(vec![]),
                Some(raw),
            )
        })
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
        let parent_id = reveal(parent.id());
        let child_id = reveal(child.id());
        child
            .install(&agent_holder(component_id.clone(), target_agent_id))
            .expect("failed to install derived card");
        parent
            .install(&agent_holder(component_id, caller_agent_id))
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
        let id = reveal(card.id());
        await_promise(&release).await;
        card.install(&agent_holder(component_id, target_agent_id))
            .expect("failed to install replayed card");
        id
    }

    async fn derive_before_promise(&self, release: PromiseId) -> CardId {
        let card = derive_persistent_card_from_wallet();
        let id = reveal(card.id());
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
        wallet_card(&card_id)
            .expect("card to revoke is not installed")
            .revoke()
            .expect("card revocation failed")
    }

    async fn revoke_card_after_promise_is_denied(
        &self,
        card_id: CardId,
        release: PromiseId,
    ) -> bool {
        let card = wallet_card(&card_id).expect("card to revoke is not installed");
        await_promise(&release).await;
        card.revoke().is_err()
    }

    fn has_card(&self, card_id: CardId) -> bool {
        wallet_card(&card_id).is_some()
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
        let child_id = reveal(child.id());
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
        let scope_id = reveal(scope.id());
        check(
            "the callee observes the per-call scope card",
            target
                .with_scope_card(&scope)
                .inspect_scope(scope_id.clone(), reveal(template.id()))
                .await
                == (true, true, true),
        );
        check(
            "the scope card ends with the invocation",
            !target.has_card(scope_id.clone()).await,
        );

        child
            .install(&agent_holder(component_id.clone(), target_agent_id.clone()))
            .expect("install failed");
        check(
            "the target holds the installed card",
            target.has_card(child_id.clone()).await,
        );
        card.install(&agent_holder(component_id.clone(), caller_agent_id))
            .expect("self install failed");
        let revoked = wallet_card(&reveal(card_id.clone()))
            .expect("installed card is missing from the wallet")
            .revoke();
        check("revoke cuts the card and its child", revoked == Ok(2));
        check(
            "the target loses the revoked child",
            !target.has_card(child_id).await,
        );
        check(
            "the caller loses the revoked card",
            wallet_card(&reveal(card_id.clone())).is_none(),
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
        let root_card_id = scope_root_id(&scope_card);
        let target_id = ScopeCardAgentClient::get(target).get_agent_id();
        let mut client = ScopeCardInspectorClient::bind(&ParsedAgentId::new(target_id))
            .expect("failed to bind the scope-card inspector");
        client.set_default_scope_card(&scope_card);
        let observed = client
            .inspect_scope(reveal(scope_card.id()), root_card_id)
            .await
            .map(|invocation| invocation.value);
        *self.kept_scope_card.borrow_mut() = Some(KeptScopeCard {
            id: scope_card.id(),
            parents: scope_card.parents(),
            expires_at: scope_card.expires_at(),
            scope_card,
            client,
        });
        matches!(observed, Ok((true, true, true)))
    }

    async fn stale_scope_card_observations(&self, target: String) -> Vec<(String, bool)> {
        let KeptScopeCard {
            scope_card,
            client,
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
        let scope_card_id = reveal(scope_card.id());
        let root_card_id = scope_root_id(&scope_card);
        checks.push((
            "inspect still works".to_string(),
            scope_card.inspect().is_ok(),
        ));
        checks.push((
            "the stale default scope card is denied".to_string(),
            matches!(
                client
                    .inspect_scope(scope_card_id.clone(), root_card_id.clone())
                    .await,
                Err(GolemReflectError::Denied(_))
            ),
        ));
        checks.push((
            "a stale per-call scope card is denied".to_string(),
            matches!(
                client
                    .with_scope_card(&scope_card)
                    .inspect_scope(scope_card_id.clone(), root_card_id.clone())
                    .await,
                Err(GolemReflectError::Denied(_))
            ),
        ));
        checks.push((
            "a stale scope card on a raw synchronous call is denied".to_string(),
            with_raw_scope(&scope_card, |raw| {
                matches!(
                    scope_card_rpc(target).invoke_and_await(
                        "inspect_scope",
                        encode_parameters(vec![scope_card_id.to_value(), root_card_id.to_value()]),
                        Some(raw),
                    ),
                    Err(RpcError::Denied(_))
                )
            }),
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
        checks.push((
            "dropping the stale scope card does not trap".to_string(),
            true,
        ));
        checks
    }

    async fn lend_env_scope(&self, target: String, sealed: bool) -> (bool, Option<String>, CardId) {
        let parent = parent_card();
        let grant = parent
            .inspect()
            .expect("failed to inspect parent card")
            .lower_positive
            .into_iter()
            .find(|grant| grant.class == "env" && grant.resource_id == LENT_ENV)
            .expect("parent card does not grant the lent environment variable");
        let scope_card = permissions::derive_scope(&[(&parent).into()], &lower_positive(grant))
            .expect("scope-card derivation failed");
        let scope_card_id = reveal(scope_card.id());
        let (listed, value) = if sealed {
            SealedScopeAgentClient::get(target)
                .with_scope_card(&scope_card)
                .lent_scope_observation(scope_card_id.clone())
                .await
        } else {
            ScopeCardAgentClient::get(target)
                .with_scope_card(&scope_card)
                .lent_scope_observation(scope_card_id.clone())
                .await
        };
        (listed, value, scope_card_id)
    }

    fn lent_scope_observation(&self, scope_card_id: CardId) -> (bool, Option<String>) {
        lent_scope_observation(&scope_card_id)
    }

    async fn lend_scope_to_observer(&self, target: String) -> (bool, bool) {
        let scope_card = derive_scope_card();
        SealedScopeAgentClient::get(target)
            .with_scope_card(&scope_card)
            .observe_lent_scope(reveal(scope_card.id()))
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
fn lent_scope_observation(scope_card_id: &CardId) -> (bool, Option<String>) {
    let listed = permissions::wallet()
        .into_iter()
        .any(|card| &reveal(card.id()) == scope_card_id);
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

    /// Whether the wallet lists `scope_card_id`, and the value of
    /// [`LENT_ENV`] visible to this agent.
    fn lent_scope_observation(&self, scope_card_id: CardId) -> (bool, Option<String>);

    /// Whether the wallet lists the lent scope card `scope_card_id`, and
    /// whether every wallet handle with that id is refused as a
    /// `derive-scope` parent.
    fn observe_lent_scope(&self, scope_card_id: CardId) -> (bool, bool);
}

pub struct SealedScopeAgentImpl {
    _name: String,
}

#[agent_implementation]
impl SealedScopeAgent for SealedScopeAgentImpl {
    fn new(name: String) -> Self {
        Self { _name: name }
    }

    fn lent_scope_observation(&self, scope_card_id: CardId) -> (bool, Option<String>) {
        lent_scope_observation(&scope_card_id)
    }

    fn observe_lent_scope(&self, scope_card_id: CardId) -> (bool, bool) {
        let listed = permissions::wallet()
            .into_iter()
            .filter(|card| reveal(card.id()) == scope_card_id)
            .collect::<Vec<_>>();
        let parent_refused = listed.iter().all(|card| {
            matches!(
                permissions::derive_scope(&[card.into()], &Bounds::default()),
                Err(PermissionError::NotPermitted(_))
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
        wallet_card(&card_id)
            .expect("card to revoke is not installed")
            .revoke()
            .expect("card revocation failed")
    }
}
