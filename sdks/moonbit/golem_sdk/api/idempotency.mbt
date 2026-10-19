///|
/// Generates an idempotency key. This operation will never be replayed —
/// the key is persisted and committed, such that it can be used in
/// third-party systems (e.g. payment processing) to introduce idempotence.
pub fn generate_idempotency_key() -> @schema.Uuid {
  @schema.Uuid::from_wit(@raw.generate_idempotency_key())
}

///|
/// Generates an idempotency key and returns it as a standard UUID string.
pub fn generate_idempotency_key_string() -> String {
  generate_idempotency_key().to_string()
}
