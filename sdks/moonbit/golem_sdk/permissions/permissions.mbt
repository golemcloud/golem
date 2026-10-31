// Typed permission-card management over `golem:permissions@0.1.0`.
//
// A `PermissionCard` (concrete) or a `PolymorphicPermissionCard` (template)
// wraps an owned persistent `golem:core/types.permission-card` resource in a
// take-once holder, and a `ScopeCard` wraps the handle of an invocation-local
// scope card. `wallet` labels each card with its kind as a `WalletCard`.
// `PatternGrant`, `CardView`, `Bounds`, `Lifespan` and `CardId` are plain
// data; nothing in this package turns them into a card.
// Derivation inputs are requests that the host validates against the parent's
// lattice.
//
// The host enforces all authority: a card handle carries only authority that
// the host granted to this agent. The SDK obtains cards from host calls (the
// wallet and derivations) and from decoded agent and tool values. The raw
// generated `@types.PermissionCard` is a public `Int` wrapper, so user code
// can put any handle index into a raw wire tree and decode it. Decoding
// rejects a handle that any card holds, so such data
// cannot alias a held card. A handle that no card holds decodes into a card;
// the host traps on it when the handle is unknown, and otherwise it names a
// card that the agent already owns through the raw bindings.

///|
/// An expected failure of a permission operation, reported by the host.
/// `CardRevoked` and `CardExpired` report a card that is revoked or expired;
/// the host checks the card at every use of its handle. Host traps (for example
/// an unknown handle, or a revoked or expired card, passed to `id`, `parents`
/// or `expires_at`) are not represented here; they stop the invocation.
pub(all) suberror PermissionError {
  NotPermitted(String)
  LowerBoundTooBroad(String)
  UpperBoundTooBroad(String)
  WalletMultiSourceRequired(String)
  CardRevoked(String)
  CardExpired(String)
  InstallRecipientMismatch(String)
  UnknownResourceClass(String)
  InvalidPattern(String)
  InvalidVerb(String)
  InvalidOwner(String)
  InvalidRecipient(String)
} derive(Eq, Debug)

///|
pub extend PermissionError with Eq::{equal, not_equal}

///|
pub extend PermissionError with Debug::{to_repr}

///|
pub extend PermissionError with Show::{output, to_string}

///|
/// The WIT case name of the error, for example `"not-permitted"`.
pub fn PermissionError::kind(self : PermissionError) -> String {
  match self {
    NotPermitted(_) => "not-permitted"
    LowerBoundTooBroad(_) => "lower-bound-too-broad"
    UpperBoundTooBroad(_) => "upper-bound-too-broad"
    WalletMultiSourceRequired(_) => "wallet-multi-source-required"
    CardRevoked(_) => "card-revoked"
    CardExpired(_) => "card-expired"
    InstallRecipientMismatch(_) => "install-recipient-mismatch"
    UnknownResourceClass(_) => "unknown-resource-class"
    InvalidPattern(_) => "invalid-pattern"
    InvalidVerb(_) => "invalid-verb"
    InvalidOwner(_) => "invalid-owner"
    InvalidRecipient(_) => "invalid-recipient"
  }
}

///|
/// The host-provided detail message of the error.
pub fn PermissionError::message(self : PermissionError) -> String {
  match self {
    NotPermitted(message)
    | LowerBoundTooBroad(message)
    | UpperBoundTooBroad(message)
    | WalletMultiSourceRequired(message)
    | CardRevoked(message)
    | CardExpired(message)
    | InstallRecipientMismatch(message)
    | UnknownResourceClass(message)
    | InvalidPattern(message)
    | InvalidVerb(message)
    | InvalidOwner(message)
    | InvalidRecipient(message) => message
  }
}

///|
pub impl Show for PermissionError with fn output(self, logger) {
  logger.write_string(self.kind())
  logger.write_string(": ")
  logger.write_string(self.message())
}

