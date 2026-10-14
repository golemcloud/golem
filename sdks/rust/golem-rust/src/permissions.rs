// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Typed permission-card management over the `golem:permissions` WIT interfaces.
//!
//! A [`PermissionCard`] is an unforgeable capability held by the host. Guest code
//! can only obtain one from the host ([`wallet`], [`derive_from_wallet`],
//! [`PermissionCard::derive`]) or receive one as an agent method argument.
//! A [`ScopeCard`] ([`derive_scope`]) lives only in memory for the current
//! invocation and can only be lent to awaited agent calls. Nothing in this
//! module turns a [`CardId`], a grant list or any other data into a card.
//!
//! Every operation is checked by the host against the caller's own cards:
//! deriving requires `card:derive`, inspecting requires `card:inspect`,
//! revoking requires `card:revoke` and installing requires `card:install`.
//! An expected refusal is returned as a [`PermissionError`].
//!
//! # Typical usage
//!
//! ```rust,ignore
//! use golem_rust::permissions::{self, Bounds, Lifespan, PatternGrant};
//!
//! let grant = PatternGrant::new("card", "*", "*", "inspect", "*");
//! let card = permissions::derive_from_wallet(
//!     &Bounds {
//!         lower_positive: vec![grant.clone()],
//!         ..Bounds::default()
//!     },
//!     None,
//! )?;
//! let child = card.derive(
//!     &Bounds {
//!         lower_positive: vec![grant],
//!         ..Bounds::default()
//!     },
//!     Some(&Lifespan::iso8601("PT1H")?),
//! )?;
//! let revoked = card.revoke()?; // revokes `card` and `child`
//! ```

use crate::bindings::golem::permissions::{
    derive as raw_derive, inspect as raw_inspect, kernel_introspection as raw_kernel,
    revoke as raw_revoke, types as raw, wallet as raw_wallet,
};
use crate::schema::wit::GuestPermissionCardHandle;
use crate::schema::wit::direct::{
    WireError, WirePreflight, WireReader, WireSchemaBuilder, WireWriter,
};
use crate::schema::wit::wire;
use crate::{AgentId, FromSchema, FromWire, IntoSchema, IntoWire, Uuid, WireSchema};
use std::collections::HashMap;
use std::sync::Arc;
#[cfg(feature = "export_golem_agentic")]
use std::sync::Weak;
use std::time::Duration;

#[cfg(feature = "export_golem_agentic")]
use crate::schema::{
    FromSchemaError, PermissionCardSpec, SchemaBuilder, SchemaType, SchemaValue, TypeId,
};

/// A permission card held by the calling component.
///
/// The card wraps a host resource handle. It has no public constructor, it
/// cannot be built from a [`CardId`] or from data, and it cannot be serialized.
///
/// Borrowing operations ([`id`](Self::id), [`parents`](Self::parents),
/// [`expires_at`](Self::expires_at), [`inspect`](Self::inspect),
/// [`derive`](Self::derive)) leave the card in place. Consuming operations
/// ([`revoke`](Self::revoke), [`install`](Self::install)) and passing the card
/// as an agent method argument or return value move the host handle out of
/// the card. Every later use of a moved card panics.
///
/// Dropping a card releases the caller's handle. It does not revoke the card
/// and does not remove it from the caller's wallet.
///
/// Decoding a card from an agent method argument or result
/// ([`FromSchema`](crate::FromSchema), [`FromWire`](crate::FromWire)) takes
/// the handle out of the decoded value, so a second decode of the same value
/// fails and two card objects never share one handle. Safe code cannot build
/// such a value without an existing handle. The `golem-schema` codec handle
/// types expose hidden constructors and accessors that can alias a handle the
/// component already holds, and wit-bindgen's `unsafe` resource constructors
/// can name any handle number. Neither path creates authority: the host
/// validates every handle against the component's own resource table.
pub struct PermissionCard {
    handle: GuestPermissionCardHandle,
}

const CARD_MOVED: &str = "permission card has already been moved (revoked, installed, or \
     transferred to another agent) and can no longer be used";

impl PermissionCard {
    fn from_raw(raw: wire::PermissionCard) -> Self {
        Self {
            handle: GuestPermissionCardHandle::new(raw),
        }
    }

    fn with_raw<R>(&self, f: impl FnOnce(&wire::PermissionCard) -> R) -> R {
        self.handle
            .with_handle(f)
            .unwrap_or_else(|| panic!("{CARD_MOVED}"))
    }

    fn take_raw(self) -> wire::PermissionCard {
        self.handle.take().unwrap_or_else(|| panic!("{CARD_MOVED}"))
    }

    /// The identifier of this card.
    ///
    /// # Panics
    ///
    /// Panics if the card was moved. The host traps if the card was revoked
    /// or is unknown.
    pub fn id(&self) -> CardId {
        CardId::from_raw(self.with_raw(raw::id))
    }

    /// The identifiers of the cards this card was derived from.
    ///
    /// # Panics
    ///
    /// Panics if the card was moved. The host traps if the card was revoked
    /// or is unknown.
    pub fn parents(&self) -> Vec<CardId> {
        self.with_raw(raw::parents)
            .into_iter()
            .map(CardId::from_raw)
            .collect()
    }

    /// The expiry time of this card, or `None` for a card without a lifespan.
    ///
    /// # Panics
    ///
    /// Panics if the card was moved. The host traps if the card was revoked
    /// or is unknown.
    pub fn expires_at(&self) -> Option<Timestamp> {
        self.with_raw(raw::expires_at).map(Timestamp::from)
    }

    /// Returns the four grant lists of this card. Requires `card:inspect`.
    ///
    /// # Panics
    ///
    /// Panics if the card was moved.
    pub fn inspect(&self) -> Result<CardView, PermissionError> {
        self.with_raw(raw_inspect::inspect_card)
            .map(CardView::from)
            .map_err(PermissionError::from)
    }

