// Recursive, in-memory mirror of the `golem:core/types@2.0.0` schema model.
//
// The WIT carrier (`schema-graph` / `schema-value-tree`) is flat-with-indices
// because WIT has no recursive types. This module defines the ergonomic
// recursive forms (`SchemaType`, `SchemaValue`, `SchemaGraph`,
// `TypedSchemaValue`) the rest of the SDK works with; `wit.mbt` converts to and
// from the flat carrier mechanically.
//
// Non-recursive leaf structures (restrictions, specs, metadata, capability
// payloads, datetime, …) are reused directly from the generated bindings
// (`@types`) so field names and shapes match the wire format exactly and the
// codecs can pass them through unchanged.

// ============================================================
// Schema type (recursive)
// ============================================================

///|
/// A stable, language-independent identifier for a named type definition.
/// Conventionally a package-qualified dotted type name.
pub type TypeId = String

///|
/// A schema type node: a structural body plus its metadata envelope.
pub(all) struct SchemaType {
  body : SchemaTypeBody
  metadata : @types.MetadataEnvelope
} derive(Debug, Eq)

///|
pub extend SchemaType with Debug::{to_repr}

///|
pub extend SchemaType with Eq::{equal, not_equal}

///|
/// The structural body of a recursive schema type. Recursive positions hold
/// nested `SchemaType`s directly (the flat carrier holds indices instead).
pub(all) enum SchemaTypeBody {
  /// Reference to a named definition in the enclosing `SchemaGraph`.
  Ref(TypeId)
  // Primitives
  Bool
  S8(@types.NumericRestrictions?)
  S16(@types.NumericRestrictions?)
  S32(@types.NumericRestrictions?)
  S64(@types.NumericRestrictions?)
  U8(@types.NumericRestrictions?)
  U16(@types.NumericRestrictions?)
  U32(@types.NumericRestrictions?)
  U64(@types.NumericRestrictions?)
  F32(@types.NumericRestrictions?)
  F64(@types.NumericRestrictions?)
  Char
  String
  // Structural composites
  Record(Array[NamedFieldType])
  Variant(Array[VariantCaseType])
  Enum(Array[String])
  Flags(Array[String])
  Tuple(Array[SchemaType])
  List(SchemaType)
  FixedList(SchemaType, UInt32)
  Map(SchemaType, SchemaType)
  Option(SchemaType)
  // ok, err (each optional: unit ok/err omit the payload)
  Result(SchemaType?, SchemaType?)
  // Rich semantic types
  Text(@types.TextRestrictions)
  Binary(@types.BinaryRestrictions)
  Path(@types.PathSpec)
  Url(@types.UrlRestrictions)
  Uuid
  Datetime
  Duration
  Quantity(@types.QuantitySpec)
  // Discriminated union (closed, inferred-tag)
  Union(Array[UnionBranch])
  // Capability nodes
  Secret(SecretSpec)
  QuotaToken(@types.QuotaTokenSpec)
  PermissionCard(@types.PermissionCardSpec)
  // WASI P3 asynchronous values
  Future(SchemaType?)
  Stream(SchemaType?)
} derive(Debug, Eq)

///|
pub extend SchemaTypeBody with Debug::{to_repr}

///|
pub extend SchemaTypeBody with Eq::{equal, not_equal}

///|
pub(all) struct NamedFieldType {
  name : String
  body : SchemaType
  metadata : @types.MetadataEnvelope
} derive(Debug, Eq)

///|
pub extend NamedFieldType with Debug::{to_repr}

///|
pub extend NamedFieldType with Eq::{equal, not_equal}

///|
pub(all) struct VariantCaseType {
  name : String
  payload : SchemaType?
  metadata : @types.MetadataEnvelope
} derive(Debug, Eq)

///|
pub extend VariantCaseType with Debug::{to_repr}

///|
pub extend VariantCaseType with Eq::{equal, not_equal}

///|
pub(all) struct UnionBranch {
  /// Logical branch name (carried in `union` values).
  tag : String
  body : SchemaType
  discriminator : @types.DiscriminatorRule
  metadata : @types.MetadataEnvelope
} derive(Debug, Eq)

