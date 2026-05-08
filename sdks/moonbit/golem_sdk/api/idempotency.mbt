///|
/// Gets the current idempotence mode. See `set_idempotence_mode` for details.
pub fn get_idempotence_mode() -> Bool {
  @raw.get_idempotence_mode()
}

///|
/// Sets the current idempotence mode. The default is true.
/// True means side-effects are treated idempotent and Golem guarantees
/// at-least-once semantics. False provides at-most-once semantics, failing
/// the agent if it is not known whether the side effect was already executed.
pub fn set_idempotence_mode(idempotent : Bool) -> Unit {
  @raw.set_idempotence_mode(idempotent)
}

///|
/// Runs a function with the given idempotence mode, restoring the previous
/// mode afterward (even if the function raises).
pub fn[T] with_idempotence_mode(idempotent : Bool, f : () -> T) -> T {
  let prev = @raw.get_idempotence_mode()
  @raw.set_idempotence_mode(idempotent)
  defer @raw.set_idempotence_mode(prev)
  f()
}

///|
/// Generates an idempotency key. This operation will never be replayed —
/// the key is persisted and committed, such that it can be used in
/// third-party systems (e.g. payment processing) to introduce idempotence.
pub fn generate_idempotency_key() -> @types.Uuid {
  @raw.generate_idempotency_key()
}

///|
/// Generates an idempotency key and returns it as a standard UUID string.
pub fn generate_idempotency_key_string() -> String {
  @types.uuid_to_string(@raw.generate_idempotency_key())
}