    /// Derives a child card from this card. Requires `card:derive`.
    ///
    /// The positive lists of `bounds` select the grants of this card that the
    /// child retains. The negative lists of `bounds` are denials that the child
    /// adds. The child's lifespan cannot exceed the lifespan of this card.
    ///
    /// This card may be a template card from the caller's wallet; the host
    /// resolves it against the caller first.
    ///
    /// # Panics
    ///
    /// Panics if the card was moved.
    pub fn derive(
        &self,
        bounds: &Bounds,
        lifespan: Option<&Lifespan>,
    ) -> Result<PermissionCard, PermissionError> {
        let bounds = RawBounds::from(bounds);
        let lifespan = lifespan.map(Lifespan::to_raw);
        self.with_raw(|card| {
            raw_derive::derive(
                card,
                &bounds.lower_positive,
                &bounds.lower_negative,
                &bounds.upper_positive,
                &bounds.upper_negative,
                lifespan.as_ref(),
            )
        })
        .map(PermissionCard::from_raw)
        .map_err(PermissionError::from)
    }

    /// Revokes this card and every card derived from it. Requires `card:revoke`.
    ///
    /// Returns the number of revoked cards. The card is moved even when the
    /// host refuses the revocation.
    ///
    /// # Panics
    ///
    /// Panics if the card was already moved.
    pub fn revoke(self) -> Result<u32, PermissionError> {
        raw_revoke::revoke_card(self.take_raw()).map_err(PermissionError::from)
    }

    /// Installs this card into the wallet of `target`. Requires `card:install`.
    ///
    /// The host currently accepts only [`Holder::Agent`] targets and refuses
    /// account and application targets with [`PermissionError::NotPermitted`].
    /// A target that does not match the card's recipient pattern fails with
    /// [`PermissionError::InstallRecipientMismatch`]. The card is moved even
    /// when the host refuses the installation.
    ///
    /// # Panics
    ///
    /// Panics if the card was already moved.
    pub fn install(self, target: &Holder) -> Result<(), PermissionError> {
        let target = target.to_raw();
        raw_wallet::install_card(self.take_raw(), &target).map_err(PermissionError::from)
    }
}

impl std::fmt::Debug for PermissionCard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = if self.handle.is_present() {
            "present"
        } else {
            "moved"
        };
        write!(f, "PermissionCard(<{state}>)")
    }
}

/// Returns every card in the caller's wallet.
///
/// Each returned card is an owned handle. Drop the cards that you do not keep
/// to release their handles; dropping a card does not change the wallet.
///
/// During an invocation that received a scope card, the result also contains
/// that scope card, typed as a `PermissionCard`, if the agent holds
/// `card:inspect`. The host refuses to revoke, install or persist it.
pub fn wallet() -> Vec<PermissionCard> {
    raw_wallet::self_wallet()
        .into_iter()
        .map(PermissionCard::from_raw)
        .collect()
}

/// Returns the version token of the caller's wallet. The token changes
/// whenever a card is installed into or removed from the wallet.
pub fn wallet_version() -> WalletVersionToken {
    raw_wallet::self_version().into()
}

/// Derives a card from the caller's wallet. Requires `card:derive`.
///
/// Unlike [`PermissionCard::derive`], all four lists of `bounds` are the full
/// requested grant lists of the new card. The host refuses the request if the
/// wallet does not cover them.
pub fn derive_from_wallet(
    bounds: &Bounds,
    lifespan: Option<&Lifespan>,
) -> Result<PermissionCard, PermissionError> {
    let bounds = RawBounds::from(bounds);
    let lifespan = lifespan.map(Lifespan::to_raw);
    raw_derive::derive_from_wallet(
        &bounds.lower_positive,
        &bounds.lower_negative,
        &bounds.upper_positive,
        &bounds.upper_negative,
        lifespan.as_ref(),
    )
    .map(PermissionCard::from_raw)
    .map_err(PermissionError::from)
}

/// Derives a scope card from one or more parents. Requires `card:derive`.
///
/// A parent is a card in the caller's wallet or a scope card of the current
/// invocation. All four lists of `bounds` are the full requested grant lists of
/// the new card.
///
/// # Panics
///
/// Panics if a parent card was moved.
pub fn derive_scope(
    parents: &[ScopeParent<'_>],
    bounds: &Bounds,
) -> Result<ScopeCard, PermissionError> {
    let bounds = RawBounds::from(bounds);
    with_raw_parents(parents, |parents| {
        raw_derive::derive_scope(
            parents,
            &bounds.lower_positive,
            &bounds.lower_negative,
            &bounds.upper_positive,
            &bounds.upper_negative,
        )
    })
    .map(|raw| ScopeCard { raw: Arc::new(raw) })
    .map_err(PermissionError::from)
}

/// A parent of [`derive_scope`]: a persistent card or a scope card.
#[derive(Clone, Copy, Debug)]
pub enum ScopeParent<'a> {
    Card(&'a PermissionCard),
    Scope(&'a ScopeCard),
}

impl<'a> From<&'a PermissionCard> for ScopeParent<'a> {
    fn from(card: &'a PermissionCard) -> Self {
        Self::Card(card)
    }
}

impl<'a> From<&'a ScopeCard> for ScopeParent<'a> {
    fn from(card: &'a ScopeCard) -> Self {
        Self::Scope(card)
    }
}

/// An invocation-local card derived with [`derive_scope`].
///
/// A scope card is valid only during the invocation that derived it. Lend it
/// to the callee of an awaited agent call with a generated client's
/// `with_scope_card` or `set_default_scope_card`, or with the
/// `*_with_scope_card` methods of the dynamic clients; the callee holds its
/// authority until that call ends. Lending borrows the card and never
/// consumes it. When the invocation ends, the host retires the scope card:
/// the handle stays valid until it is dropped, but lending it to an awaited
/// call, directly or as a client's default, fails with a `denied` RPC error,
/// and using it as a [`derive_scope`] parent fails with
/// [`PermissionError::NotPermitted`].
///
/// A scope card cannot be revoked, installed, derived into a persistent card,
/// serialized, or passed as an agent method argument or result. Dropping it
/// releases the handle.
pub struct ScopeCard {
    raw: Arc<wire::PermissionCard>,
}