///|
pub extend UnionBranch with Debug::{to_repr}

///|
pub extend UnionBranch with Eq::{equal, not_equal}

///|
pub(all) struct SecretSpec {
  /// Revealed payload type carried by this secret handle.
  inner : SchemaType
  category : String?
} derive(Debug, Eq)

///|
pub extend SecretSpec with Debug::{to_repr}

///|
pub extend SecretSpec with Eq::{equal, not_equal}

// ============================================================
// Schema graph (recursive, self-contained)
// ============================================================

///|
/// A named type definition inside a `SchemaGraph`. `id` is the stable
/// identifier `Ref` bodies point at; `name` is display-only.
pub(all) struct SchemaTypeDef {
  id : TypeId
  /// Optional human-readable qualified name (display only).
  name : String?
  body : SchemaType
} derive(Debug, Eq)

///|
pub extend SchemaTypeDef with Debug::{to_repr}

///|
pub extend SchemaTypeDef with Eq::{equal, not_equal}

///|
/// A self-contained schema graph: an ordered registry of named definitions
/// plus a root type. `Ref(id)` bodies reference the `defs` entry with that
/// `id`. Definition order is the first-seen / commit order produced by the
/// recursive walk (it mirrors the Rust `golem-schema` `Vec<SchemaTypeDef>`);
/// it is deterministic and is NOT sorted.
pub(all) struct SchemaGraph {
  defs : Array[SchemaTypeDef]
  root : SchemaType
} derive(Debug, Eq)

///|
pub extend SchemaGraph with Debug::{to_repr}

///|
pub extend SchemaGraph with Eq::{equal, not_equal}

// ============================================================
// Schema value (recursive)
// ============================================================

///|
/// A recursive schema value. Always conceptually paired with a `SchemaGraph`
/// (see `TypedSchemaValue`).
pub(all) enum SchemaValue {
  // Primitives
  Bool(Bool)
  S8(Int32)
  S16(Int32)
  S32(Int32)
  S64(Int64)
  U8(Byte)
  U16(UInt32)
  U32(UInt32)
  U64(UInt64)
  F32(Float)
  F64(Double)
  Char(Char)
  String(String)
  // Structural composites
  Record(Array[SchemaValue])
  // case index, optional payload
  Variant(UInt32, SchemaValue?)
  Enum(UInt32)
  Flags(Array[Bool])
  Tuple(Array[SchemaValue])
  List(Array[SchemaValue])
  FixedList(Array[SchemaValue])
  Map(Array[SchemaMapEntry])
  Option(SchemaValue?)
  // result<ok, err>: exactly one of these, inner optional for unit payloads
  ResultOk(SchemaValue?)
  ResultErr(SchemaValue?)
  // Rich semantic
  // text, language
  Text(String, String?)
  // bytes, mime-type
  Binary(FixedArray[Byte], String?)
  Path(String)
  Url(String)
  Uuid(@types.Uuid)
  Datetime(@types.Datetime)
  // total nanoseconds (signed)
  Duration(Int64)
  Quantity(@types.QuantityValue)
  // discriminator literal tag, body
  Union(String, SchemaValue)
  // owned `secret` resource carried as an opaque, affine handle
  Secret(GuestSecretHandle)
  // owned `quota-token` resource carried as an opaque, affine handle
  QuotaToken(GuestQuotaTokenHandle)
  // native stream endpoint, retained without proxying or materialization
  Stream(GuestSchemaValueStreamHandle)
  // owned `permission-card` resource carried as an opaque, affine handle
  PermissionCard(@card.GuestPermissionCardHandle)
} derive(Debug, Eq)

///|
pub extend SchemaValue with Debug::{to_repr}

///|
pub extend SchemaValue with Eq::{equal, not_equal}

///|
pub(all) struct SchemaMapEntry {
  key : SchemaValue
  value : SchemaValue
} derive(Debug, Eq)

///|
pub extend SchemaMapEntry with Debug::{to_repr}

