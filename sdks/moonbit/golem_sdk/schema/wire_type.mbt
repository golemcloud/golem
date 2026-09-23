///|
pub(all) struct WireTypeEncoder {
  nodes : Array[@types.SchemaTypeNode]
  defs : Array[@types.SchemaTypeDef]
  ids : Map[String, Int]
}

///|
pub fn WireTypeEncoder::new() -> WireTypeEncoder {
  { nodes: [], defs: [], ids: Map([]), }
}

///|
pub fn empty_wire_metadata() -> @types.MetadataEnvelope {
  { doc: None, aliases: [], examples: [], deprecated: None, role: None, }
}

///|
pub fn WireTypeEncoder::push(
  self : WireTypeEncoder,
  body : @types.SchemaTypeBody,
) -> Int {
  let index = self.nodes.length()
  self.nodes.push({ body, metadata: empty_wire_metadata(), })
  index
}

///|
pub fn WireTypeEncoder::named(
  self : WireTypeEncoder,
  id : String,
  build : () -> Int,
) -> Int {
  match self.ids.get(id) {
    Some(definition) => self.push(@types.SchemaTypeBody::RefType(definition))
    None => {
      let definition = self.defs.length()
      self.ids[id] = definition
      self.defs.push({ id, name: Some(id), body: 0, })
      let body = build()
      self.defs[definition] = { id, name: Some(id), body, }
      self.push(@types.SchemaTypeBody::RefType(definition))
    }
  }
}

///|
pub(open) trait WireSchema {
  fn append_wire_type(WireTypeEncoder) -> Int
  fn append_result_type(WireTypeEncoder) -> Int? = _
}

///|
impl WireSchema with fn append_result_type(encoder) {
  Some(Self::append_wire_type(encoder))
}

///|
pub fn[T : WireSchema] append_wire_type_with(
  encoder : WireTypeEncoder,
  _tag : TypeTag[T],
) -> Int {
  T::append_wire_type(encoder)
}

///|
pub fn[T : WireSchema] wire_schema(_tag : TypeTag[T]) -> @types.SchemaGraph {
  let encoder = WireTypeEncoder::new()
  let root = T::append_wire_type(encoder)
  { type_nodes: encoder.nodes, defs: encoder.defs, root, }
}

///|
pub impl WireSchema for String with fn append_wire_type(encoder) {
  encoder.push(@types.SchemaTypeBody::StringType)
}

///|
pub impl WireSchema for Bool with fn append_wire_type(encoder) {
  encoder.push(@types.SchemaTypeBody::BoolType)
}

///|
pub impl WireSchema for Int with fn append_wire_type(encoder) {
  encoder.push(@types.SchemaTypeBody::S32Type(None))
}

///|
pub impl WireSchema for UInt with fn append_wire_type(encoder) {
  encoder.push(@types.SchemaTypeBody::U32Type(None))
}

///|
pub impl WireSchema for Int64 with fn append_wire_type(encoder) {
  encoder.push(@types.SchemaTypeBody::S64Type(None))
}

///|
pub impl WireSchema for UInt64 with fn append_wire_type(encoder) {
  encoder.push(@types.SchemaTypeBody::U64Type(None))
}

///|
pub impl WireSchema for Float with fn append_wire_type(encoder) {
  encoder.push(@types.SchemaTypeBody::F32Type(None))
}

///|
pub impl WireSchema for Double with fn append_wire_type(encoder) {
  encoder.push(@types.SchemaTypeBody::F64Type(None))
}

///|
pub impl WireSchema for Byte with fn append_wire_type(encoder) {
  encoder.push(@types.SchemaTypeBody::U8Type(None))
}

///|
pub impl WireSchema for Char with fn append_wire_type(encoder) {
  encoder.push(@types.SchemaTypeBody::CharType)
}

///|
pub impl WireSchema for Unit with fn append_wire_type(encoder) {
  encoder.push(@types.SchemaTypeBody::TupleType([]))
}

///|
pub impl WireSchema for Unit with fn append_result_type(_encoder) {
  None
}

///|
pub impl[T : WireSchema] WireSchema for T? with fn append_wire_type(encoder) {
  encoder.push(@types.SchemaTypeBody::OptionType(T::append_wire_type(encoder)))
}

///|
pub impl[T : WireSchema] WireSchema for Array[T] with fn append_wire_type(
  encoder,
) {
  encoder.push(@types.SchemaTypeBody::ListType(T::append_wire_type(encoder)))
}

///|
pub impl[T : WireSchema, E : WireSchema] WireSchema for Result[T, E] with fn append_wire_type(
  encoder,
) {
  encoder.push(
    @types.SchemaTypeBody::ResultType({
      ok: T::append_result_type(encoder),
      err: E::append_result_type(encoder),
    }),
  )
}