impl ScopeCard {
    /// The identifier of this scope card.
    pub fn id(&self) -> CardId {
        CardId::from_raw(raw::id(&self.raw))
    }

    /// The identifiers of the persistent root cards of this scope card.
    pub fn parents(&self) -> Vec<CardId> {
        raw::parents(&self.raw)
            .into_iter()
            .map(CardId::from_raw)
            .collect()
    }

    /// The expiry time of this scope card, if one of its roots expires.
    pub fn expires_at(&self) -> Option<Timestamp> {
        raw::expires_at(&self.raw).map(Timestamp::from)
    }

    /// Returns the four grant lists of this scope card. Requires `card:inspect`.
    pub fn inspect(&self) -> Result<CardView, PermissionError> {
        raw_inspect::inspect_card(&self.raw)
            .map(CardView::from)
            .map_err(PermissionError::from)
    }

    pub(crate) fn raw(&self) -> &wire::PermissionCard {
        &self.raw
    }

    /// Lends the host handle to a raw `golem:agent/host` call.
    ///
    /// Only the host API tests use this, to show how the host treats a scope
    /// card on invocation variants that the typed clients never offer. The
    /// borrow cannot outlive the card, cannot be moved into a value, and grants
    /// nothing that the host does not validate.
    #[doc(hidden)]
    pub fn __raw_handle_for_host_api_tests(&self) -> &wire::PermissionCard {
        &self.raw
    }
}

impl std::fmt::Debug for ScopeCard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ScopeCard(..)")
    }
}

/// Lists the resource classes that the permission kernel knows, with their verbs.
pub fn list_modules() -> Vec<KernelModule> {
    raw_kernel::list_modules()
        .into_iter()
        .map(KernelModule::from)
        .collect()
}

/// Checks that `grant` names a known class and verb and uses valid patterns.
pub fn validate_grant(grant: &PatternGrant) -> Result<(), PermissionError> {
    raw_kernel::validate_grant(&grant.to_raw()).map_err(PermissionError::from)
}

const DEFAULT_SCOPE_CARD_DROPPED: &str = "the client's default scope card was dropped; clear \
     the default before dropping the scope card";

/// The default scope card of a generated agent client.
///
/// The slot only borrows the scope card: it keeps a weak reference, so
/// dropping the scope card releases its handle even while a client still
/// names it as its default. An awaited call through such a client panics.
#[doc(hidden)]
#[cfg(feature = "export_golem_agentic")]
#[derive(Clone, Default)]
pub struct __DefaultScopeCard(Option<Weak<wire::PermissionCard>>);

#[cfg(feature = "export_golem_agentic")]
impl __DefaultScopeCard {
    pub fn set(&mut self, card: &ScopeCard) {
        self.0 = Some(Arc::downgrade(&card.raw));
    }

    pub fn clear(&mut self) {
        self.0 = None;
    }

    /// The scope card to lend to one awaited call: the per-call card if there
    /// is one, else the default.
    ///
    /// Generated clients call this before they encode the call input, so a
    /// dropped default fails before any argument card moves.
    ///
    /// # Panics
    ///
    /// Panics if the default scope card was dropped.
    pub fn resolve(&self, scope_card: Option<&ScopeCard>) -> Option<ScopeCard> {
        match (scope_card, &self.0) {
            (Some(card), _) => Some(ScopeCard {
                raw: card.raw.clone(),
            }),
            (None, Some(default)) => Some(ScopeCard {
                raw: default
                    .upgrade()
                    .unwrap_or_else(|| panic!("{DEFAULT_SCOPE_CARD_DROPPED}")),
            }),
            (None, None) => None,
        }
    }
}

#[cfg(feature = "export_golem_agentic")]
impl std::fmt::Debug for __DefaultScopeCard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = if self.0.is_some() { "set" } else { "unset" };
        write!(f, "DefaultScopeCard(<{state}>)")
    }
}

/// Starts an awaited agent invocation, lending `scope_card` to the callee for
/// the invocation. Resolve the scope card with [`__DefaultScopeCard::resolve`]
/// before encoding `input`; this call then cannot fail before the host takes
/// ownership of `input`.
#[doc(hidden)]
#[cfg(feature = "export_golem_agentic")]
pub fn __async_invoke_and_await(
    rpc: &crate::golem_agentic::golem::agent::host::WasmRpc,
    method_name: &str,
    input: wire::SchemaValueTree,
    scope_card: Option<&ScopeCard>,
) -> crate::golem_agentic::golem::agent::host::AsyncInvocationWithMetadata {
    rpc.async_invoke_and_await(method_name, input, scope_card.map(ScopeCard::raw))
}