///|
fn permission_error_from_raw(error : @raw.PermissionError) -> PermissionError {
  match error {
    NotPermitted(message) => NotPermitted(message)
    LowerBoundTooBroad(message) => LowerBoundTooBroad(message)
    UpperBoundTooBroad(message) => UpperBoundTooBroad(message)
    WalletMultiSourceRequired(message) => WalletMultiSourceRequired(message)
    CardRevoked(message) => CardRevoked(message)
    CardExpired(message) => CardExpired(message)
    InstallRecipientMismatch(message) => InstallRecipientMismatch(message)
    UnknownResourceClass(message) => UnknownResourceClass(message)
    InvalidPattern(message) => InvalidPattern(message)
    InvalidVerb(message) => InvalidVerb(message)
    InvalidOwner(message) => InvalidOwner(message)
    InvalidRecipient(message) => InvalidRecipient(message)
  }
}

///|
fn[T] lift_result(
  result : Result[T, @raw.PermissionError],
) -> T raise PermissionError {
  match result {
    Ok(value) => value
    Err(error) => raise permission_error_from_raw(error)
  }
}

///|
/// One grant pattern: a resource class, owner and recipient paths, a verb and a
/// resource-id pattern. A grant is a request; the host validates it.
pub(all) struct PatternGrant {
  class : String
  owner : String
  recipient : String
  verb : String
  resource_id : String
} derive(Eq, Hash, Debug)

///|
pub extend PatternGrant with Eq::{equal, not_equal}

///|
pub extend PatternGrant with Hash::{hash, hash_combine}

///|
pub extend PatternGrant with Debug::{to_repr}

///|
fn PatternGrant::to_raw(self : PatternGrant) -> @raw.PatternGrant {
  {
    class: self.class,
    owner: self.owner,
    recipient: self.recipient,
    verb: self.verb,
    resource_id: self.resource_id,
  }
}

///|
fn PatternGrant::from_raw(raw : @raw.PatternGrant) -> PatternGrant {
  {
    class: raw.class,
    owner: raw.owner,
    recipient: raw.recipient,
    verb: raw.verb,
    resource_id: raw.resource_id,
  }
}

///|
fn grants_to_raw(grants : Array[PatternGrant]) -> Array[@raw.PatternGrant] {
  grants.map(PatternGrant::to_raw)
}

///|
fn grants_from_raw(grants : Array[@raw.PatternGrant]) -> Array[PatternGrant] {
  grants.map(PatternGrant::from_raw)
}

///|
/// The four grant lists of a card's lattice, as returned by `inspect`.
pub(all) struct CardView {
  lower_positive : Array[PatternGrant]
  lower_negative : Array[PatternGrant]
  upper_positive : Array[PatternGrant]
  upper_negative : Array[PatternGrant]
} derive(Eq, Debug)

///|
pub extend CardView with Eq::{equal, not_equal}

///|
pub extend CardView with Debug::{to_repr}

///|
/// The requested lattice of a derivation.
///
/// For `PermissionCard::derive_child` the positive lists name the parent grants
/// to retain and the negative lists name the denials to add. For
/// `derive_from_wallet` and `derive_scope` the four lists are the complete
/// requested lattice.
pub(all) struct Bounds {
  lower_positive : Array[PatternGrant]
  lower_negative : Array[PatternGrant]
  upper_positive : Array[PatternGrant]
  upper_negative : Array[PatternGrant]
} derive(Eq, Debug)

///|
pub extend Bounds with Eq::{equal, not_equal}

///|
pub extend Bounds with Debug::{to_repr}

///|
/// Builds a `Bounds` value; an omitted list is empty.
pub fn Bounds::new(
  lower_positive~ : Array[PatternGrant] = [],
  lower_negative~ : Array[PatternGrant] = [],
  upper_positive~ : Array[PatternGrant] = [],
  upper_negative~ : Array[PatternGrant] = [],
) -> Bounds {
  { lower_positive, lower_negative, upper_positive, upper_negative, }
}

///|
/// The lifespan of a derived card, as an ISO-8601 duration.
///
/// The host accepts only the `PnDTnHnMnS` form: days, hours, minutes and
/// seconds, for example `"P1DT2H"` or `"PT30S"`. It rejects year, month and
/// week units and malformed values with `PermissionError::InvalidPattern`.
pub struct Lifespan {
  priv iso_8601 : String
} derive(Eq, Debug)

///|
pub extend Lifespan with Eq::{equal, not_equal}

///|
pub extend Lifespan with Debug::{to_repr}

///|
/// A lifespan from an ISO-8601 duration string. The host validates the value.
pub fn Lifespan::iso8601(value : String) -> Lifespan {
  { iso_8601: value, }
}

