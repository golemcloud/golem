///|
/// Extracts the element array from a `DataValue::Tuple`. Panics if the value is not a tuple.
pub fn extract_tuple(value : @types.DataValue) -> Array[@types.ElementValue] {
  match value {
    @types.DataValue::Tuple(elements) => elements
    _ => panic()
  }
}

///|
/// Extracts the named element array from a `DataValue::Multimodal`. Panics if the value is not multimodal.
pub fn extract_multimodal(
  value : @types.DataValue,
) -> Array[(String, @types.ElementValue)] {
  match value {
    @types.DataValue::Multimodal(elements) => elements
    _ => panic()
  }
}

///|
/// Asserts that the array has exactly one element and returns it. Panics otherwise.
pub fn expect_single_element(
  values : Array[@types.ElementValue],
) -> @types.ElementValue {
  if values.length() != 1 {
    panic()
  }
  values[0]
}

///|
/// Unwraps a `ComponentModel` element value and creates an `Extractor` for it.
pub fn extract_component_model_value(
  element : @types.ElementValue,
) -> &Extractor raise @common.AgentError {
  guard element is @types.ElementValue::ComponentModel(model) else {
    raise @common.AgentError::InvalidInput(
      "Expected ComponentModel ElementValue",
    )
  }
  extract(model)
}

///|
/// Extracts a `TextReference` from an `UnstructuredText` element value. Panics on type mismatch.
pub fn extract_unstructured_text(
  element : @types.ElementValue,
) -> @types.TextReference {
  match element {
    @types.ElementValue::UnstructuredText(text) => text
    _ => panic()
  }
}

///|
/// Extracts a `BinaryReference` from an `UnstructuredBinary` element value. Panics on type mismatch.
pub fn extract_unstructured_binary(
  element : @types.ElementValue,
) -> @types.BinaryReference {
  match element {
    @types.ElementValue::UnstructuredBinary(data) => data
    _ => panic()
  }
}

///|
/// Creates an `Extractor` for reading values from a `WitValue` tree.
pub fn extract(value : @types.WitValue) -> &Extractor {
  WitValueExtractor::{ value, }
}

///|
/// Implements `Extractor` for a complete `WitValue`, delegating to its root node.
priv struct WitValueExtractor {
  value : @types.WitValue
}

///|
impl Extractor for WitValueExtractor with u8(self : WitValueExtractor) -> Byte? {
  self.value.nodes.get(0).bind(node => Extractor::u8(self.extract(node)))
}

///|
impl Extractor for WitValueExtractor with u16(self : WitValueExtractor) -> UInt? {
  self.value.nodes.get(0).bind(node => Extractor::u16(self.extract(node)))
}

///|
impl Extractor for WitValueExtractor with u32(self : WitValueExtractor) -> UInt? {
  self.value.nodes.get(0).bind(node => Extractor::u32(self.extract(node)))
}

///|
impl Extractor for WitValueExtractor with u64(self : WitValueExtractor) -> UInt64? {
  self.value.nodes.get(0).bind(node => Extractor::u64(self.extract(node)))
}

///|
impl Extractor for WitValueExtractor with s8(self : WitValueExtractor) -> Int? {
  self.value.nodes.get(0).bind(node => Extractor::s8(self.extract(node)))
}

///|
impl Extractor for WitValueExtractor with s16(self : WitValueExtractor) -> Int? {
  self.value.nodes.get(0).bind(node => Extractor::s16(self.extract(node)))
}

///|
impl Extractor for WitValueExtractor with s32(self : WitValueExtractor) -> Int? {
  self.value.nodes.get(0).bind(node => Extractor::s32(self.extract(node)))
}

///|
impl Extractor for WitValueExtractor with s64(self : WitValueExtractor) -> Int64? {
  self.value.nodes.get(0).bind(node => Extractor::s64(self.extract(node)))
}

///|
impl Extractor for WitValueExtractor with f32(self : WitValueExtractor) -> Float? {
  self.value.nodes.get(0).bind(node => Extractor::f32(self.extract(node)))
}

///|
impl Extractor for WitValueExtractor with f64(self : WitValueExtractor) -> Double? {
  self.value.nodes.get(0).bind(node => Extractor::f64(self.extract(node)))
}

///|
impl Extractor for WitValueExtractor with bool(self : WitValueExtractor) -> Bool? {
  self.value.nodes.get(0).bind(node => Extractor::bool(self.extract(node)))
}

///|
impl Extractor for WitValueExtractor with char(self : WitValueExtractor) -> Char? {
  self.value.nodes.get(0).bind(node => Extractor::char(self.extract(node)))
}