/// Borrows the host handles of all `parents` at the same time.
///
/// Each persistent card's handle is behind its own lock, so a card that
/// appears more than once is locked once and its handle is passed again.
fn with_raw_parents<R>(
    parents: &[ScopeParent<'_>],
    f: impl FnOnce(&[&wire::PermissionCard]) -> R,
) -> R {
    type WithRawCards<'a, R> = Box<dyn FnOnce(&[&wire::PermissionCard]) -> R + 'a>;

    fn go<R>(
        parents: &[ScopeParent<'_>],
        first_index: &HashMap<*const (), usize>,
        acc: &[&wire::PermissionCard],
        f: WithRawCards<'_, R>,
    ) -> R {
        let Some((parent, rest)) = parents.split_first() else {
            return f(acc);
        };
        let card = match parent {
            ScopeParent::Scope(scope) => {
                let mut next = acc.to_vec();
                next.push(scope.raw.as_ref());
                return go(rest, first_index, &next, f);
            }
            ScopeParent::Card(card) => card,
        };
        let index = first_index[&card.handle.cell_id()];
        if index < acc.len() {
            let mut next = acc.to_vec();
            next.push(acc[index]);
            return go(rest, first_index, &next, f);
        }
        card.with_raw(|raw| {
            let mut next = acc.to_vec();
            next.push(raw);
            go(rest, first_index, &next, f)
        })
    }

    let mut first_index = HashMap::new();
    for (index, parent) in parents.iter().enumerate() {
        if let ScopeParent::Card(card) = parent {
            first_index.entry(card.handle.cell_id()).or_insert(index);
        }
    }
    go(parents, &first_index, &[], Box::new(f))
}

/// An opaque card identifier.
///
/// Card identifiers support only equality and hashing. Compare
/// [`PermissionCard::id`] with [`PermissionCard::parents`] to follow the
/// derivation graph, or to find a card in the [`wallet`]. Formatting and serde
/// serialization show a redacted placeholder. A card identifier has no
/// constructor and no schema or wire codec, and it never grants authority.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct CardId {
    uuid: Uuid,
}

impl CardId {
    fn from_raw(raw: wire::CardId) -> Self {
        Self {
            uuid: raw.uuid.into(),
        }
    }

    /// Reveals the identifier as schema data, for tests that compare it with
    /// a card identifier known to the host. There is no reverse conversion.
    #[doc(hidden)]
    pub fn __reveal_for_tests(&self) -> golem_schema::CardId {
        golem_schema::CardId::new(self.uuid)
    }
}

const REDACTED_CARD_ID: &str = "<redacted card id>";

impl std::fmt::Debug for CardId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CardId({REDACTED_CARD_ID})")
    }
}

impl std::fmt::Display for CardId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(REDACTED_CARD_ID)
    }
}

#[cfg(feature = "export_golem_agentic")]
impl crate::serde::Serialize for CardId {
    fn serialize<S: crate::serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(REDACTED_CARD_ID)
    }
}

/// A grant pattern: `class(owner) @ recipient : verb : resource-id`.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, IntoSchema, FromSchema, IntoWire, FromWire, WireSchema,
)]
#[schema(named = "golem.permissions.PatternGrant")]
pub struct PatternGrant {
    pub class: String,
    pub owner: String,
    pub recipient: String,
    pub verb: String,
    pub resource_id: String,
}

impl PatternGrant {
    pub fn new(
        class: impl Into<String>,
        owner: impl Into<String>,
        recipient: impl Into<String>,
        verb: impl Into<String>,
        resource_id: impl Into<String>,
    ) -> Self {
        Self {
            class: class.into(),
            owner: owner.into(),
            recipient: recipient.into(),
            verb: verb.into(),
            resource_id: resource_id.into(),
        }
    }

    fn to_raw(&self) -> raw::PatternGrant {
        raw::PatternGrant {
            class: self.class.clone(),
            owner: self.owner.clone(),
            recipient: self.recipient.clone(),
            verb: self.verb.clone(),
            resource_id: self.resource_id.clone(),
        }
    }
}

impl From<raw::PatternGrant> for PatternGrant {
    fn from(value: raw::PatternGrant) -> Self {
        Self {
            class: value.class,
            owner: value.owner,
            recipient: value.recipient,
            verb: value.verb,
            resource_id: value.resource_id,
        }
    }
}

/// The four grant lists of a card, as returned by [`PermissionCard::inspect`].
#[derive(
    Clone, Debug, Default, PartialEq, Eq, IntoSchema, FromSchema, IntoWire, FromWire, WireSchema,
)]
#[schema(named = "golem.permissions.CardView")]
pub struct CardView {
    pub lower_positive: Vec<PatternGrant>,
    pub lower_negative: Vec<PatternGrant>,
    pub upper_positive: Vec<PatternGrant>,
    pub upper_negative: Vec<PatternGrant>,
}

impl From<raw::CardView> for CardView {
    fn from(value: raw::CardView) -> Self {
        let convert =
            |grants: Vec<raw::PatternGrant>| grants.into_iter().map(PatternGrant::from).collect();
        Self {
            lower_positive: convert(value.lower_positive),
            lower_negative: convert(value.lower_negative),
            upper_positive: convert(value.upper_positive),
            upper_negative: convert(value.upper_negative),
        }
    }
}

/// The requested grant lists of a derivation.
///
/// For [`PermissionCard::derive`] the positive lists are the parent grants to
/// retain and the negative lists are the denials to add. For
/// [`derive_from_wallet`] and [`derive_scope`] the four lists are the full
/// requested grant lists of the new card.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bounds {
    pub lower_positive: Vec<PatternGrant>,
    pub lower_negative: Vec<PatternGrant>,
    pub upper_positive: Vec<PatternGrant>,
    pub upper_negative: Vec<PatternGrant>,
}

struct RawBounds {
    lower_positive: Vec<raw::PatternGrant>,
    lower_negative: Vec<raw::PatternGrant>,
    upper_positive: Vec<raw::PatternGrant>,
    upper_negative: Vec<raw::PatternGrant>,
}

impl From<&Bounds> for RawBounds {
    fn from(value: &Bounds) -> Self {
        let convert = |grants: &[PatternGrant]| grants.iter().map(PatternGrant::to_raw).collect();
        Self {
            lower_positive: convert(&value.lower_positive),
            lower_negative: convert(&value.lower_negative),
            upper_positive: convert(&value.upper_positive),
            upper_negative: convert(&value.upper_negative),
        }
    }
}

/// The lifespan of a derived card, as an ISO-8601 duration of the form
/// `PnDTnHnMnS`, for example `"P1DT2H"` or `"PT30.5S"`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Lifespan {
    iso_8601: String,
}