///|
/// A lifespan of the given duration, rendered in the `PnDTnHnMnS` form.
///
/// Fails with `PermissionError::InvalidPattern` when the duration is zero or
/// negative.
pub fn Lifespan::from_duration(
  duration : @schema.Duration,
) -> Lifespan raise PermissionError {
  guard duration.nanoseconds > 0 else {
    raise InvalidPattern(
      "a lifespan must be positive, got \{duration.nanoseconds} nanoseconds",
    )
  }
  {
    iso_8601: duration_to_iso8601(duration.nanoseconds.reinterpret_as_uint64()),
  }
}

///|
/// The ISO-8601 duration sent to the host.
pub fn Lifespan::to_iso8601(self : Lifespan) -> String {
  self.iso_8601
}

///|
fn Lifespan::to_raw(self : Lifespan) -> @raw.Duration {
  { iso_8601: self.iso_8601, }
}

///|
fn lifespan_to_raw(lifespan : Lifespan?) -> @raw.Duration? {
  lifespan.map(Lifespan::to_raw)
}

///|
const NANOS_PER_SECOND : UInt64 = 1_000_000_000

///|
fn duration_to_iso8601(magnitude : UInt64) -> String {
  let total_seconds = magnitude / NANOS_PER_SECOND
  let nanos = magnitude % NANOS_PER_SECOND
  let days = total_seconds / 86400
  let hours = total_seconds % 86400 / 3600
  let minutes = total_seconds % 3600 / 60
  let seconds = total_seconds % 60
  let out = StringBuilder()
  out.write_char('P')
  if days != 0 {
    out.write_string("\{days}D")
  }
  if hours != 0 || minutes != 0 || seconds != 0 || nanos != 0 {
    out.write_char('T')
    if hours != 0 {
      out.write_string("\{hours}H")
    }
    if minutes != 0 {
      out.write_string("\{minutes}M")
    }
    if seconds != 0 || nanos != 0 {
      out.write_string(seconds.to_string())
      if nanos != 0 {
        // Render the fraction with up to nine digits and no trailing zeros.
        let mut digits = nanos
        let mut width = 9
        while digits % 10 == 0 {
          digits = digits / 10
          width = width - 1
        }
        let text = digits.to_string()
        out.write_char('.')
        for _ in text.length()..<width {
          out.write_char('0')
        }
        out.write_string(text)
      }
      out.write_char('S')
    }
  }
  out.to_string()
}

///|
/// A point in time as seconds and nanoseconds since the Unix epoch.
pub(all) struct Timestamp {
  seconds : Int64
  nanoseconds : UInt32
} derive(Eq, Compare, Debug)

///|
pub extend Timestamp with Eq::{equal, not_equal}

///|
pub extend Timestamp with Compare::{compare, op_lt, op_le, op_gt, op_ge}

///|
pub extend Timestamp with Debug::{to_repr}

///|
/// A token that changes whenever the set of cards in the wallet changes.
pub(all) struct WalletVersionToken {
  wallet_id_hash : Bytes
  generation : UInt64
} derive(Eq, Debug)

///|
pub extend WalletVersionToken with Eq::{equal, not_equal}

///|
pub extend WalletVersionToken with Debug::{to_repr}

///|
/// A resource class known to the permission kernel, with its verbs.
pub(all) struct KernelModule {
  class_name : String
  verbs : Array[String]
} derive(Eq, Debug)

///|
pub extend KernelModule with Eq::{equal, not_equal}

///|
pub extend KernelModule with Debug::{to_repr}

///|
/// The holder that receives an installed card.
///
/// The host currently accepts only `Agent` targets. `Account` and `App`
/// targets fail with `PermissionError::NotPermitted`.
pub(all) enum Holder {
  Account(@types.Uuid)
  App(@types.Uuid)
  Agent(@types.AgentId)
} derive(Eq, Debug)

///|
pub extend Holder with Eq::{equal, not_equal}

///|
pub extend Holder with Debug::{to_repr}

///|
fn Holder::to_raw(self : Holder) -> @raw.Holder {
  match self {
    Account(uuid) => Account(uuid)
    App(uuid) => App(uuid)
    Agent(agent_id) => Agent(agent_id)
  }
}

