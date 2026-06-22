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
pub typealias String as TypeId

///|
/// A schema type node: a structural body plus its metadata envelope.
pub(all) struct SchemaType {
  body : SchemaTypeBody
  metadata : @types.MetadataEnvelope
} derive(Show, Eq)

///|
/// The structural body of a recursive schema type. Recursive positions hold
/// nested `SchemaType`s directly (the flat carrier holds indices instead).
pub(all) enum SchemaTypeBody {
  /// Reference to a named definition in the enclosing `SchemaGraph`.
  Ref(TypeId)
  // Primitives
  Bool
  S8
  S16
  S32
  S64
  U8
  U16
  U32
  U64
  F32
  F64
  Char
  String
  // Structural composites
  Record(Array[NamedFieldType])
  Variant(Array[VariantCaseType])
  Enum(Array[String])
  Flags(Array[String])
  Tuple(Array[SchemaType])
  List(SchemaType)
  FixedList(SchemaType, UInt)
  Map(SchemaType, SchemaType)
  Option(SchemaType)
  // ok, err (each optional: unit ok/err omit the payload)
  Result(SchemaType?, SchemaType?)
  // Rich semantic types
  Text(@types.TextRestrictions)
  Binary(@types.BinaryRestrictions)
  Path(@types.PathSpec)
  Url(@types.UrlRestrictions)
  Datetime
  Duration
  Quantity(@types.QuantitySpec)
  // Discriminated union (closed, inferred-tag)
  Union(Array[UnionBranch])
  // Capability nodes
  Secret(@types.SecretSpec)
  QuotaToken(@types.QuotaTokenSpec)
  // WASI P3 stubs (parseable only; no semantics yet)
  Future(SchemaType?)
  Stream(SchemaType?)
} derive(Show, Eq)

///|
pub(all) struct NamedFieldType {
  name : String
  body : SchemaType
  metadata : @types.MetadataEnvelope
} derive(Show, Eq)

///|
pub(all) struct VariantCaseType {
  name : String
  payload : SchemaType?
  metadata : @types.MetadataEnvelope
} derive(Show, Eq)

///|
pub(all) struct UnionBranch {
  /// Logical branch name (carried in `union` values).
  tag : String
  body : SchemaType
  discriminator : @types.DiscriminatorRule
  metadata : @types.MetadataEnvelope
} derive(Show, Eq)

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
} derive(Show, Eq)

///|
/// A self-contained schema graph: an ordered registry of named definitions
/// plus a root type. `Ref(id)` bodies reference the `defs` entry with that
/// `id`. Definition order is the first-seen / commit order produced by the
/// recursive walk (it mirrors the Rust `golem-schema` `Vec<SchemaTypeDef>`);
/// it is deterministic and is NOT sorted.
pub(all) struct SchemaGraph {
  defs : Array[SchemaTypeDef]
  root : SchemaType
} derive(Show, Eq)

// ============================================================
// Schema value (recursive)
// ============================================================

///|
/// A recursive schema value. Always conceptually paired with a `SchemaGraph`
/// (see `TypedSchemaValue`).
pub(all) enum SchemaValue {
  // Primitives
  Bool(Bool)
  S8(Int)
  S16(Int)
  S32(Int)
  S64(Int64)
  U8(Byte)
  U16(UInt)
  U32(UInt)
  U64(UInt64)
  F32(Float)
  F64(Double)
  Char(Char)
  String(String)
  // Structural composites
  Record(Array[SchemaValue])
  // case index, optional payload
  Variant(UInt, SchemaValue?)
  Enum(UInt)
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
  Datetime(@types.Datetime)
  // total nanoseconds (signed)
  Duration(Int64)
  Quantity(@types.QuantityValue)
  // discriminator literal tag, body
  Union(String, SchemaValue)
  // opaque secret reference
  Secret(String)
  QuotaToken(@types.QuotaTokenValuePayload)
} derive(Show, Eq)

///|
pub(all) struct SchemaMapEntry {
  key : SchemaValue
  value : SchemaValue
} derive(Show, Eq)

// ============================================================
// Wire carrier (recursive)
// ============================================================

///|
/// A typed value: a self-contained schema graph paired with a value built
/// against that schema.
pub(all) struct TypedSchemaValue {
  graph : SchemaGraph
  value : SchemaValue
} derive(Show, Eq)

// ============================================================
// Constructors / helpers
// ============================================================

///|
/// An empty metadata envelope (no docs / aliases / examples / deprecation / role).
pub fn empty_metadata() -> @types.MetadataEnvelope {
  { doc: None, aliases: [], examples: [], deprecated: None, role: None }
}

///|
/// Wrap a body into a `SchemaType` node, defaulting to empty metadata.
pub fn schema_type(
  body : SchemaTypeBody,
  metadata? : @types.MetadataEnvelope = empty_metadata(),
) -> SchemaType {
  { body, metadata }
}

///|
/// A record field with the given name and (empty by default) metadata.
pub fn field(
  name : String,
  body : SchemaType,
  metadata? : @types.MetadataEnvelope = empty_metadata(),
) -> NamedFieldType {
  { name, body, metadata }
}

///|
/// A variant case with an optional payload and (empty by default) metadata.
pub fn variant_case(
  name : String,
  payload? : SchemaType,
  metadata? : @types.MetadataEnvelope = empty_metadata(),
) -> VariantCaseType {
  { name, payload, metadata }
}