impl Lifespan {
    /// A lifespan from an ISO-8601 duration string of the form `PnDTnHnMnS`.
    /// Years, months and weeks are rejected.
    pub fn iso8601(value: impl Into<String>) -> Result<Self, LifespanError> {
        let iso_8601 = value.into();
        if !iso_8601.starts_with('P') {
            return Err(LifespanError(format!(
                "ISO-8601 duration must start with 'P': {iso_8601:?}"
            )));
        }
        crate::schema::canonical::duration::from_text(&iso_8601)
            .map_err(|error| LifespanError(format!("{iso_8601:?}: {error}")))?;
        Ok(Self { iso_8601 })
    }

    /// The ISO-8601 representation of this lifespan.
    pub fn as_iso8601(&self) -> &str {
        &self.iso_8601
    }

    fn to_raw(&self) -> raw::Duration {
        raw::Duration {
            iso_8601: self.iso_8601.clone(),
        }
    }
}

impl TryFrom<Duration> for Lifespan {
    type Error = LifespanError;

    /// Rejects a zero duration and a duration that does not fit in `i64`
    /// nanoseconds.
    fn try_from(value: Duration) -> Result<Self, Self::Error> {
        if value.is_zero() {
            return Err(LifespanError("duration must be positive".to_string()));
        }
        let nanoseconds = i64::try_from(value.as_nanos()).map_err(|_| {
            LifespanError(format!("duration {value:?} exceeds the supported range"))
        })?;
        Ok(Self {
            iso_8601: crate::schema::canonical::duration::to_text(
                &crate::schema::DurationValuePayload { nanoseconds },
            ),
        })
    }
}

/// A lifespan that cannot be represented.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LifespanError(String);

impl std::fmt::Display for LifespanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid lifespan: {}", self.0)
    }
}

impl std::error::Error for LifespanError {}

/// A point in time, in seconds and nanoseconds since the Unix epoch.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    IntoSchema,
    FromSchema,
    IntoWire,
    FromWire,
    WireSchema,
)]
#[schema(named = "golem.permissions.Timestamp")]
pub struct Timestamp {
    pub seconds: i64,
    pub nanoseconds: u32,
}

impl From<raw::Timestamp> for Timestamp {
    fn from(value: raw::Timestamp) -> Self {
        Self {
            seconds: value.seconds,
            nanoseconds: value.nanoseconds,
        }
    }
}

/// A wallet version token, as returned by [`wallet_version`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct WalletVersionToken {
    /// The 32-byte BLAKE3 hash of the wallet holder.
    pub wallet_id_hash: Vec<u8>,
    /// Increases whenever the wallet changes.
    pub generation: u64,
}

impl From<raw::WalletVersionToken> for WalletVersionToken {
    fn from(value: raw::WalletVersionToken) -> Self {
        Self {
            wallet_id_hash: value.wallet_id_hash,
            generation: value.generation,
        }
    }
}

/// A resource class of the permission kernel, as returned by [`list_modules`].
#[derive(Clone, Debug, PartialEq, Eq, IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
#[schema(named = "golem.permissions.KernelModule")]
pub struct KernelModule {
    pub class_name: String,
    pub verbs: Vec<String>,
}

impl From<raw_kernel::KernelModule> for KernelModule {
    fn from(value: raw_kernel::KernelModule) -> Self {
        Self {
            class_name: value.class_name,
            verbs: value.verbs,
        }
    }
}

/// The holder of a wallet: the target of [`PermissionCard::install`].
///
/// The host currently accepts only [`Holder::Agent`] as an install target.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Holder {
    Account(Uuid),
    App(Uuid),
    Agent(AgentId),
}

impl Holder {
    fn to_raw(&self) -> raw::Holder {
        match self {
            Self::Account(uuid) => raw::Holder::Account((*uuid).into()),
            Self::App(uuid) => raw::Holder::App((*uuid).into()),
            Self::Agent(agent_id) => {
                raw::Holder::Agent(crate::schema_agent_id_to_wire(agent_id.clone()))
            }
        }
    }
}

/// An expected refusal of a permission operation by the host.
///
/// Each case carries the host's explanation.
#[derive(Clone, Debug, PartialEq, Eq, IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
#[schema(named = "golem.permissions.PermissionError")]
pub enum PermissionError {
    NotPermitted(String),
    LowerBoundTooBroad(String),
    UpperBoundTooBroad(String),
    WalletMultiSourceRequired(String),
    CardRevoked(String),
    CardExpired(String),
    InstallRecipientMismatch(String),
    UnknownResourceClass(String),
    InvalidPattern(String),
    InvalidVerb(String),
    InvalidOwner(String),
    InvalidRecipient(String),
}

impl PermissionError {
    /// The host's explanation of the refusal.
    pub fn message(&self) -> &str {
        match self {
            Self::NotPermitted(message)
            | Self::LowerBoundTooBroad(message)
            | Self::UpperBoundTooBroad(message)
            | Self::WalletMultiSourceRequired(message)
            | Self::CardRevoked(message)
            | Self::CardExpired(message)
            | Self::InstallRecipientMismatch(message)
            | Self::UnknownResourceClass(message)
            | Self::InvalidPattern(message)
            | Self::InvalidVerb(message)
            | Self::InvalidOwner(message)
            | Self::InvalidRecipient(message) => message,
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Self::NotPermitted(_) => "not-permitted",
            Self::LowerBoundTooBroad(_) => "lower-bound-too-broad",
            Self::UpperBoundTooBroad(_) => "upper-bound-too-broad",
            Self::WalletMultiSourceRequired(_) => "wallet-multi-source-required",
            Self::CardRevoked(_) => "card-revoked",
            Self::CardExpired(_) => "card-expired",
            Self::InstallRecipientMismatch(_) => "install-recipient-mismatch",
            Self::UnknownResourceClass(_) => "unknown-resource-class",
            Self::InvalidPattern(_) => "invalid-pattern",
            Self::InvalidVerb(_) => "invalid-verb",
            Self::InvalidOwner(_) => "invalid-owner",
            Self::InvalidRecipient(_) => "invalid-recipient",
        }
    }
}