///|
/// The identifier of a card. It is opaque: it supports only equality and
/// hashing, so a guest can compare `PermissionCard::id` with
/// `PermissionCard::parents`. There is no way to build a `CardId` or to read
/// the identifier it holds, and no way to turn it into a card. Printing,
/// debugging and JSON output show the redacted placeholder
/// `CardId(<redacted>)`.
pub struct CardId {
  priv uuid : @types.Uuid
}

///|
pub extend CardId with Eq::{equal, not_equal}

///|
pub extend CardId with Hash::{hash, hash_combine}

///|
pub extend CardId with Debug::{to_repr}

///|
pub extend CardId with Show::{output, to_string}

///|
pub extend CardId with ToJson::{to_json}

///|
const REDACTED_CARD_ID : String = "CardId(<redacted>)"

///|
pub impl Eq for CardId with fn equal(self, other) {
  self.uuid.high_bits == other.uuid.high_bits &&
  self.uuid.low_bits == other.uuid.low_bits
}

///|
pub impl Hash for CardId with fn hash_combine(self, hasher) {
  hasher.combine_uint64(self.uuid.high_bits)
  hasher.combine_uint64(self.uuid.low_bits)
}

///|
pub impl Show for CardId with fn output(_self, logger) {
  logger.write_string(REDACTED_CARD_ID)
}

///|
pub impl Debug for CardId with fn to_repr(_self) {
  Repr::literal(REDACTED_CARD_ID)
}

///|
pub impl ToJson for CardId with fn to_json(_self) {
  Json::string(REDACTED_CARD_ID)
}

///|
fn CardId::from_raw(raw : @types.CardId) -> CardId {
  { uuid: raw.uuid, }
}

///|
/// A concrete permission card held by this agent: every grant names concrete
/// owners and resources.
///
/// The card wraps the owned host resource in a take-once holder. Borrowing
/// operations (`id`, `parents`, `expires_at`, `inspect`, `derive_child`, and
/// use as a parent in `derive_scope`) keep the card. Consuming operations
/// (`revoke`, `install`, `drop`, and passing the card as an agent or tool
/// argument or return value) move it. Using a moved card aborts the
/// invocation. A failed encode of an agent or tool value leaves the card with
/// its holder.
///
/// There is no public constructor, and a card has no JSON codec. A card value
/// can travel only as an owned resource handle. Decoding a value never yields
/// a second card for a handle that a card already holds.
///
/// No operation turns a `PermissionCard` into a `PolymorphicPermissionCard` or
/// a `ScopeCard`, and a schema value of a held card decodes only into the
/// card's own type. A card that moves into a wire tree is no longer held: the
/// type that decodes the tree decides its kind, as for a card that an agent
/// receives. The host keeps the real kind of every card.
pub struct PermissionCard {
  priv handle : @card.GuestPermissionCardHandle
}

///|
const CARD_MOVED : String = "permission card has already been moved (revoked, installed, dropped or transferred) and can no longer be used"

///|
fn borrow_held(
  handle : @card.GuestPermissionCardHandle,
) -> @types.PermissionCard {
  match @card.borrow(handle) {
    Some(raw) => raw
    None => abort(CARD_MOVED)
  }
}

///|
fn take_held(handle : @card.GuestPermissionCardHandle) -> @types.PermissionCard {
  match @card.take(handle) {
    Some(raw) => raw
    None => abort(CARD_MOVED)
  }
}

///|
fn drop_held(handle : @card.GuestPermissionCardHandle) -> Unit {
  match @card.take(handle) {
    Some(raw) => raw.drop()
    None => ()
  }
}

///|
fn timestamp_from_raw(raw : @raw.Timestamp) -> Timestamp {
  { seconds: raw.seconds, nanoseconds: raw.nanoseconds, }
}

///|
fn derive_child_raw(
  parent : @types.PermissionCard,
  bounds : Bounds,
  lifespan : Lifespan?,
) -> PersistentCard raise PermissionError {
  let raw = lift_result(
    @rawDerive.derive_(
      parent,
      grants_to_raw(bounds.lower_positive),
      grants_to_raw(bounds.lower_negative),
      grants_to_raw(bounds.upper_positive),
      grants_to_raw(bounds.upper_negative),
      lifespan_to_raw(lifespan),
    ),
  )
  PersistentCard::from_raw(raw)
}

///|
fn PermissionCard::from_raw(raw : @types.PermissionCard) -> PermissionCard {
  let handle = @card.adopt(raw)
  ignore(@card.claim_kind(handle, false))
  { handle, }
}

