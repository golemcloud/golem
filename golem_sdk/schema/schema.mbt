///|
pub(open) trait HasElementSchema {
  element_schema() -> @common.ElementSchema
}

///|
pub(open) trait FromExtractor {
  from_extractor(&@extractor.Extractor) -> Self raise @common.AgentError
}

///|
pub(open) trait FromElementValue {
  from_element_value(@common.ElementValue) -> Self raise @common.AgentError
}

///|
pub fn[T : HasElementSchema] schema_of(_v : T) -> @common.ElementSchema {
  T::element_schema()
}

///|
pub(all) struct TypeTag[T] {}

///|
pub fn[T : HasElementSchema] schema_of_tag(
  _ : TypeTag[T],
) -> @common.ElementSchema {
  T::element_schema()
}

///|
pub fn[T : FromElementValue] from_element_value_as(
  v : @common.ElementValue,
) -> T raise @common.AgentError {
  T::from_element_value(v)
}

///|
pub(open) trait ToElementValue {
  to_element_value(Self) -> @common.ElementValue raise @common.AgentError
}

///|
pub fn[T : ToElementValue] to_element_value_as(
  v : T,
) -> @common.ElementValue raise @common.AgentError {
  T::to_element_value(v)
}

///|
pub fn[T : FromExtractor] from_extractor_as(
  e : &@extractor.Extractor,
) -> T raise @common.AgentError {
  T::from_extractor(e)
}