impl From<raw::PermissionError> for PermissionError {
    fn from(value: raw::PermissionError) -> Self {
        match value {
            raw::PermissionError::NotPermitted(message) => Self::NotPermitted(message),
            raw::PermissionError::LowerBoundTooBroad(message) => Self::LowerBoundTooBroad(message),
            raw::PermissionError::UpperBoundTooBroad(message) => Self::UpperBoundTooBroad(message),
            raw::PermissionError::WalletMultiSourceRequired(message) => {
                Self::WalletMultiSourceRequired(message)
            }
            raw::PermissionError::CardRevoked(message) => Self::CardRevoked(message),
            raw::PermissionError::CardExpired(message) => Self::CardExpired(message),
            raw::PermissionError::InstallRecipientMismatch(message) => {
                Self::InstallRecipientMismatch(message)
            }
            raw::PermissionError::UnknownResourceClass(message) => {
                Self::UnknownResourceClass(message)
            }
            raw::PermissionError::InvalidPattern(message) => Self::InvalidPattern(message),
            raw::PermissionError::InvalidVerb(message) => Self::InvalidVerb(message),
            raw::PermissionError::InvalidOwner(message) => Self::InvalidOwner(message),
            raw::PermissionError::InvalidRecipient(message) => Self::InvalidRecipient(message),
        }
    }
}

impl std::fmt::Display for PermissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind(), self.message())
    }
}

impl std::error::Error for PermissionError {}

impl WireSchema for PermissionCard {
    fn wire_type_id() -> String {
        GuestPermissionCardHandle::wire_type_id()
    }

    fn append_schema(builder: &mut WireSchemaBuilder) -> i32 {
        GuestPermissionCardHandle::append_schema(builder)
    }
}

impl FromWire for PermissionCard {
    /// Takes the handle out of the decoded node. Decoding the same node again,
    /// for example from a second reader over a shared snapshot, fails.
    fn read_wire(reader: &mut WireReader, index: i32) -> Result<Self, WireError> {
        GuestPermissionCardHandle::read_wire(reader, index)?
            .take()
            .map(Self::from_raw)
            .ok_or(WireError::ConsumedResource("permission-card"))
    }
}

impl IntoWire for PermissionCard {
    fn preflight(&self, resources: &mut WirePreflight) -> Result<(), WireError> {
        self.handle.preflight(resources)
    }

    fn write_wire(&self, writer: &mut WireWriter) -> Result<i32, WireError> {
        self.handle.write_wire(writer)
    }
}

#[cfg(feature = "export_golem_agentic")]
impl IntoSchema for PermissionCard {
    fn type_id() -> TypeId {
        TypeId::new("golem.core.PermissionCard")
    }

    fn register_in(_builder: &mut SchemaBuilder) -> SchemaType {
        SchemaType::permission_card(PermissionCardSpec::default())
    }

    /// Lowers the card into a schema value that shares its handle.
    ///
    /// The handle moves out of the card only when the schema value is
    /// encoded into a WIT `schema-value-tree`.
    fn to_value(&self) -> SchemaValue {
        SchemaValue::PermissionCard(self.handle.clone())
    }
}

#[cfg(feature = "export_golem_agentic")]
impl FromSchema for PermissionCard {
    /// Takes the handle out of `value`. Decoding the same value again fails.
    fn from_value(value: &SchemaValue) -> Result<Self, FromSchemaError> {
        match value {
            SchemaValue::PermissionCard(handle) => {
                handle.take().map(Self::from_raw).ok_or_else(|| {
                    FromSchemaError::custom(
                        "permission card has already been moved out of this value",
                    )
                })
            }
            other => Err(FromSchemaError::shape_mismatch(
                "permission-card",
                format!("{other:?}"),
                "PermissionCard",
            )),
        }
    }
}

#[cfg(feature = "export_golem_agentic")]
impl crate::serde::Serialize for PermissionCard {
    fn serialize<S: crate::serde::Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
        Err(crate::serde::ser::Error::custom(
            "permission cards cannot be serialized; pass them as agent method arguments or results",
        ))
    }
}

#[cfg(feature = "export_golem_agentic")]
impl<'de> crate::serde::Deserialize<'de> for PermissionCard {
    fn deserialize<D: crate::serde::Deserializer<'de>>(_deserializer: D) -> Result<Self, D::Error> {
        Err(crate::serde::de::Error::custom(
            "permission cards cannot be deserialized or forged from data",
        ))
    }
}