///|
fn PermissionCard::borrow_raw(self : PermissionCard) -> @types.PermissionCard {
  borrow_held(self.handle)
}

///|
/// The identifier of this card.
///
/// Traps if the host does not know the handle. Aborts if this card was moved.
pub fn PermissionCard::id(self : PermissionCard) -> CardId {
  CardId::from_raw(@raw.id(self.borrow_raw()))
}

///|
/// The identifiers of the cards this card was derived from.
///
/// Traps if the host does not know the handle. Aborts if this card was moved.
pub fn PermissionCard::parents(self : PermissionCard) -> Array[CardId] {
  @raw.parents(self.borrow_raw()).map(CardId::from_raw)
}

///|
/// The expiry of this card, or `None` for a card without a lifespan.
///
/// Traps if the host does not know the handle. Aborts if this card was moved.
pub fn PermissionCard::expires_at(self : PermissionCard) -> Timestamp? {
  @raw.expires_at(self.borrow_raw()).map(timestamp_from_raw)
}

///|
/// The complete grant lattice of this card. Requires `card:inspect` authority.
pub fn PermissionCard::inspect(
  self : PermissionCard,
) -> CardView raise PermissionError {
  view_from_raw(lift_result(@rawInspect.inspect_card(self.borrow_raw())))
}

///|
fn view_from_raw(view : @raw.CardView) -> CardView {
  {
    lower_positive: grants_from_raw(view.lower_positive),
    lower_negative: grants_from_raw(view.lower_negative),
    upper_positive: grants_from_raw(view.upper_positive),
    upper_negative: grants_from_raw(view.upper_negative),
  }
}

///|
/// Derives a narrower child card from this card. This card is kept.
///
/// The positive lists of `bounds` name the parent grants to retain and the
/// negative lists name the denials to add. A child of a card with an expiry
/// needs a `lifespan` that ends no later than the parent. Requires
/// `card:derive` authority.
/// Slots name the eventual holder. The host checks attenuation for every
/// holder and supplies the concrete or template result tag.
pub fn PermissionCard::derive_child(
  self : PermissionCard,
  bounds : Bounds,
  lifespan? : Lifespan,
) -> PersistentCard raise PermissionError {
  derive_child_raw(self.borrow_raw(), bounds, lifespan)
}

///|
/// Revokes this card and every card derived from it, and returns the number of
/// revoked cards. Requires `card:revoke` authority.
///
/// This consumes the card even when the host returns an error, because the host
/// drops the handle in both cases.
pub fn PermissionCard::revoke(
  self : PermissionCard,
) -> UInt32 raise PermissionError {
  lift_result(@rawRevoke.revoke_card(take_held(self.handle)))
}

///|
/// Installs this card in the wallet of `target`. Requires `card:install`
/// authority. The host currently supports only `Holder::Agent` targets.
///
/// This consumes the card even when the host returns an error, because the host
/// drops the handle in both cases.
pub fn PermissionCard::install(
  self : PermissionCard,
  target : Holder,
) -> Unit raise PermissionError {
  lift_result(@rawWallet.install_card(take_held(self.handle), target.to_raw()))
}

///|
/// Whether this card was moved by `revoke`, `install`, `drop`, or a transfer as
/// an agent or tool value. A moved card can no longer be used.
pub fn PermissionCard::is_moved(self : PermissionCard) -> Bool {
  !self.handle.is_present()
}

///|
/// Releases this card's handle without revoking the card. Dropping a moved card
/// does nothing.
pub fn PermissionCard::drop(self : PermissionCard) -> Unit {
  drop_held(self.handle)
}

///|
/// A template permission card held by this agent: its grants may contain slot
/// variables (`?agent`, `?env`, ...), which the host resolves for each holder.
///
/// It has the operations of `PermissionCard`, with the same borrowing and
/// consuming rules, and the same non-forgeability: there is no public
/// constructor, no JSON codec, and decoding never yields a second card for a
/// handle that a card already holds. `derive_child` returns a host-tagged
/// `PersistentCard`. As an agent or tool parameter or return value its schema
/// is `permission-card { polymorphic: true }`; a `PermissionCard` declares
/// `polymorphic: false`. A received card decodes into the type that the
/// parameter or return value declares, and the host refuses an agent call
/// input or output whose card does not match the declared kind.
pub struct PolymorphicPermissionCard {
  priv handle : @card.GuestPermissionCardHandle
}

