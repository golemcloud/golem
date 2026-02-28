///|
fn make_prim_schema(type_node : @types.WitTypeNode) -> @common.ElementSchema {
  @common.ElementSchema::ComponentModel({
    nodes: [{ name: None, owner: None, type_: type_node }],
  })
}

///|
pub impl HasElementSchema for String with element_schema() {
  make_prim_schema(@types.WitTypeNode::PrimStringType)
}

///|
pub impl FromExtractor for String with from_extractor(e) {
  guard e.string() is Some(s) else {
    raise @common.AgentError::InvalidInput("Expected String")
  }
  s
}

///|
pub impl FromElementValue for String with from_element_value(v) {
  FromExtractor::from_extractor(@extractor.extract_component_model_value(v))
}

///|
pub impl ToElementValue for String with to_element_value(self) {
  @common.ElementValue::ComponentModel(@builder.Builder::new().string(self))
}

///|
pub impl HasElementSchema for Bool with element_schema() {
  make_prim_schema(@types.WitTypeNode::PrimBoolType)
}

///|
pub impl FromExtractor for Bool with from_extractor(e) {
  guard e.bool() is Some(b) else {
    raise @common.AgentError::InvalidInput("Expected Bool")
  }
  b
}

///|
pub impl FromElementValue for Bool with from_element_value(v) {
  FromExtractor::from_extractor(@extractor.extract_component_model_value(v))
}

///|
pub impl ToElementValue for Bool with to_element_value(self) {
  @common.ElementValue::ComponentModel(@builder.Builder::new().bool(self))
}

///|
pub impl HasElementSchema for Int with element_schema() {
  make_prim_schema(@types.WitTypeNode::PrimS32Type)
}

///|
pub impl FromExtractor for Int with from_extractor(e) {
  guard e.s32() is Some(i) else {
    raise @common.AgentError::InvalidInput("Expected Int (S32)")
  }
  i
}

///|
pub impl FromElementValue for Int with from_element_value(v) {
  FromExtractor::from_extractor(@extractor.extract_component_model_value(v))
}

///|
pub impl ToElementValue for Int with to_element_value(self) {
  @common.ElementValue::ComponentModel(@builder.Builder::new().s32(self))
}

///|
pub impl HasElementSchema for UInt with element_schema() {
  make_prim_schema(@types.WitTypeNode::PrimU32Type)
}

///|
pub impl FromExtractor for UInt with from_extractor(e) {
  guard e.u32() is Some(u) else {
    raise @common.AgentError::InvalidInput("Expected UInt (U32)")
  }
  u
}

///|
pub impl FromElementValue for UInt with from_element_value(v) {
  FromExtractor::from_extractor(@extractor.extract_component_model_value(v))
}

///|
pub impl ToElementValue for UInt with to_element_value(self) {
  @common.ElementValue::ComponentModel(@builder.Builder::new().u32(self))
}

///|
pub impl HasElementSchema for Int64 with element_schema() {
  make_prim_schema(@types.WitTypeNode::PrimS64Type)
}

///|
pub impl FromExtractor for Int64 with from_extractor(e) {
  guard e.s64() is Some(i) else {
    raise @common.AgentError::InvalidInput("Expected Int64 (S64)")
  }
  i
}

///|
pub impl FromElementValue for Int64 with from_element_value(v) {
  FromExtractor::from_extractor(@extractor.extract_component_model_value(v))
}

///|
pub impl ToElementValue for Int64 with to_element_value(self) {
  @common.ElementValue::ComponentModel(@builder.Builder::new().s64(self))
}

///|
pub impl HasElementSchema for UInt64 with element_schema() {
  make_prim_schema(@types.WitTypeNode::PrimU64Type)
}

///|
pub impl FromExtractor for UInt64 with from_extractor(e) {
  guard e.u64() is Some(u) else {
    raise @common.AgentError::InvalidInput("Expected UInt64 (U64)")
  }
  u
}

///|
pub impl FromElementValue for UInt64 with from_element_value(v) {
  FromExtractor::from_extractor(@extractor.extract_component_model_value(v))
}

///|
pub impl ToElementValue for UInt64 with to_element_value(self) {
  @common.ElementValue::ComponentModel(@builder.Builder::new().u64(self))
}

///|
pub impl HasElementSchema for Float with element_schema() {
  make_prim_schema(@types.WitTypeNode::PrimF32Type)
}

///|
pub impl FromExtractor for Float with from_extractor(e) {
  guard e.f32() is Some(f) else {
    raise @common.AgentError::InvalidInput("Expected Float (F32)")
  }
  f
}

///|
pub impl FromElementValue for Float with from_element_value(v) {
  FromExtractor::from_extractor(@extractor.extract_component_model_value(v))
}

///|
pub impl ToElementValue for Float with to_element_value(self) {
  @common.ElementValue::ComponentModel(@builder.Builder::new().f32(self))
}

///|
pub impl HasElementSchema for Double with element_schema() {
  make_prim_schema(@types.WitTypeNode::PrimF64Type)
}

///|
pub impl FromExtractor for Double with from_extractor(e) {
  guard e.f64() is Some(d) else {
    raise @common.AgentError::InvalidInput("Expected Double (F64)")
  }
  d
}

///|
pub impl FromElementValue for Double with from_element_value(v) {
  FromExtractor::from_extractor(@extractor.extract_component_model_value(v))
}

///|
pub impl ToElementValue for Double with to_element_value(self) {
  @common.ElementValue::ComponentModel(@builder.Builder::new().f64(self))
}

///|
pub impl HasElementSchema for Byte with element_schema() {
  make_prim_schema(@types.WitTypeNode::PrimU8Type)
}

///|
pub impl FromExtractor for Byte with from_extractor(e) {
  guard e.u8() is Some(b) else {
    raise @common.AgentError::InvalidInput("Expected Byte (U8)")
  }
  b
}

///|
pub impl FromElementValue for Byte with from_element_value(v) {
  FromExtractor::from_extractor(@extractor.extract_component_model_value(v))
}

///|
pub impl ToElementValue for Byte with to_element_value(self) {
  @common.ElementValue::ComponentModel(@builder.Builder::new().u8(self))
}

///|
pub impl HasElementSchema for Char with element_schema() {
  make_prim_schema(@types.WitTypeNode::PrimCharType)
}

///|
pub impl FromExtractor for Char with from_extractor(e) {
  guard e.char() is Some(c) else {
    raise @common.AgentError::InvalidInput("Expected Char")
  }
  c
}

///|
pub impl FromElementValue for Char with from_element_value(v) {
  FromExtractor::from_extractor(@extractor.extract_component_model_value(v))
}

///|
pub impl ToElementValue for Char with to_element_value(self) {
  @common.ElementValue::ComponentModel(@builder.Builder::new().char(self))
}