///|
impl Extractor for WitValueExtractor with string(self : WitValueExtractor) -> String? {
  self.value.nodes.get(0).bind(node => Extractor::string(self.extract(node)))
}

///|
impl Extractor for WitValueExtractor with field(
  self : WitValueExtractor,
  field_idx : Int,
) -> &Extractor? {
  self.value.nodes
  .get(0)
  .bind(node => Extractor::field(self.extract(node), field_idx))
}

///|
impl Extractor for WitValueExtractor with variant(self : WitValueExtractor) -> (
  UInt,
  &Extractor?,
)? {
  self.value.nodes.get(0).bind(node => Extractor::variant(self.extract(node)))
}

///|
impl Extractor for WitValueExtractor with enum_value(self : WitValueExtractor) -> UInt? {
  self.value.nodes
  .get(0)
  .bind(node => Extractor::enum_value(self.extract(node)))
}

///|
impl Extractor for WitValueExtractor with flags(self : WitValueExtractor) -> Array[
  Bool,
]? {
  self.value.nodes.get(0).bind(node => Extractor::flags(self.extract(node)))
}

///|
impl Extractor for WitValueExtractor with tuple_element(
  self : WitValueExtractor,
  element_idx : Int,
) -> &Extractor? {
  self.value.nodes
  .get(0)
  .bind(node => Extractor::tuple_element(self.extract(node), element_idx))
}

///|
impl Extractor for WitValueExtractor with list_elements(
  self : WitValueExtractor,
) -> Array[&Extractor]? {
  self.value.nodes
  .get(0)
  .bind(node => Extractor::list_elements(self.extract(node)))
}

///|
impl Extractor for WitValueExtractor with option(self : WitValueExtractor) -> &Extractor?? {
  self.value.nodes.get(0).bind(node => Extractor::option(self.extract(node)))
}

///|
impl Extractor for WitValueExtractor with result(self : WitValueExtractor) -> Result[
  &Extractor?,
  &Extractor?,
]? {
  self.value.nodes.get(0).bind(node => Extractor::result(self.extract(node)))
}

///|
impl Extractor for WitValueExtractor with handle(self : WitValueExtractor) -> (
  @types.Uri,
  UInt64,
)? {
  self.value.nodes.get(0).bind(node => Extractor::handle(self.extract(node)))
}

///|
/// Creates a `NodeExtractor` for a specific node within this value's context.
fn WitValueExtractor::extract(
  self : WitValueExtractor,
  node : @types.WitNode,
) -> NodeExtractor {
  { node, context: self.value }
}

///|
/// Implements `Extractor` for a single `WitNode` within a `WitValue` context.
priv struct NodeExtractor {
  node : @types.WitNode
  context : @types.WitValue
}

///|
impl Extractor for NodeExtractor with u8(self : NodeExtractor) -> Byte? {
  match self.node {
    PrimU8(value) => Some(value)
    _ => None
  }
}

///|
impl Extractor for NodeExtractor with u16(self : NodeExtractor) -> UInt? {
  match self.node {
    PrimU16(value) => Some(value)
    _ => None
  }
}

///|
impl Extractor for NodeExtractor with u32(self : NodeExtractor) -> UInt? {
  match self.node {
    PrimU32(value) => Some(value)
    _ => None
  }
}

///|
impl Extractor for NodeExtractor with u64(self : NodeExtractor) -> UInt64? {
  match self.node {
    PrimU64(value) => Some(value)
    _ => None
  }
}

///|
impl Extractor for NodeExtractor with s8(self : NodeExtractor) -> Int? {
  match self.node {
    PrimS8(value) => Some(value)
    _ => None
  }
}

///|
impl Extractor for NodeExtractor with s16(self : NodeExtractor) -> Int? {
  match self.node {
    PrimS16(value) => Some(value)
    _ => None
  }
}

///|
impl Extractor for NodeExtractor with s32(self : NodeExtractor) -> Int? {
  match self.node {
    PrimS32(value) => Some(value)
    _ => None
  }
}

///|
impl Extractor for NodeExtractor with s64(self : NodeExtractor) -> Int64? {
  match self.node {
    PrimS64(value) => Some(value)
    _ => None
  }
}

///|
impl Extractor for NodeExtractor with f32(self : NodeExtractor) -> Float? {
  match self.node {
    PrimFloat32(value) => Some(value)
    _ => None
  }
}

///|
impl Extractor for NodeExtractor with f64(self : NodeExtractor) -> Double? {
  match self.node {
    PrimFloat64(value) => Some(value)
    _ => None
  }
}

///|
impl Extractor for NodeExtractor with bool(self : NodeExtractor) -> Bool? {
  match self.node {
    PrimBool(value) => Some(value)
    _ => None
  }
}