///|
fn PolymorphicPermissionCard::borrow_raw(
  self : PolymorphicPermissionCard,
) -> @types.PermissionCard {
  borrow_held(self.handle)
}

///|
/// The identifier of this card.
///
/// Traps if the host does not know the handle. Aborts if this card was moved.
pub fn PolymorphicPermissionCard::id(
  self : PolymorphicPermissionCard,
) -> CardId {
  CardId::from_raw(@raw.id(self.borrow_raw()))
}

///|
/// The identifiers of the cards this card was derived from.
///
/// Traps if the host does not know the handle. Aborts if this card was moved.
pub fn PolymorphicPermissionCard::parents(
  self : PolymorphicPermissionCard,
) -> Array[CardId] {
  @raw.parents(self.borrow_raw()).map(CardId::from_raw)
}

///|
/// The expiry of this card, or `None` for a card without a lifespan.
///
/// Traps if the host does not know the handle. Aborts if this card was moved.
pub fn PolymorphicPermissionCard::expires_at(
  self : PolymorphicPermissionCard,
) -> Timestamp? {
  @raw.expires_at(self.borrow_raw()).map(timestamp_from_raw)
}

///|
/// The complete grant lattice of this card, with its slot variables
/// unresolved. Requires `card:inspect` authority.
pub fn PolymorphicPermissionCard::inspect(
  self : PolymorphicPermissionCard,
) -> CardView raise PermissionError {
  view_from_raw(lift_result(@rawInspect.inspect_card(self.borrow_raw())))
}

///|
/// Derives a narrower persistent child card from this card. This card is kept.
///
/// The host accepts a template parent only when this card is in the caller's
/// wallet, so a template card received as an agent or tool value fails with
/// `PermissionError::NotPermitted`. The host resolves the slot variables of
/// this card for the caller, and
/// checks `bounds` against the result. Slots in the requested grants name the
/// eventual holder, not caller metadata. The host checks attenuation for every
/// holder and reports whether the result is concrete or a template. The
/// rules for `bounds` and `lifespan` are those of
/// `PermissionCard::derive_child`. Requires `card:derive` authority.
pub fn PolymorphicPermissionCard::derive_child(
  self : PolymorphicPermissionCard,
  bounds : Bounds,
  lifespan? : Lifespan,
) -> PersistentCard raise PermissionError {
  derive_child_raw(self.borrow_raw(), bounds, lifespan)
}

///|
/// Revokes this card and every card derived from it, and returns the number of
/// revoked cards. Requires `card:revoke` authority.
///
/// This consumes the card even when the host returns an error, because the host
/// drops the handle in both cases.
pub fn PolymorphicPermissionCard::revoke(
  self : PolymorphicPermissionCard,
) -> UInt32 raise PermissionError {
  lift_result(@rawRevoke.revoke_card(take_held(self.handle)))
}

///|
/// Installs a concrete card in the wallet of `target`: the host resolves the
/// slot variables of this card for `target` and installs the result. Requires
/// `card:install` authority. The host currently supports only `Holder::Agent`
/// targets.
///
/// This consumes the card even when the host returns an error, because the host
/// drops the handle in both cases.
pub fn PolymorphicPermissionCard::install(
  self : PolymorphicPermissionCard,
  target : Holder,
) -> Unit raise PermissionError {
  lift_result(@rawWallet.install_card(take_held(self.handle), target.to_raw()))
}

///|
/// Whether this card was moved by `revoke`, `install`, `drop`, or a transfer as
/// an agent or tool value. A moved card can no longer be used.
pub fn PolymorphicPermissionCard::is_moved(
  self : PolymorphicPermissionCard,
) -> Bool {
  !self.handle.is_present()
}

///|
/// Releases this card's handle without revoking the card. Dropping a moved card
/// does nothing.
pub fn PolymorphicPermissionCard::drop(
  self : PolymorphicPermissionCard,
) -> Unit {
  drop_held(self.handle)
}

