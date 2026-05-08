///|
pub using @raw {type QuotaTokenRecord}

///|
/// Error returned when a reservation cannot be satisfied because the
/// resource's enforcement policy is `reject`.
pub(all) struct FailedReservation {
  estimated_wait_nanos : UInt64?
}

///|
/// An unforgeable capability granting the right to consume a named
/// resource. Dropping the token releases the underlying lease back to
/// the executor pool.
pub(all) struct QuotaToken {
  priv inner : @raw.QuotaToken
}

///|
/// A short-lived capability that represents a pending or committed
/// resource consumption. Drop without committing is equivalent to
/// committing zero usage.
pub(all) struct Reservation {
  priv inner : @raw.Reservation
}

///|
/// Creates a new quota token for the given resource.
/// - `resource_name` : the resource name (as declared in the manifest).
/// - `expected_use`  : expected units per reservation; used to derive
///                      credit rate and max-credit for fair scheduling.
pub fn QuotaToken::new(
  resource_name : String,
  expected_use : UInt64,
) -> QuotaToken {
  { inner: @raw.QuotaToken::quota_token(resource_name, expected_use) }
}

///|
/// Reserve `amount` units from the local allocation.
///
/// Blocks internally until capacity is available or the resource's
/// enforcement action fires. Returns a `Reservation` handle that
/// must be committed (or dropped) to release unused capacity.
pub fn QuotaToken::reserve(
  self : QuotaToken,
  amount : UInt64,
) -> Result[Reservation, FailedReservation] {
  match self.inner.reserve(amount) {
    Ok(r) => Ok({ inner: r })
    Err(e) => Err({ estimated_wait_nanos: e.estimated_wait_nanos })
  }
}

///|
/// Split off a child token with `child_expected_use` units from this token.
///
/// The parent's `expected-use` is reduced by `child_expected_use`.
/// Credits are divided proportionally between parent and child.
///
/// Traps if `child_expected_use` exceeds the parent's current `expected-use`.
pub fn QuotaToken::split(
  self : QuotaToken,
  child_expected_use : UInt64,
) -> QuotaToken {
  { inner: self.inner.split(child_expected_use) }
}

///|
/// Merge `other` back into this token.
///
/// The two tokens must refer to the same resource (same resource-name
/// and environment). `other` is consumed.
///
/// Traps if the tokens refer to different resources.
pub fn QuotaToken::merge(self : QuotaToken, other : QuotaToken) -> Unit {
  self.inner.merge(other.inner)
}

///|
/// Serialize this token to a `QuotaTokenRecord`.
pub fn QuotaToken::to_record(self : QuotaToken) -> QuotaTokenRecord {
  self.inner.to_record()
}

///|
/// Deserialize a token from a `QuotaTokenRecord`, restoring all associated state.
pub fn QuotaToken::from_record(serialized : QuotaTokenRecord) -> QuotaToken {
  { inner: @raw.QuotaToken::from_record(serialized) }
}

///|
/// Drops the underlying resource handle.
pub fn QuotaToken::drop(self : QuotaToken) -> Unit {
  self.inner.drop()
}

///|
/// Commit actual usage, consuming the reservation.
///
/// If `used` < reserved — unused capacity is returned to the pool.
/// If `used` > reserved — the excess is deducted from the token's
///                         remaining allocation as "debt".
pub fn Reservation::commit(self : Reservation, used : UInt64) -> Unit {
  self.inner.commit(used)
}

///|
/// Drops the underlying resource handle.
pub fn Reservation::drop(self : Reservation) -> Unit {
  self.inner.drop()
}

///|
pub impl @schema.HasElementSchema for QuotaToken with element_schema() {
  let prim_u64 : @types.WitType = {
    nodes: [{ name: None, owner: None, type_: @types.WitTypeNode::PrimU64Type }],
  }
  let prim_s64 : @types.WitType = {
    nodes: [{ name: None, owner: None, type_: @types.WitTypeNode::PrimS64Type }],
  }
  let prim_u32 : @types.WitType = {
    nodes: [{ name: None, owner: None, type_: @types.WitTypeNode::PrimU32Type }],
  }
  let prim_string : @types.WitType = {
    nodes: [
      { name: None, owner: None, type_: @types.WitTypeNode::PrimStringType },
    ],
  }
  let uuid_type = @builder.TypeBuilder::new().record_type([
    ("high-bits", prim_u64),
    ("low-bits", prim_u64),
  ])
  let env_id_type = @builder.TypeBuilder::new().record_type([
    ("uuid", uuid_type),
  ])
  let last_credit_at_type = @builder.TypeBuilder::new().record_type([
    ("seconds", prim_u64),
    ("nanoseconds", prim_u32),
  ])
  @schema.make_record_schema([
    ("environment-id", @types.ElementSchema::ComponentModel(env_id_type)),
    ("resource-name", @types.ElementSchema::ComponentModel(prim_string)),
    ("expected-use", @types.ElementSchema::ComponentModel(prim_u64)),
    ("last-credit", @types.ElementSchema::ComponentModel(prim_s64)),
    (
      "last-credit-at",
      @types.ElementSchema::ComponentModel(last_credit_at_type),
    ),
  ])
}

