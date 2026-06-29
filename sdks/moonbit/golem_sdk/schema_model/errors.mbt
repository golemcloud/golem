// Structured errors for the recursive schema model and its WIT codecs.
//
// These never silently drop information: every failure path raises one of these
// with a descriptive message so callers can surface a precise diagnostic.

///|
/// Raised by the schema model, its builder, and its WIT codecs.
pub suberror SchemaModelError {
  /// Encoding an in-memory schema/value into its flat WIT carrier failed.
  EncodeError(String)
  /// Decoding a flat WIT carrier into the in-memory model failed.
  DecodeError(String)
  /// Merging per-root graphs found two definitions sharing a `type-id` but with
  /// structurally different bodies. Carries the conflicting `type-id`.
  MergeConflict(String)
  /// A `SchemaBuilder` reservation was never committed before `finish`.
  BuilderError(String)
} derive(Show, Eq)
