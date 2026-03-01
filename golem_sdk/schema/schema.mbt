///|
/// Returns the `ElementSchema` (type description) for a MoonBit type. Used by the SDK to generate agent type definitions.
pub(open) trait HasElementSchema {
  element_schema() -> @common.ElementSchema
}

///|
/// Deserializes a value from a low-level `Extractor` (WitNode level). Implement this for custom types.
pub(open) trait FromExtractor {
  from_extractor(&@extractor.Extractor) -> Self raise @common.AgentError
}

///|
/// Deserializes a value from an `ElementValue`. Typically delegates to `FromExtractor`.
pub(open) trait FromElementValue {
  from_element_value(@common.ElementValue) -> Self raise @common.AgentError
}

///|
/// Infers and returns the `ElementSchema` for a value via its `HasElementSchema` implementation.
pub fn[T : HasElementSchema] schema_of(_v : T) -> @common.ElementSchema {
  T::element_schema()
}

///|
/// Zero-sized type tag used to resolve `HasElementSchema` without needing a value instance.
pub(all) struct TypeTag[T] {}

///|
/// Options for applying restrictions to schemas (e.g., language codes for text, MIME types for binary).
pub(all) struct SchemaOptions {
  text_languages : Array[String]
  binary_mime_types : Array[String]
} derive(Show, Eq)

///|
/// Returns the `ElementSchema` for a type given its `TypeTag`, without needing a value instance.
pub fn[T : HasElementSchema] schema_of_tag(
  _ : TypeTag[T],
) -> @common.ElementSchema {
  T::element_schema()
}

///|
/// Returns the `ElementSchema` for a type with schema restrictions applied. Raises `AgentError` if the options are incompatible with the schema kind.
pub fn[T : HasElementSchema] schema_of_tag_with_options(
  _ : TypeTag[T],
  options : SchemaOptions,
) -> @common.ElementSchema raise @common.AgentError {
  let base = T::element_schema()
  apply_schema_options(base, options)
}

///|
/// Applies text language and binary MIME type restrictions to an `ElementSchema`. Raises if the schema kind is incompatible with the options.
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
/// Typed convenience function to deserialize a value from an `ElementValue`.
pub fn[T : FromElementValue] from_element_value_as(
  v : @common.ElementValue,
) -> T raise @common.AgentError {
  T::from_element_value(v)
}

///|
/// Serializes a value to an `ElementValue`. Implement this for custom types.
pub(open) trait ToElementValue {
  to_element_value(Self) -> @common.ElementValue raise @common.AgentError
}

///|
/// Typed convenience function to serialize a value to an `ElementValue`.
pub fn[T : ToElementValue] to_element_value_as(
  v : T,
) -> @common.ElementValue raise @common.AgentError {
  T::to_element_value(v)
}

///|
/// Typed convenience function to deserialize a value from an `Extractor`.
pub fn[T : FromExtractor] from_extractor_as(
  e : &@extractor.Extractor,
) -> T raise @common.AgentError {
  T::from_extractor(e)
}
