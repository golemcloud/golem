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
pub fn[T : FromElementValue] from_element_value_as(
  v : @common.ElementValue,
) -> T raise @common.AgentError {
  T::from_element_value(v)
}
