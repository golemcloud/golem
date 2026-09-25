///|
/// Append configuration declarations directly to the agent's wire type pool.
pub(open) trait WireConfigField {
  fn append_wire_entries(@schema.WireTypeEncoder, Array[String]) -> Array[
    @common.AgentConfigDeclaration,
  ]
}

///|
pub fn[T : WireConfigField] append_wire_entries_with(
  encoder : @schema.WireTypeEncoder,
  _tag : @schema.TypeTag[T],
  path : Array[String],
) -> Array[@common.AgentConfigDeclaration] {
  T::append_wire_entries(encoder, path)
}

///|
fn[T : @schema.WireSchema] wire_local_entry(
  encoder : @schema.WireTypeEncoder,
  tag : @schema.TypeTag[T],
  path : Array[String],
) -> Array[@common.AgentConfigDeclaration] {
  [
    {
      source: @common.AgentConfigSource::LOCAL,
      path,
      value_type: @schema.append_wire_type_with(encoder, tag),
    },
  ]
}

///|
pub impl WireConfigField for String with fn append_wire_entries(encoder, path) {
  wire_local_entry(
    encoder,
    (@schema.type_tag() : @schema.TypeTag[String]),
    path,
  )
}

///|
pub impl WireConfigField for Bool with fn append_wire_entries(encoder, path) {
  wire_local_entry(encoder, (@schema.type_tag() : @schema.TypeTag[Bool]), path)
}

///|
pub impl WireConfigField for Int with fn append_wire_entries(encoder, path) {
  wire_local_entry(encoder, (@schema.type_tag() : @schema.TypeTag[Int]), path)
}

///|
pub impl WireConfigField for UInt with fn append_wire_entries(encoder, path) {
  wire_local_entry(encoder, (@schema.type_tag() : @schema.TypeTag[UInt]), path)
}

///|
pub impl WireConfigField for Int64 with fn append_wire_entries(encoder, path) {
  wire_local_entry(encoder, (@schema.type_tag() : @schema.TypeTag[Int64]), path)
}

///|
pub impl WireConfigField for UInt64 with fn append_wire_entries(encoder, path) {
  wire_local_entry(
    encoder,
    (@schema.type_tag() : @schema.TypeTag[UInt64]),
    path,
  )
}

///|
pub impl WireConfigField for Float with fn append_wire_entries(encoder, path) {
  wire_local_entry(encoder, (@schema.type_tag() : @schema.TypeTag[Float]), path)
}

///|
pub impl WireConfigField for Double with fn append_wire_entries(encoder, path) {
  wire_local_entry(
    encoder,
    (@schema.type_tag() : @schema.TypeTag[Double]),
    path,
  )
}

///|
pub impl WireConfigField for Byte with fn append_wire_entries(encoder, path) {
  wire_local_entry(encoder, (@schema.type_tag() : @schema.TypeTag[Byte]), path)
}

///|
pub impl WireConfigField for Char with fn append_wire_entries(encoder, path) {
  wire_local_entry(encoder, (@schema.type_tag() : @schema.TypeTag[Char]), path)
}

///|
pub impl WireConfigField for Bytes with fn append_wire_entries(encoder, path) {
  wire_local_entry(encoder, (@schema.type_tag() : @schema.TypeTag[Bytes]), path)
}

///|
pub impl[T : @schema.WireSchema] WireConfigField for T? with fn append_wire_entries(
  encoder,
  path,
) {
  wire_local_entry(encoder, (@schema.type_tag() : @schema.TypeTag[T?]), path)
}

///|
pub impl[T : @schema.WireSchema] WireConfigField for Array[T] with fn append_wire_entries(
  encoder,
  path,
) {
  wire_local_entry(
    encoder,
    (@schema.type_tag() : @schema.TypeTag[Array[T]]),
    path,
  )
}

///|
pub impl[T : @schema.WireSchema, E : @schema.WireSchema] WireConfigField for Result[
  T,
  E,
] with fn append_wire_entries(encoder, path) {
  wire_local_entry(
    encoder,
    (@schema.type_tag() : @schema.TypeTag[Result[T, E]]),
    path,
  )
}

///|
pub impl[T : @schema.WireSchema] WireConfigField for Secret[T] with fn append_wire_entries(
  encoder,
  path,
) {
  let inner = @schema.append_wire_type_with(
    encoder,
    (@schema.type_tag() : @schema.TypeTag[T]),
  )
  let value_type = encoder.push(@types.SecretType({ inner, category: None, }))
  [{ source: @common.AgentConfigSource::SECRET, path, value_type, }]
}