///|
/// A card in the caller's wallet, labelled with its kind by the host. Each case
/// holds a new owned handle. User code can match a `WalletCard` but cannot
/// build one.
pub enum WalletCard {
  /// A concrete card: every grant names concrete owners and resources.
  Concrete(PermissionCard)
  /// A template card: its grants may contain slot variables, which resolve for
  /// each holder.
  Template(PolymorphicPermissionCard)
  /// The scope card that the caller lent to the current invocation. The host
  /// lists it only when this agent holds `card:inspect`, and retires it at the
  /// end of the invocation.
  Scope(ScopeCard)
}

///|
fn WalletCard::from_raw(raw : @raw.WalletCard) -> WalletCard {
  match raw {
    Concrete(raw) => Concrete(PermissionCard::from_raw(raw))
    Template(raw) => {
      let handle = @card.adopt(raw)
      ignore(@card.claim_kind(handle, true))
      Template({ handle, })
    }
    Scope(raw) => Scope({ handle: @card.adopt_scope(raw), })
  }
}

///|
/// Releases the handle of this wallet entry. Dropping a moved card does
/// nothing.
pub fn WalletCard::drop(self : WalletCard) -> Unit {
  match self {
    Concrete(card) => card.drop()
    Template(card) => card.drop()
    Scope(card) => card.drop()
  }
}

///|
/// The cards installed in the caller's wallet, each labelled with its kind, as
/// new owned handles. Release the cards you do not keep with `drop` or
/// `drop_all`.
///
/// In a callee that holds `card:inspect`, the result also contains the scope
/// card that the caller lent to the current invocation, as a
/// `WalletCard::Scope`. It has the operations of any `ScopeCard`, and the host
/// retires it at the end of the invocation.
pub fn wallet() -> Array[WalletCard] {
  @rawWallet.self_wallet().map(WalletCard::from_raw)
}

///|
/// Releases every card in `cards`. Moved cards are skipped.
pub fn drop_all(cards : Array[WalletCard]) -> Unit {
  for card in cards {
    card.drop()
  }
}

///|
/// The version token of the caller's wallet.
pub fn wallet_version() -> WalletVersionToken {
  let raw = @rawWallet.self_version()
  {
    wallet_id_hash: Bytes::from_array(
      Array::from_fixed_array(raw.wallet_id_hash),
    ),
    generation: raw.generation,
  }
}

///|
/// Derives a new persistent card directly from the caller's wallet.
///
/// The four lists of `bounds` are the complete requested lattice. Fails with
/// `PermissionError::NotPermitted` when the wallet does not grant the requested
/// authority. Slots name the eventual holder, not caller metadata. The host
/// checks attenuation for every holder and supplies the result tag.
pub fn derive_from_wallet(
  bounds : Bounds,
  lifespan? : Lifespan,
) -> PersistentCard raise PermissionError {
  let raw = lift_result(
    @rawDerive.derive_from_wallet(
      grants_to_raw(bounds.lower_positive),
      grants_to_raw(bounds.lower_negative),
      grants_to_raw(bounds.upper_positive),
      grants_to_raw(bounds.upper_negative),
      lifespan_to_raw(lifespan),
    ),
  )
  PersistentCard::from_raw(raw)
}

///|
/// An invocation-local scope card, derived with `derive_scope` or lent to the
/// current invocation by its caller (`WalletCard::Scope`).
///
/// A scope card is valid only during the invocation that derived or received
/// it. Attach it to an awaited RPC call to lend its authority to the callee for
/// that call. After that invocation the host retires the handle. Attaching a
/// retired scope card, per call or as a client default, makes the host deny
/// the call:
/// `@rpc` and the generated typed clients abort with `RPC denied`, and the
/// reflection clients raise `Remote(Denied)`. On a denial the host discards
/// the whole input tree, argument cards included. `derive_scope` with a
/// retired scope card as a parent fails with `PermissionError::NotPermitted`.
/// Dropping a retired scope card is safe.
///
/// A scope card has only borrowing operations (`id`, `parents`, `expires_at`,
/// `inspect`, and use as a parent in `derive_scope`) and `drop`. It cannot be
/// revoked, installed or used as the parent of a persistent card, and it has
/// no schema or wire support, so it cannot be an agent or tool argument or
/// return value. Using a dropped scope card aborts the invocation.
pub struct ScopeCard {
  /// The holder of the scope card's handle. Only the SDK can read the handle
  /// from it.
  handle : @card.ScopeCardHandle
}

///|
const SCOPE_CARD_MOVED : String = "scope card has already been dropped and can no longer be used"