///|
pub impl[K : WireSchema, V : WireSchema] WireSchema for Map[K, V] with fn append_wire_type(
  encoder,
) {
  encoder.push(
    @types.SchemaTypeBody::MapType({
      key: K::append_wire_type(encoder),
      value: V::append_wire_type(encoder),
    }),
  )
}

///|
pub impl WireSchema for Bytes with fn append_wire_type(encoder) {
  encoder.push(
    @types.SchemaTypeBody::BinaryType({
      mime_types: None,
      min_bytes: None,
      max_bytes: None,
    }),
  )
}

///|
pub impl WireSchema for @types.Datetime with fn append_wire_type(encoder) {
  encoder.push(@types.SchemaTypeBody::DatetimeType)
}

///|
pub impl WireSchema for Path with fn append_wire_type(encoder) {
  encoder.push(
    @types.SchemaTypeBody::PathType({
      direction: @types.PathDirection::IN_OUT,
      kind: @types.PathKind::ANY,
      allowed_mime_types: None,
      allowed_extensions: None,
    }),
  )
}

///|
pub impl[A : WireSchema, B : WireSchema] WireSchema for (A, B) with fn append_wire_type(
  encoder,
) {
  encoder.push(
    @types.SchemaTypeBody::TupleType([
      A::append_wire_type(encoder),
      B::append_wire_type(encoder),
    ]),
  )
}

///|
pub impl[A : WireSchema, B : WireSchema, C : WireSchema] WireSchema for (
  A,
  B,
  C,
) with fn append_wire_type(encoder) {
  encoder.push(
    @types.SchemaTypeBody::TupleType([
      A::append_wire_type(encoder),
      B::append_wire_type(encoder),
      C::append_wire_type(encoder),
    ]),
  )
}

///|
pub impl[A : WireSchema, B : WireSchema, C : WireSchema, D : WireSchema] WireSchema for (
  A,
  B,
  C,
  D,
) with fn append_wire_type(encoder) {
  encoder.push(
    @types.TupleType([
      A::append_wire_type(encoder),
      B::append_wire_type(encoder),
      C::append_wire_type(encoder),
      D::append_wire_type(encoder),
    ]),
  )
}

///|
pub impl[
  A : WireSchema,
  B : WireSchema,
  C : WireSchema,
  D : WireSchema,
  E : WireSchema,
] WireSchema for (A, B, C, D, E) with fn append_wire_type(encoder) {
  encoder.push(
    @types.TupleType([
      A::append_wire_type(encoder),
      B::append_wire_type(encoder),
      C::append_wire_type(encoder),
      D::append_wire_type(encoder),
      E::append_wire_type(encoder),
    ]),
  )
}

///|
pub impl[
  A : WireSchema,
  B : WireSchema,
  C : WireSchema,
  D : WireSchema,
  E : WireSchema,
  F : WireSchema,
] WireSchema for (A, B, C, D, E, F) with fn append_wire_type(encoder) {
  encoder.push(
    @types.TupleType([
      A::append_wire_type(encoder),
      B::append_wire_type(encoder),
      C::append_wire_type(encoder),
      D::append_wire_type(encoder),
      E::append_wire_type(encoder),
      F::append_wire_type(encoder),
    ]),
  )
}

///|
pub impl[
  A : WireSchema,
  B : WireSchema,
  C : WireSchema,
  D : WireSchema,
  E : WireSchema,
  F : WireSchema,
  G : WireSchema,
] WireSchema for (A, B, C, D, E, F, G) with fn append_wire_type(encoder) {
  encoder.push(
    @types.TupleType([
      A::append_wire_type(encoder),
      B::append_wire_type(encoder),
      C::append_wire_type(encoder),
      D::append_wire_type(encoder),
      E::append_wire_type(encoder),
      F::append_wire_type(encoder),
      G::append_wire_type(encoder),
    ]),
  )
}

///|
pub impl[
  A : WireSchema,
  B : WireSchema,
  C : WireSchema,
  D : WireSchema,
  E : WireSchema,
  F : WireSchema,
  G : WireSchema,
  H : WireSchema,
] WireSchema for (A, B, C, D, E, F, G, H) with fn append_wire_type(encoder) {
  encoder.push(
    @types.TupleType([
      A::append_wire_type(encoder),
      B::append_wire_type(encoder),
      C::append_wire_type(encoder),
      D::append_wire_type(encoder),
      E::append_wire_type(encoder),
      F::append_wire_type(encoder),
      G::append_wire_type(encoder),
      H::append_wire_type(encoder),
    ]),
  )
}