///|
pub extend SchemaMapEntry with Eq::{equal, not_equal}

// ============================================================
// Wire carrier (recursive)
// ============================================================

///|
/// A typed value: a self-contained schema graph paired with a value built
/// against that schema.
pub(all) struct TypedSchemaValue {
  graph : SchemaGraph
  value : SchemaValue
} derive(Debug, Eq)

///|
pub extend TypedSchemaValue with Debug::{to_repr}

///|
pub extend TypedSchemaValue with Eq::{equal, not_equal}

///|
/// Host-provided cleanup for owned resources rejected at a schema-value
/// boundary.
pub(all) struct SchemaValueResourceDrops {
  drop_secret : (@types.Secret) -> Unit
  drop_quota_token : (@types.QuotaToken) -> Unit
  drop_stream : (@types.SchemaValueStream) -> Unit
  drop_native_stream : (@async-core.Stream[@types.SchemaValueTree]) -> Unit
}

///|
/// Release every resource still owned by this value. Handles are take-once, so
/// resources already transferred out of the value are left alone and repeated
/// cleanup is harmless.
pub fn SchemaValue::release_resources(
  self : SchemaValue,
  drops : SchemaValueResourceDrops,
) -> Unit {
  self.release_owned(drops, false)
}

///|
/// Rolls back a failed invocation-input encode: releases every resource still
/// owned by this value except permission cards. The host never took the cards,
/// so they stay with their holders and the caller keeps them.
pub fn SchemaValue::rollback_input_resources(
  self : SchemaValue,
  drops : SchemaValueResourceDrops,
) -> Unit {
  self.release_owned(drops, true)
}

///|
fn SchemaValue::release_owned(
  self : SchemaValue,
  drops : SchemaValueResourceDrops,
  keep_permission_cards : Bool,
) -> Unit {
  match self {
    Record(values) | Tuple(values) | List(values) | FixedList(values) =>
      for value in values {
        value.release_owned(drops, keep_permission_cards)
      }
    Variant(_, payload)
    | Option(payload)
    | ResultOk(payload)
    | ResultErr(payload) =>
      match payload {
        Some(value) => value.release_owned(drops, keep_permission_cards)
        None => ()
      }
    Map(entries) =>
      for entry in entries {
        entry.key.release_owned(drops, keep_permission_cards)
        entry.value.release_owned(drops, keep_permission_cards)
      }
    Union(_, body) => body.release_owned(drops, keep_permission_cards)
    Secret(handle) =>
      match handle.take() {
        Some(value) => (drops.drop_secret)(value)
        None => ()
      }
    QuotaToken(handle) =>
      match handle.take() {
        Some(value) => (drops.drop_quota_token)(value)
        None => ()
      }
    PermissionCard(handle) =>
      if !keep_permission_cards {
        match @card.take(handle) {
          Some(value) => value.drop()
          None => ()
        }
      }
    Stream(handle) =>
      handle.release(drops.drop_stream, drops.drop_native_stream)
    _ => ()
  }
}

// ============================================================
// Constructors / helpers
// ============================================================

///|
/// An empty metadata envelope (no docs / aliases / examples / deprecation / role).
pub fn empty_metadata() -> @types.MetadataEnvelope {
  { doc: None, aliases: [], examples: [], deprecated: None, role: None, }
}

///|
/// Wrap a body into a `SchemaType` node, defaulting to empty metadata.
pub fn schema_type(
  body : SchemaTypeBody,
  metadata~ : @types.MetadataEnvelope = empty_metadata(),
) -> SchemaType {
  { body, metadata, }
}

///|
/// A record field with the given name and (empty by default) metadata.
pub fn field(
  name : String,
  body : SchemaType,
  metadata~ : @types.MetadataEnvelope = empty_metadata(),
) -> NamedFieldType {
  { name, body, metadata, }
}

///|
/// A variant case with an optional payload and (empty by default) metadata.
pub fn variant_case(
  name : String,
  payload? : SchemaType,
  metadata~ : @types.MetadataEnvelope = empty_metadata(),
) -> VariantCaseType {
  { name, payload, metadata, }
}