///|
fn ScopeCard::borrow_raw(self : ScopeCard) -> @types.PermissionCard {
  match @card.borrow_scope(self.handle) {
    Some(raw) => raw
    None => abort(SCOPE_CARD_MOVED)
  }
}

///|
/// The identifier of this scope card.
///
/// Traps if the host does not know the handle. Aborts if this scope card was
/// dropped.
pub fn ScopeCard::id(self : ScopeCard) -> CardId {
  CardId::from_raw(@raw.id(self.borrow_raw()))
}

///|
/// The identifiers of the root cards of this scope card: the persistent cards
/// that its authority comes from. A scope-card parent contributes its own root
/// cards, not its identifier.
///
/// Traps if the host does not know the handle. Aborts if this scope card was
/// dropped.
pub fn ScopeCard::parents(self : ScopeCard) -> Array[CardId] {
  @raw.parents(self.borrow_raw()).map(CardId::from_raw)
}

///|
/// The earliest expiry of the root cards of this scope card, or `None` when no
/// root card expires. The scope card stops authorizing then.
///
/// Traps if the host does not know the handle. Aborts if this scope card was
/// dropped.
pub fn ScopeCard::expires_at(self : ScopeCard) -> Timestamp? {
  @raw.expires_at(self.borrow_raw()).map(timestamp_from_raw)
}

///|
/// The complete grant lattice of this scope card. Requires `card:inspect`
/// authority.
pub fn ScopeCard::inspect(self : ScopeCard) -> CardView raise PermissionError {
  view_from_raw(lift_result(@rawInspect.inspect_card(self.borrow_raw())))
}

///|
/// Whether this scope card was dropped. A dropped scope card can no longer be
/// used.
pub fn ScopeCard::is_moved(self : ScopeCard) -> Bool {
  !self.handle.is_present()
}

///|
/// Releases this scope card's handle. Dropping a dropped scope card does
/// nothing.
pub fn ScopeCard::drop(self : ScopeCard) -> Unit {
  match @card.take_scope(self.handle) {
    Some(raw) => raw.drop()
    None => ()
  }
}

///|
/// Derives an invocation-local scope card from one or more parents. The
/// parents are kept. `parents` are concrete persistent cards,
/// `template_parents` are template persistent cards, whose slot variables the
/// host resolves for the caller, and `scope_parents` are scope cards of the
/// current invocation; `ScopeCard::parents` of the result names the root cards
/// of all of them. The four lists of `bounds` are the complete requested
/// lattice, and its grants must be concrete. Slots fail with
/// `PermissionError::InvalidPattern`. The host accepts a persistent
/// parent, concrete or template, only when it is in the caller's wallet, and
/// fails with `PermissionError::NotPermitted` otherwise, for example for a
/// card that this agent derived or received as a value.
/// `ScopeCard::expires_at` of the result is the earliest expiry of those root
/// cards.
///
/// Aborts if a parent was moved.
pub fn derive_scope(
  parents : Array[PermissionCard],
  bounds : Bounds,
  template_parents~ : Array[PolymorphicPermissionCard] = [],
  scope_parents~ : Array[ScopeCard] = [],
) -> ScopeCard raise PermissionError {
  let raw_parents = parents.map(PermissionCard::borrow_raw)
  for template_parent in template_parents {
    raw_parents.push(template_parent.borrow_raw())
  }
  for scope_parent in scope_parents {
    raw_parents.push(scope_parent.borrow_raw())
  }
  let raw = lift_result(
    @rawDerive.derive_scope(
      raw_parents,
      grants_to_raw(bounds.lower_positive),
      grants_to_raw(bounds.lower_negative),
      grants_to_raw(bounds.upper_positive),
      grants_to_raw(bounds.upper_negative),
    ),
  )
  { handle: @card.adopt_scope(raw), }
}

///|
/// Every resource class the permission kernel knows, with its verbs.
pub fn list_modules() -> Array[KernelModule] {
  @rawKernel.list_modules().map(fn(kernel_module) {
    { class_name: kernel_module.class_name, verbs: kernel_module.verbs, }
  })
}

///|
/// Checks that `grant` names a known class, owner, recipient, verb and
/// resource-id pattern.
pub fn validate_grant(grant : PatternGrant) -> Unit raise PermissionError {
  lift_result(@rawKernel.validate_grant(grant.to_raw()))
}
