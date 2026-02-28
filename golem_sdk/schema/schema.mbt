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
pub(all) struct SchemaOptions {
  text_languages : Array[String]
  binary_mime_types : Array[String]
} derive(Show, Eq)

///|
pub fn[T : HasElementSchema] schema_of_tag(
  _ : TypeTag[T],
) -> @common.ElementSchema {
  T::element_schema()
}

///|
pub fn[T : HasElementSchema] schema_of_tag_with_options(
  _ : TypeTag[T],
  options : SchemaOptions,
) -> @common.ElementSchema raise @common.AgentError {
  let base = T::element_schema()
  apply_schema_options(base, options)
}

///|
fn apply_schema_options(
  schema : @common.ElementSchema,
  options : SchemaOptions,
) -> @common.ElementSchema raise @common.AgentError {
  let mut result = schema
  if options.text_languages.length() > 0 {
    match schema {
      @common.ElementSchema::UnstructuredText(desc) =>
        result = @common.ElementSchema::UnstructuredText(@common.TextDescriptor::{
          ..desc,
          restrictions: Some(
            options.text_languages.map(fn(code) {
              @common.TextType::{ language_code: code }
            }),
          ),
        })
      _ =>
        raise @common.AgentError::InvalidInput(
          "#derive.text_languages requires the parameter type to implement HasElementSchema with UnstructuredText schema",
        )
    }
  }
  if options.binary_mime_types.length() > 0 {
    match result {
      @common.ElementSchema::UnstructuredBinary(desc) =>
        result = @common.ElementSchema::UnstructuredBinary(@common.BinaryDescriptor::{
          ..desc,
          restrictions: Some(
            options.binary_mime_types.map(fn(mime) {
              @common.BinaryType::{ mime_type: mime }
            }),
          ),
        })
      _ =>
        raise @common.AgentError::InvalidInput(
          "#derive.mime_types requires the parameter type to implement HasElementSchema with UnstructuredBinary schema",
        )
    }
  }
  result
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
