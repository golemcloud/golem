///|
pub impl @schema.HasElementSchema for UnstructuredText with element_schema() {
  @common.ElementSchema::UnstructuredText(@common.TextDescriptor::{
    restrictions: None,
  })
}

///|
pub impl @schema.FromExtractor for UnstructuredText with from_extractor(_e) {
  raise @common.AgentError::InvalidInput(
    "UnstructuredText does not use the Extractor path",
  )
}

///|
pub impl @schema.FromElementValue for UnstructuredText with from_element_value(
  v,
) {
  match v {
    @common.ElementValue::UnstructuredText(text_ref) =>
      match text_ref {
        @common.TextReference::Url(url) => Url(url)
        @common.TextReference::Inline(source) =>
          Inline(
            text=source.data,
            language_code=source.text_type.map(fn(t) { t.language_code }),
          )
      }
    _ =>
      raise @common.AgentError::InvalidInput(
        "Expected UnstructuredText ElementValue",
      )
  }
}

///|
pub impl @schema.ToElementValue for UnstructuredText with to_element_value(self) {
  match self {
    Url(url) =>
      @common.ElementValue::UnstructuredText(@common.TextReference::Url(url))
    Inline(text~, language_code~) =>
      @common.ElementValue::UnstructuredText(
        @common.TextReference::Inline(@common.TextSource::{
          data: text,
          text_type: language_code.map(fn(code) {
            @common.TextType::{ language_code: code }
          }),
        }),
      )
  }
}

///|
pub impl @schema.HasElementSchema for UnstructuredBinary with element_schema() {
  @common.ElementSchema::UnstructuredBinary(@common.BinaryDescriptor::{
    restrictions: None,
  })
}

///|
pub impl @schema.FromExtractor for UnstructuredBinary with from_extractor(_e) {
  raise @common.AgentError::InvalidInput(
    "UnstructuredBinary does not use the Extractor path",
  )
}

///|
pub impl @schema.FromElementValue for UnstructuredBinary with from_element_value(
  v,
) {
  match v {
    @common.ElementValue::UnstructuredBinary(bin_ref) =>
      match bin_ref {
        @common.BinaryReference::Url(url) => Url(url)
        @common.BinaryReference::Inline(source) =>
          Inline(
            data=Bytes::from_array(source.data[:]),
            mime_type=source.binary_type.mime_type,
          )
      }
    _ =>
      raise @common.AgentError::InvalidInput(
        "Expected UnstructuredBinary ElementValue",
      )
  }
}

///|
pub impl @schema.ToElementValue for UnstructuredBinary with to_element_value(
  self,
) {
  match self {
    Url(url) =>
      @common.ElementValue::UnstructuredBinary(
        @common.BinaryReference::Url(url),
      )
    Inline(data~, mime_type~) =>
      @common.ElementValue::UnstructuredBinary(
        @common.BinaryReference::Inline(@common.BinarySource::{
          data: data.to_fixedarray(),
          binary_type: @common.BinaryType::{ mime_type, },
        }),
      )
  }
}
