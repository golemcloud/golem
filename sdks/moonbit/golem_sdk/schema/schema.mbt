// Runtime trait surface for converting MoonBit values to and from the recursive
// schema model (`schema_model`). Mirrors the Rust SDK's
// `golem-schema/src/schema/conversion.rs`:
//
//   - `IntoSchema` emits a self-contained `SchemaGraph` for a type and converts
//     a value of that type into a matching `SchemaValue`.
//   - `FromSchema` decodes a `SchemaValue` back into a value.
//
// Hand-written impls for the standard primitive / compound set live in
// `primitives.mbt` and `compounds.mbt`; generated code (Phase 4) emits impls for
// user record / enum / variant types using the helpers in `records.mbt`.
//
// This package depends only on `schema_model` (the recursive model + builder +
// WIT codec); it never touches the flat `@types` carriers directly.

///|
/// Convert a type into a self-contained schema graph and a value of that type
/// into a matching `SchemaValue`.
///
/// `type_id` and `register_in` are static (type-level); `to_value` is an
/// instance method. All three are infallible — well-formedness is validated
/// when the graph is finalized (`into_schema_graph`), not during construction,
/// matching the Rust `IntoSchema`.
pub(open) trait IntoSchema {
  type_id() -> @model.TypeId
  register_in(@model.SchemaBuilder) -> @model.SchemaType
  to_value(Self) -> @model.SchemaValue
}

///|
/// Decode a `SchemaValue` back into a value. The only fallible direction.
pub(open) trait FromSchema {
  from_value(@model.SchemaValue) -> Self raise FromSchemaError
}

// ============================================================
// Errors
// ============================================================

///|
/// Errors produced by `FromSchema::from_value`. Mirrors the Rust
/// `FromSchemaError`.
pub suberror FromSchemaError {
  /// The expected value shape does not match what was provided.
  ShapeMismatch(expected~ : String, actual~ : String, context~ : String)
  /// A variant or enum case index is out of range.
  OutOfRange(index~ : UInt, len~ : UInt, context~ : String)
  /// A union tag was not matched by any branch.
  UnknownUnionTag(String)
  /// Free-form error message produced by generated decoders.
  Custom(String)
} derive(Show, Eq)

///|
/// Construct a `ShapeMismatch` error.
pub fn FromSchemaError::shape_mismatch(
  expected : String,
  actual : String,
  context : String,
) -> FromSchemaError {
  ShapeMismatch(expected~, actual~, context~)
}

///|
/// Construct an `OutOfRange` error.
pub fn FromSchemaError::out_of_range(
  index : UInt,
  len : UInt,
  context : String,
) -> FromSchemaError {
  OutOfRange(index~, len~, context~)
}

///|
/// Construct a `Custom` error.
pub fn FromSchemaError::custom(msg : String) -> FromSchemaError {
  Custom(msg)
}

///|
/// Stringify the kind of a `SchemaValue` for error messages.
pub fn value_kind(v : @model.SchemaValue) -> String {
  match v {
    Bool(_) => "bool"
    S8(_) => "s8"
    S16(_) => "s16"
    S32(_) => "s32"
    S64(_) => "s64"
    U8(_) => "u8"
    U16(_) => "u16"
    U32(_) => "u32"
    U64(_) => "u64"
    F32(_) => "f32"
    F64(_) => "f64"
    Char(_) => "char"
    String(_) => "string"
    Record(_) => "record"
    Variant(_, _) => "variant"
    Enum(_) => "enum"
    Flags(_) => "flags"
    Tuple(_) => "tuple"
    List(_) => "list"
    FixedList(_) => "fixed-list"
    Map(_) => "map"
    Option(_) => "option"
    ResultOk(_) => "result"
    ResultErr(_) => "result"
    Text(_, _) => "text"
    Binary(_, _) => "binary"
    Path(_) => "path"
    Url(_) => "url"
    Datetime(_) => "datetime"
    Duration(_) => "duration"
    Quantity(_) => "quantity"
    Union(_, _) => "union"
    Secret(_) => "secret"
    QuotaToken(_) => "quota-token"
  }
}

// ============================================================
// Type-level helpers
// ============================================================

///|
/// Zero-sized type tag used to resolve `IntoSchema` / `FromSchema` static
/// methods without needing a value instance.
pub(all) struct TypeTag[T] {}

///|
/// Construct a `TypeTag` for `T`.
pub fn[T] type_tag() -> TypeTag[T] {
  TypeTag::{  }
}

