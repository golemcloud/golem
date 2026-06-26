///|
/// Error returned when a reservation cannot be satisfied because the
/// resource's enforcement policy is `reject`.
pub(all) struct FailedReservation {
  estimated_wait_nanos : UInt64?
}

///|
/// An unforgeable capability granting the right to consume a named
/// resource.
///
/// The underlying owned `golem:core/types.quota-token` resource is held inside
/// an opaque, affine (take-once) handle: through this supported API guest code
/// can hold and transfer a token, but can never inspect or forge it. Passing a
/// token across an RPC / agent boundary moves the owned resource out of the
/// handle exactly once; after that the originating token is consumed and using
/// it again traps. Split the token first if you need to both keep and send a
/// capability.
pub(all) struct QuotaToken {
  priv handle : @model.GuestQuotaTokenHandle
}

///|
/// A short-lived capability that represents a pending or committed
/// resource consumption. Drop without committing is equivalent to
/// committing zero usage.
pub(all) struct Reservation {
  priv inner : @raw.Reservation
}

///|
const TOKEN_CONSUMED : String = "quota token has already been transferred and can no longer be used; split the token first if you need to both keep and send a capability"

///|
/// Creates a new quota token for the given resource.
/// - `resource_name` : the resource name (as declared in the manifest).
/// - `expected_use`  : expected units per reservation; used to derive
///                      credit rate and max-credit for fair scheduling.
pub fn QuotaToken::new(
  resource_name : String,
  expected_use : UInt64,
) -> QuotaToken {
  {
    handle: @model.GuestQuotaTokenHandle::new(
      @raw.new_token(resource_name, expected_use),
    ),
  }
}

///|
/// Reserve `amount` units from the local allocation.
///
/// Blocks internally until capacity is available or the resource's
/// enforcement action fires. Returns a `Reservation` handle that
/// must be committed (or dropped) to release unused capacity.
///
/// Traps if this token has already been transferred (e.g. sent through an RPC
/// call or returned from a method).
pub fn QuotaToken::reserve(
  self : QuotaToken,
  amount : UInt64,
) -> Result[Reservation, FailedReservation] {
  match self.handle.with_handle(h => @raw.reserve(h, amount)) {
    Some(Ok(r)) => Ok({ inner: r })
    Some(Err(e)) => Err({ estimated_wait_nanos: e.estimated_wait_nanos })
    None => abort(TOKEN_CONSUMED)
  }
}

///|
/// Split off a child token with `child_expected_use` units from this token.
///
/// The parent's `expected-use` is reduced by `child_expected_use`.
/// Credits are divided proportionally between parent and child.
///
/// Traps if `child_expected_use` exceeds the parent's current `expected-use`,
/// or if this token has already been transferred.
pub fn QuotaToken::split(
  self : QuotaToken,
  child_expected_use : UInt64,
) -> QuotaToken {
  match self.handle.with_handle(h => @raw.split(h, child_expected_use)) {
    Some(raw) => { handle: @model.GuestQuotaTokenHandle::new(raw) }
    None => abort(TOKEN_CONSUMED)
  }
}

///|
/// Merge `other` back into this token.
///
/// The two tokens must refer to the same resource (same resource-name
/// and environment). `other` is consumed.
///
/// Traps if the tokens refer to different resources, if either token has
/// already been transferred, or if a token is merged with itself.
pub fn QuotaToken::merge(self : QuotaToken, other : QuotaToken) -> Unit {
  // Reject merging a token into itself before taking any handle, so a shared
  // handle is not consumed as `other` and then borrowed again as the receiver.
  if physical_equal(self.handle, other.handle) {
    abort("cannot merge a quota token with itself")
  }
  // Check the receiver first so a consumed receiver does not consume `other`.
  guard self.handle.is_present() else { abort(TOKEN_CONSUMED) }
  let other_raw = match other.handle.take() {
    Some(raw) => raw
    None => abort(TOKEN_CONSUMED)
  }
  let _ = self.handle.with_handle(h => @raw.merge(h, other_raw))
}

///|
/// Drops the underlying resource handle, releasing the lease back to the
/// executor pool. A token that was already transferred is a no-op.
pub fn QuotaToken::drop(self : QuotaToken) -> Unit {
  match self.handle.take() {
    Some(raw) => raw.drop()
    None => ()
  }
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
/// Lower the token into a schema value by sharing its opaque owned handle. The
/// underlying owned resource is not moved here; it is taken out of the handle
/// exactly once when the resulting `SchemaValue` is encoded into a WIT
/// `schema-value-tree`. Mirrors the Rust/TS SDK `to_value` / `_toSchemaValue`.
pub impl @schema.IntoSchema for QuotaToken with to_value(self) {
  QuotaToken(self.handle)
}

///|
/// Reconstruct a token from a decoded schema value's opaque handle. Mirrors the
/// Rust SDK `impl FromSchema for QuotaToken` / TS `_fromSchemaValue`: only a
/// `SchemaValue::quota-token` reconstructs a token.
pub impl @schema.FromSchema for QuotaToken with from_value(v) {
  match v {
    QuotaToken(handle) => { handle, }
    other =>
      raise @schema.FromSchemaError::shape_mismatch(
        "quota-token",
        @schema.value_kind(other),
        "QuotaToken",
      )
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
