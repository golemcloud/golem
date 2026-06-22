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
/// A `QuotaToken` is a first-class capability node in the schema model
/// (`SchemaType::quota-token` / `SchemaValue::quota-token`), so its schema is a
/// single capability node rather than a structural record. Mirrors the Rust SDK
/// `impl IntoSchema for QuotaToken`.
pub impl @schema.IntoSchema for QuotaToken with type_id() {
  "golem.core.QuotaToken"
}

///|
pub impl @schema.IntoSchema for QuotaToken with register_in(_b) {
  @model.schema_type(QuotaToken({ resource_name: None }))
}

///|
pub impl @schema.IntoSchema for QuotaToken with to_value(self) {
  QuotaToken(quota_token_payload_from_record(self.to_record()))
}

///|
/// Mirrors the Rust SDK `impl FromSchema for QuotaToken`: only a
/// `SchemaValue::quota-token` payload reconstructs a token; the timestamp must
/// be non-negative.
pub impl @schema.FromSchema for QuotaToken with from_value(v) {
  match v {
    QuotaToken(payload) =>
      QuotaToken::from_record(quota_token_record_from_payload(payload))
    other =>
      raise @schema.FromSchemaError::shape_mismatch(
        "quota-token",
        @schema.value_kind(other),
        "QuotaToken",
      )
  }
}

///|
/// Pure mapping from the host `QuotaTokenRecord` to the schema value carrier.
/// The `uuid` is the same `@types.Uuid`; only `Datetime.seconds` differs in
/// signedness (`UInt64` → `Int64`), bridged by bit-pattern reinterpret (matches
/// the Rust `seconds as i64`). Factored out so it can be tested without the
/// `to_record` host import.
fn quota_token_payload_from_record(
  r : @raw.QuotaTokenRecord,
) -> @types.QuotaTokenValuePayload {
  {
    environment_id: { uuid: r.environment_id.uuid },
    resource_name: r.resource_name,
    expected_use: r.expected_use,
    last_credit: r.last_credit,
    last_credit_at: {
      seconds: r.last_credit_at.seconds.reinterpret_as_int64(),
      nanoseconds: r.last_credit_at.nanoseconds,
    },
  }
}

///|
/// Pure mapping from the schema value carrier back to the host
/// `QuotaTokenRecord`. Mirrors the Rust non-negative-timestamp guard before the
/// `Int64` → `UInt64` reinterpret. Factored out so it can be tested without the
/// `from_record` host import.
fn quota_token_record_from_payload(
  payload : @types.QuotaTokenValuePayload,
) -> @raw.QuotaTokenRecord raise @schema.FromSchemaError {
  guard payload.last_credit_at.seconds >= 0L else {
    raise @schema.FromSchemaError::custom(
      "quota token timestamp must be non-negative",
    )
  }
  {
    environment_id: @host.EnvironmentId::{ uuid: payload.environment_id.uuid },
    resource_name: payload.resource_name,
    expected_use: payload.expected_use,
    last_credit: payload.last_credit,
    last_credit_at: @wallClock.Datetime::{
      seconds: payload.last_credit_at.seconds.reinterpret_as_uint64(),
      nanoseconds: payload.last_credit_at.nanoseconds,
    },
  }
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