#[cfg(feature = "export_golem_agentic")]
impl crate::serde::Serialize for ScopeCard {
    fn serialize<S: crate::serde::Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
        Err(crate::serde::ser::Error::custom(
            "scope cards cannot be serialized; they are valid only during the invocation that derived them",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::wit::direct;
    use test_r::test;

    fn fake_card(handle: u32) -> PermissionCard {
        PermissionCard::from_raw(unsafe { wire::PermissionCard::from_handle(handle) })
    }

    /// Releases a fake handle without calling the host's resource drop.
    fn forget(card: PermissionCard) -> Option<u32> {
        card.handle.take().map(|raw| raw.take_handle())
    }

    fn fake_scope(handle: u32) -> ScopeCard {
        ScopeCard {
            raw: Arc::new(unsafe { wire::PermissionCard::from_handle(handle) }),
        }
    }

    /// Releases the only reference to a fake scope handle without calling the
    /// host's resource drop.
    fn forget_scope(scope: ScopeCard) -> u32 {
        Arc::try_unwrap(scope.raw)
            .unwrap_or_else(|_| panic!("scope handle is still shared"))
            .take_handle()
    }

    /// Drops a scope card whose handle is still shared with another scope
    /// card, and returns the handle number.
    #[cfg(feature = "export_golem_agentic")]
    fn forget_scope_shared_handle(scope: ScopeCard) -> u32 {
        assert!(Arc::strong_count(&scope.raw) > 1);
        scope.raw.handle()
    }

    fn panic_message(f: impl FnOnce() + std::panic::UnwindSafe) -> String {
        let payload = std::panic::catch_unwind(f).expect_err("operation must panic");
        payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_default()
    }

    #[test]
    fn permission_error_covers_every_wit_case() {
        let cases = [
            (
                raw::PermissionError::NotPermitted("a".into()),
                PermissionError::NotPermitted("a".into()),
                "not-permitted",
            ),
            (
                raw::PermissionError::LowerBoundTooBroad("b".into()),
                PermissionError::LowerBoundTooBroad("b".into()),
                "lower-bound-too-broad",
            ),
            (
                raw::PermissionError::UpperBoundTooBroad("c".into()),
                PermissionError::UpperBoundTooBroad("c".into()),
                "upper-bound-too-broad",
            ),
            (
                raw::PermissionError::WalletMultiSourceRequired("d".into()),
                PermissionError::WalletMultiSourceRequired("d".into()),
                "wallet-multi-source-required",
            ),
            (
                raw::PermissionError::CardRevoked("e".into()),
                PermissionError::CardRevoked("e".into()),
                "card-revoked",
            ),
            (
                raw::PermissionError::CardExpired("f".into()),
                PermissionError::CardExpired("f".into()),
                "card-expired",
            ),
            (
                raw::PermissionError::InstallRecipientMismatch("g".into()),
                PermissionError::InstallRecipientMismatch("g".into()),
                "install-recipient-mismatch",
            ),
            (
                raw::PermissionError::UnknownResourceClass("h".into()),
                PermissionError::UnknownResourceClass("h".into()),
                "unknown-resource-class",
            ),
            (
                raw::PermissionError::InvalidPattern("i".into()),
                PermissionError::InvalidPattern("i".into()),
                "invalid-pattern",
            ),
            (
                raw::PermissionError::InvalidVerb("j".into()),
                PermissionError::InvalidVerb("j".into()),
                "invalid-verb",
            ),
            (
                raw::PermissionError::InvalidOwner("k".into()),
                PermissionError::InvalidOwner("k".into()),
                "invalid-owner",
            ),
            (
                raw::PermissionError::InvalidRecipient("l".into()),
                PermissionError::InvalidRecipient("l".into()),
                "invalid-recipient",
            ),
        ];
        // The `golem:permissions/types.permission-error` variant has exactly
        // this many cases; a WIT change must update this test.
        assert_eq!(cases.len(), 12);
        let mut kinds = std::collections::HashSet::new();
        for (raw_error, expected, kind) in cases {
            let message = expected.message().to_string();
            let converted = PermissionError::from(raw_error);
            assert_eq!(converted, expected);
            assert_eq!(converted.to_string(), format!("{kind}: {message}"));
            assert!(kinds.insert(kind));
        }
    }

    #[test]
    fn lifespan_from_iso8601_accepts_only_days_and_time_units() {
        for value in ["PT1H", "P1DT2H", "PT30.5S", "P2D"] {
            assert_eq!(Lifespan::iso8601(value).unwrap().as_iso8601(), value);
        }
        for value in ["P1Y", "P1M", "P1W", "PT", "1h", "5s", "", "-PT1S"] {
            assert!(
                Lifespan::iso8601(value).is_err(),
                "{value} must be rejected"
            );
        }
    }

    #[test]
    fn lifespan_from_duration_is_a_fixed_iso8601_duration() {
        let lifespan = |duration| Lifespan::try_from(duration).unwrap();
        assert!(Lifespan::try_from(Duration::ZERO).is_err());
        assert_eq!(
            lifespan(Duration::from_secs(90_061)).as_iso8601(),
            "P1DT1H1M1S"
        );
        assert_eq!(
            lifespan(Duration::from_millis(2_500)).as_iso8601(),
            "PT2.5S"
        );
        assert_eq!(
            lifespan(Duration::from_nanos(1)).as_iso8601(),
            "PT0.000000001S"
        );
        assert!(Lifespan::try_from(Duration::from_secs(u64::MAX)).is_err());
    }

    #[test]
    fn moved_card_fails_loudly() {
        let card = fake_card(7);
        let alias = PermissionCard {
            handle: card.handle.clone(),
        };
        assert_eq!(forget(card), Some(7));

        assert_eq!(format!("{alias:?}"), "PermissionCard(<moved>)");
        let message = panic_message(std::panic::AssertUnwindSafe(|| {
            let _ = alias.id();
        }));
        assert!(message.contains("already been moved"), "{message}");
        let message = panic_message(std::panic::AssertUnwindSafe(|| {
            let _ = alias.inspect();
        }));
        assert!(message.contains("already been moved"), "{message}");
        let message = panic_message(std::panic::AssertUnwindSafe(|| {
            let _ = alias.derive(&Bounds::default(), None);
        }));
        assert!(message.contains("already been moved"), "{message}");
        let message = panic_message(std::panic::AssertUnwindSafe(|| {
            let _ = derive_scope(&[(&alias).into()], &Bounds::default());
        }));
        assert!(message.contains("already been moved"), "{message}");

        let revoked = PermissionCard {
            handle: alias.handle.clone(),
        };
        let message = panic_message(std::panic::AssertUnwindSafe(move || {
            let _ = revoked.revoke();
        }));
        assert!(message.contains("already been moved"), "{message}");
        let installed = PermissionCard {
            handle: alias.handle.clone(),
        };
        let message = panic_message(std::panic::AssertUnwindSafe(move || {
            let _ = installed.install(&Holder::Account(Uuid::nil()));
        }));
        assert!(message.contains("already been moved"), "{message}");
    }

    #[test]
    fn transferred_card_is_moved_and_cannot_be_encoded_twice() {
        let card = fake_card(11);
        let tree = direct::encode(&card).unwrap();
        assert_eq!(format!("{card:?}"), "PermissionCard(<moved>)");
        assert!(matches!(
            direct::encode(&card),
            Err(WireError::ConsumedResource(_))
        ));

        let received = direct::decode::<PermissionCard>(tree).unwrap();
        assert_eq!(format!("{received:?}"), "PermissionCard(<present>)");
        assert_eq!(forget(received), Some(11));
    }

    #[test]
    fn aliased_card_is_rejected_before_any_handle_moves() {
        let card = fake_card(13);
        let alias = PermissionCard {
            handle: card.handle.clone(),
        };
        let pair = (card, alias);
        assert!(matches!(
            direct::encode(&pair),
            Err(WireError::AliasedResource(_))
        ));
        assert_eq!(format!("{:?}", pair.1), "PermissionCard(<present>)");
        assert_eq!(forget(pair.0), Some(13));
    }

    #[cfg(feature = "export_golem_agentic")]
    #[test]
    fn card_cannot_be_serialized_or_deserialized() {
        let card = fake_card(17);
        let error = crate::serde_json::to_string(&card).unwrap_err().to_string();
        assert!(error.contains("cannot be serialized"), "{error}");
        let error = crate::serde_json::from_str::<PermissionCard>("17")
            .unwrap_err()
            .to_string();
        assert!(error.contains("cannot be deserialized"), "{error}");
        assert_eq!(forget(card), Some(17));
    }

    #[cfg(feature = "export_golem_agentic")]
    #[test]
    fn scope_card_cannot_be_serialized() {
        let scope = fake_scope(18);
        let error = crate::serde_json::to_string(&scope)
            .unwrap_err()
            .to_string();
        assert!(error.contains("cannot be serialized"), "{error}");
        assert_eq!(format!("{scope:?}"), "ScopeCard(..)");
        assert_eq!(forget_scope(scope), 18);
    }

    #[cfg(feature = "export_golem_agentic")]
    #[test]
    fn decoding_a_card_takes_its_handle() {
        let card = fake_card(23);
        let value = card.to_value();
        let decoded = PermissionCard::from_value(&value).unwrap();
        assert_eq!(format!("{card:?}"), "PermissionCard(<moved>)");
        assert!(PermissionCard::from_value(&value).is_err());
        assert!(PermissionCard::from_value(&SchemaValue::Bool(true)).is_err());
        assert_eq!(forget(decoded), Some(23));
    }

    #[cfg(feature = "export_golem_agentic")]
    #[test]
    fn default_scope_card_is_borrowed_and_per_call_card_wins() {
        let default_scope = fake_scope(19);
        let per_call = fake_scope(20);
        let mut default = __DefaultScopeCard::default();
        assert!(default.resolve(None).is_none());
        default.set(&default_scope);
        let handle = |card: Option<ScopeCard>| card.map(forget_scope_shared_handle);
        assert_eq!(handle(default.resolve(None)), Some(19));
        assert_eq!(handle(default.resolve(Some(&per_call))), Some(20));

        // The default does not keep the scope card alive.
        assert_eq!(forget_scope(default_scope), 19);
        let message = panic_message(std::panic::AssertUnwindSafe(|| {
            let _ = default.resolve(None);
        }));
        assert!(
            message.contains("default scope card was dropped"),
            "{message}"
        );
        assert_eq!(handle(default.resolve(Some(&per_call))), Some(20));

        default.clear();
        assert!(default.resolve(None).is_none());
        assert_eq!(forget_scope(per_call), 20);
    }

    #[test]
    fn scope_parents_are_each_locked_once() {
        let first = fake_card(21);
        let second = fake_card(22);
        let scope = fake_scope(24);
        let handles = with_raw_parents(
            &[
                (&first).into(),
                (&scope).into(),
                (&second).into(),
                (&first).into(),
            ],
            |raw| raw.iter().map(|card| card.handle()).collect::<Vec<_>>(),
        );
        assert_eq!(handles, vec![21, 24, 22, 21]);
        assert_eq!(forget(first), Some(21));
        assert_eq!(forget(second), Some(22));
        assert_eq!(forget_scope(scope), 24);
    }

    #[test]
    fn card_id_is_opaque_but_comparable() {
        let raw_id = || wire::CardId {
            uuid: Uuid::from_u128(42).into(),
        };
        let id = CardId::from_raw(raw_id());
        assert_eq!(id, CardId::from_raw(raw_id()));
        assert_ne!(
            id,
            CardId::from_raw(wire::CardId {
                uuid: Uuid::from_u128(43).into()
            })
        );
        assert_eq!(format!("{id:?}"), "CardId(<redacted card id>)");
        assert_eq!(format!("{id:#?}"), "CardId(<redacted card id>)");
        assert_eq!(id.to_string(), "<redacted card id>");
        #[cfg(feature = "export_golem_agentic")]
        assert_eq!(
            crate::serde_json::to_string(&vec![id.clone()]).unwrap(),
            r#"["<redacted card id>"]"#
        );
        assert_eq!(
            id.__reveal_for_tests(),
            golem_schema::CardId::new(Uuid::from_u128(42))
        );
    }

    #[test]
    fn decoding_a_shared_wire_snapshot_twice_does_not_alias_the_card() {
        let card = fake_card(25);
        let tree = direct::encode(&card).unwrap();
        let snapshot = std::rc::Rc::new(direct::WireSnapshot::new(tree.value_nodes));
        let first = PermissionCard::read_wire(&mut snapshot.reader(), tree.root).unwrap();
        assert!(matches!(
            PermissionCard::read_wire(&mut snapshot.reader(), tree.root),
            Err(WireError::ConsumedResource(_))
        ));
        assert_eq!(forget(first), Some(25));
    }
}