///|
pub impl @schema.ToElementValue for QuotaToken with to_element_value(self) {
  let r = self.to_record()
  let uuid_value = @builder.Builder::new().record(item => {
    item.u64(r.environment_id.uuid.high_bits)
    item.u64(r.environment_id.uuid.low_bits)
  }) catch {
    @builder.BuilderError(msg) => raise @common.AgentError::InvalidInput(msg)
  }
  let env_id_value = @builder.Builder::new().record(item => {
    item.wit_value(uuid_value)
  }) catch {
    @builder.BuilderError(msg) => raise @common.AgentError::InvalidInput(msg)
  }
  let last_credit_at_value = @builder.Builder::new().record(item => {
    item.u64(r.last_credit_at.seconds)
    item.u32(r.last_credit_at.nanoseconds)
  }) catch {
    @builder.BuilderError(msg) => raise @common.AgentError::InvalidInput(msg)
  }
  let wit_value = @builder.Builder::new().record(item => {
    item.wit_value(env_id_value)
    item.string(r.resource_name)
    item.u64(r.expected_use)
    item.s64(r.last_credit)
    item.wit_value(last_credit_at_value)
  }) catch {
    @builder.BuilderError(msg) => raise @common.AgentError::InvalidInput(msg)
  }
  @types.ElementValue::ComponentModel(wit_value)
}

///|
pub impl @schema.FromExtractor for QuotaToken with from_extractor(e) {
  guard e.field(0) is Some(env_id_e) else {
    raise @common.AgentError::InvalidInput(
      "Expected field environment-id at index 0",
    )
  }
  guard env_id_e.field(0) is Some(uuid_e) else {
    raise @common.AgentError::InvalidInput("Expected field uuid at index 0")
  }
  guard uuid_e.field(0) is Some(hb_e) else {
    raise @common.AgentError::InvalidInput(
      "Expected field high-bits at index 0",
    )
  }
  guard hb_e.u64() is Some(high_bits) else {
    raise @common.AgentError::InvalidInput("Expected U64 for high-bits")
  }
  guard uuid_e.field(1) is Some(lb_e) else {
    raise @common.AgentError::InvalidInput("Expected field low-bits at index 1")
  }
  guard lb_e.u64() is Some(low_bits) else {
    raise @common.AgentError::InvalidInput("Expected U64 for low-bits")
  }
  guard e.field(1) is Some(rn_e) else {
    raise @common.AgentError::InvalidInput(
      "Expected field resource-name at index 1",
    )
  }
  guard rn_e.string() is Some(resource_name) else {
    raise @common.AgentError::InvalidInput("Expected String for resource-name")
  }
  guard e.field(2) is Some(eu_e) else {
    raise @common.AgentError::InvalidInput(
      "Expected field expected-use at index 2",
    )
  }
  guard eu_e.u64() is Some(expected_use) else {
    raise @common.AgentError::InvalidInput("Expected U64 for expected-use")
  }
  guard e.field(3) is Some(lc_e) else {
    raise @common.AgentError::InvalidInput(
      "Expected field last-credit at index 3",
    )
  }
  guard lc_e.s64() is Some(last_credit) else {
    raise @common.AgentError::InvalidInput("Expected S64 for last-credit")
  }
  guard e.field(4) is Some(lca_e) else {
    raise @common.AgentError::InvalidInput(
      "Expected field last-credit-at at index 4",
    )
  }
  guard lca_e.field(0) is Some(sec_e) else {
    raise @common.AgentError::InvalidInput("Expected field seconds at index 0")
  }
  guard sec_e.u64() is Some(seconds) else {
    raise @common.AgentError::InvalidInput("Expected U64 for seconds")
  }
  guard lca_e.field(1) is Some(ns_e) else {
    raise @common.AgentError::InvalidInput(
      "Expected field nanoseconds at index 1",
    )
  }
  guard ns_e.u32() is Some(nanoseconds) else {
    raise @common.AgentError::InvalidInput("Expected U32 for nanoseconds")
  }
  let record : @raw.QuotaTokenRecord = {
    environment_id: @host.EnvironmentId::{
      uuid: @types.Uuid::{ high_bits, low_bits },
    },
    resource_name,
    expected_use,
    last_credit,
    last_credit_at: @wallClock.Datetime::{ seconds, nanoseconds },
  }
  QuotaToken::from_record(record)
}

///|
pub impl @schema.FromElementValue for QuotaToken with from_element_value(v) {
  @schema.FromExtractor::from_extractor(
    @extractor.extract_component_model_value(v),
  )
}

///|
/// Reserve `amount` units, run `f` to perform work, then commit actual usage.
///
/// `f` receives the reservation and returns `(used, value)` where `used` is
/// the actual number of units consumed. The reservation is committed with
/// `used` and `value` is returned. On reservation failure, returns `Err`.
pub fn[T] with_reservation(
  token : QuotaToken,
  amount : UInt64,
  f : (Reservation) -> (UInt64, T),
) -> Result[T, FailedReservation] {
  match token.reserve(amount) {
    Ok(reservation) => {
      let (used, value) = f(reservation)
      reservation.commit(used)
      Ok(value)
    }
    Err(e) => Err(e)
  }
}
