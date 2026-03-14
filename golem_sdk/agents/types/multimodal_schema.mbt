///|
/// Trait for enum types whose variants represent modalities.
/// Each variant maps to a named ElementSchema/ElementValue pair.
pub(open) trait MultimodalModality {
  /// Returns (name, ElementSchema) for each variant
  multimodal_schema() -> Array[(String, @rpcTypes.ElementSchema)]
  /// Serializes to (name, ElementValue)
  to_named_element(Self) -> (String, @rpcTypes.ElementValue) raise @common.AgentError
  /// Deserializes from (name, ElementValue)
  from_named_element(String, @rpcTypes.ElementValue) -> Self raise @common.AgentError
}

///|
pub impl MultimodalModality for TextOrBinary with multimodal_schema() {
  [
    ("Text", UnstructuredText::element_schema()),
    ("Binary", UnstructuredBinary::element_schema()),
  ]
}

///|
pub impl MultimodalModality for TextOrBinary with to_named_element(self) {
  match self {
    Text(t) => ("Text", @schema.ToElementValue::to_element_value(t))
    Binary(b) => ("Binary", @schema.ToElementValue::to_element_value(b))
  }
}

///|
pub impl MultimodalModality for TextOrBinary with from_named_element(
  name,
  value,
) {
  match name {
    "Text" => Text(@schema.FromElementValue::from_element_value(value))
    "Binary" => Binary(@schema.FromElementValue::from_element_value(value))
    _ => raise @common.AgentError::InvalidInput("Unknown modality: " + name)
  }
}

///|
pub impl[
  T : @schema.HasElementSchema + @schema.ToElementValue + @schema.FromElementValue,
] MultimodalModality for CustomModality[T] with multimodal_schema() {
  [
    ("Text", UnstructuredText::element_schema()),
    ("Binary", UnstructuredBinary::element_schema()),
    ("Structured", T::element_schema()),
  ]
}

///|
pub impl[
  T : @schema.HasElementSchema + @schema.ToElementValue + @schema.FromElementValue,
] MultimodalModality for CustomModality[T] with to_named_element(self) {
  match self {
    Text(t) => ("Text", @schema.ToElementValue::to_element_value(t))
    Binary(b) => ("Binary", @schema.ToElementValue::to_element_value(b))
    Structured(v) => ("Structured", T::to_element_value(v))
  }
}

///|
pub impl[
  T : @schema.HasElementSchema + @schema.ToElementValue + @schema.FromElementValue,
] MultimodalModality for CustomModality[T] with from_named_element(name, value) {
  match name {
    "Text" => Text(@schema.FromElementValue::from_element_value(value))
    "Binary" => Binary(@schema.FromElementValue::from_element_value(value))
    "Structured" => Structured(T::from_element_value(value))
    _ => raise @common.AgentError::InvalidInput("Unknown modality: " + name)
  }
}

///|
/// Deserialize a Multimodal from a DataValue::Multimodal
pub fn[T : MultimodalModality] Multimodal::from_data_value(
  data : @rpcTypes.DataValue,
) -> Multimodal[T] raise @common.AgentError {
  guard data is @rpcTypes.DataValue::Multimodal(elements) else {
    raise @common.AgentError::InvalidInput("Expected Multimodal DataValue")
  }
  let items : Array[T] = []
  for pair in elements {
    let (name, value) = pair
    items.push(T::from_named_element(name, value))
  }
  { items, }
}

///|
/// Helper for codegen: returns the multimodal schema for a modality type via TypeTag dispatch.
pub fn[T : MultimodalModality] multimodal_schema_of_tag(
  _ : @schema.TypeTag[T],
) -> Array[(String, @rpcTypes.ElementSchema)] {
  T::multimodal_schema()
}

///|
/// Serialize a Multimodal to a DataValue::Multimodal
pub fn[T : MultimodalModality] Multimodal::to_data_value(
  self : Multimodal[T],
) -> @rpcTypes.DataValue raise @common.AgentError {
  let elements : Array[(String, @rpcTypes.ElementValue)] = []
  for item in self.items {
    elements.push(T::to_named_element(item))
  }
  @rpcTypes.DataValue::Multimodal(elements)
}