///|
impl Extractor for NodeExtractor with char(self : NodeExtractor) -> Char? {
  match self.node {
    PrimChar(value) => Some(value)
    _ => None
  }
}

///|
impl Extractor for NodeExtractor with string(self : NodeExtractor) -> String? {
  match self.node {
    PrimString(value) => Some(value)
    _ => None
  }
}

///|
impl Extractor for NodeExtractor with field(
  self : NodeExtractor,
  field_idx : Int,
) -> &Extractor? {
  match self.node {
    RecordValue(fields) =>
      fields.get(field_idx).bind(node_idx => self.extract(node_idx))
    _ => None
  }
}

///|
impl Extractor for NodeExtractor with variant(self : NodeExtractor) -> (
  UInt,
  &Extractor?,
)? {
  match self.node {
    VariantValue((case_idx, value)) =>
      Some((case_idx, value.bind(node_idx => self.extract(node_idx))))
    _ => None
  }
}

///|
impl Extractor for NodeExtractor with enum_value(self : NodeExtractor) -> UInt? {
  match self.node {
    EnumValue(value) => Some(value)
    _ => None
  }
}

///|
impl Extractor for NodeExtractor with flags(self : NodeExtractor) -> Array[Bool]? {
  match self.node {
    FlagsValue(flags) => Some(flags)
    _ => None
  }
}

///|
impl Extractor for NodeExtractor with tuple_element(
  self : NodeExtractor,
  element_idx : Int,
) -> &Extractor? {
  match self.node {
    TupleValue(elements) =>
      elements.get(element_idx).bind(node_idx => self.extract(node_idx))
    _ => None
  }
}

///|
impl Extractor for NodeExtractor with list_elements(self : NodeExtractor) -> Array[
  &Extractor,
]? {
  match self.node {
    ListValue(elements) => {
      let result : Array[&Extractor] = []
      let mut failing = false
      for node_idx in elements {
        match self.extract(node_idx) {
          None => {
            failing = true
            break
          }
          Some(extractor) => result.push(extractor)
        }
      }
      if not(failing) {
        Some(result)
      } else {
        None
      }
    }
    _ => None
  }
}

///|
impl Extractor for NodeExtractor with option(self : NodeExtractor) -> &Extractor?? {
  match self.node {
    OptionValue(value) =>
      match value {
        Some(node_idx) =>
          self.extract(node_idx).map(extractor => Some(extractor))
        None => Some(None)
      }
    _ => None
  }
}

///|
impl Extractor for NodeExtractor with result(self : NodeExtractor) -> Result[
  &Extractor?,
  &Extractor?,
]? {
  match self.node {
    ResultValue(Ok(node_idx)) =>
      match node_idx {
        Some(idx) => self.extract(idx).map(extractor => Ok(Some(extractor)))
        None => Some(Ok(None))
      }
    ResultValue(Err(node_idx)) =>
      match node_idx {
        Some(idx) => self.extract(idx).map(extractor => Err(Some(extractor)))
        None => Some(Err(None))
      }
    _ => None
  }
}

///|
impl Extractor for NodeExtractor with handle(self : NodeExtractor) -> (
  @types.Uri,
  UInt64,
)? {
  match self.node {
    Handle((uri, value)) => Some((uri, value))
    _ => None
  }
}

///|
/// Looks up a child node by index and wraps it in a `NodeExtractor`.
fn NodeExtractor::extract(self : NodeExtractor, node_idx : Int) -> &Extractor? {
  self.context.nodes
  .get(node_idx)
  .map(node => NodeExtractor::{ node, context: self.context })
}

///|
/// Trait for reading typed values from a `WitValue` node tree. Provides accessors
/// for all WIT primitive and compound types, returning `None` on type mismatch.
pub(open) trait Extractor {
  u8(Self) -> Byte?
  u16(Self) -> UInt?
  u32(Self) -> UInt?
  u64(Self) -> UInt64?
  s8(Self) -> Int?
  s16(Self) -> Int?
  s32(Self) -> Int?
  s64(Self) -> Int64?
  f32(Self) -> Float?
  f64(Self) -> Double?
  bool(Self) -> Bool?
  char(Self) -> Char?
  string(Self) -> String?
  field(Self, field_idx : Int) -> &Extractor?
  variant(Self) -> (UInt, &Extractor?)?
  enum_value(Self) -> UInt?
  flags(Self) -> Array[Bool]?
  tuple_element(Self, element_idx : Int) -> &Extractor?
  list_elements(Self) -> Array[&Extractor]?
  option(Self) -> &Extractor??
  result(Self) -> Result[&Extractor?, &Extractor?]?
  handle(Self) -> (@types.Uri, UInt64)?
}