///|
/// `TypeId` derivation rule (the contract Phase 4 codegen must follow):
///
/// * **Default**: a type's `type_id()` is its **package-qualified dotted MoonBit
///   type name** (e.g. `mypkg.sub.TaskInfo`). This is the stable, language-
///   independent identity used for `Ref`s, def dedup, and agent-graph merge.
/// * **`named` override**: a `#derive`-level `named("...")` attribute replaces
///   the default base for cross-SDK identity (so a MoonBit type can share a
///   `TypeId` with the Rust/TS type it mirrors). The override only replaces the
///   **base**; type arguments are still appended (see below).
/// * **Generics**: a generic instantiation appends its resolved argument
///   `TypeId`s via `type_id_with_args(base, [arg_type_ids])`, so each
///   instantiation gets its own def (`Boxed<i32>` vs `Boxed<alloc.string.String>`).
///   Argument ids are themselves derived by the same rule (recursively).
/// * **`name` is display-only**: the optional `SchemaTypeDef.name` (passed to
///   `SchemaBuilder::register`) never participates in identity or merge-conflict
///   detection — only the `TypeId` does. Two defs with the same `TypeId` must
///   have structurally equal bodies (metadata included) or merge fails.
///
/// Append resolved type arguments to a base nominal `TypeId`, mirroring the
/// Rust `type_id_with_args`. `Container<u32>` -> `Container<u32>`;
/// `Container<u32, String>` -> `Container<u32, alloc.string.String>`.
pub fn type_id_with_args(
  base : String,
  args : Array[@model.TypeId],
) -> @model.TypeId {
  if args.is_empty() {
    base
  } else {
    let parts = StringBuilder::new()
    parts.write_string(base)
    parts.write_char('<')
    for i, a in args {
      if i > 0 {
        parts.write_string(", ")
      }
      parts.write_string(a)
    }
    parts.write_char('>')
    parts.to_string()
  }
}

///|
/// Build a self-contained `SchemaGraph` for `T`, then validate it for
/// structural well-formedness. The graph is rooted at `T`'s schema and contains
/// every named definition reachable from it.
///
/// Mirrors the Rust `try_into_schema_graph`: `register_in` stays infallible
/// (schema construction never fails); well-formedness is validated here, and
/// the first collected `@model.SchemaError` is raised if the graph is malformed.
pub fn[T : IntoSchema] into_schema_graph(
  _tag : TypeTag[T],
) -> @model.SchemaGraph raise {
  let b = @model.SchemaBuilder::new()
  let root = T::register_in(b)
  let graph = b.build_graph(root)
  let errors = @model.validate_graph(graph)
  match errors.get(0) {
    Some(err) => raise err
    None => graph
  }
}

///|
/// Build a `TypedSchemaValue` — a self-contained `SchemaGraph` paired with the
/// encoded `SchemaValue` — from a value whose type implements `IntoSchema`.
///
/// Mirrors the Rust `try_into_typed_schema_value`.
pub fn[T : IntoSchema] try_into_typed_schema_value(
  value : T,
) -> @model.TypedSchemaValue raise {
  let tag : TypeTag[T] = type_tag()
  let graph = into_schema_graph(tag)
  { graph, value: T::to_value(value) }
}

///|
/// Resolve the schema graph for a type given its `TypeTag`, without a value.
pub fn[T : IntoSchema] schema_graph_of_tag(
  tag : TypeTag[T],
) -> @model.SchemaGraph raise {
  into_schema_graph(tag)
}

///|
/// Register `T`'s schema into `builder`, selecting `T` via a `TypeTag`.
///
/// Generated `register_in` impls use this to register field / element types
/// whose names are generic (e.g. `Array[Foo]`, `Foo?`) and therefore cannot be
/// written as a plain `T::register_in(builder)` static call. It threads the
/// shared builder so recursive references close correctly.
pub fn[T : IntoSchema] register_in_with(
  builder : @model.SchemaBuilder,
  _tag : TypeTag[T],
) -> @model.SchemaType {
  T::register_in(builder)
}

///|
/// Decode a `SchemaValue` into `T` (typed convenience wrapper).
pub fn[T : FromSchema] from_value_as(
  v : @model.SchemaValue,
) -> T raise FromSchemaError {
  T::from_value(v)
}

///|
/// Encode a value into a `SchemaValue` (typed convenience wrapper).
pub fn[T : IntoSchema] to_value_as(v : T) -> @model.SchemaValue {
  T::to_value(v)
}
