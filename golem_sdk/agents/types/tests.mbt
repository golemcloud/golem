///|
test "unstructured_text_url_roundtrip" {
  let original = UnstructuredText::from_url("https://example.com/doc.txt")
  let element = @schema.to_element_value_as(original)
  let restored : UnstructuredText = @schema.from_element_value_as(element)
  inspect(restored, content="Url(\"https://example.com/doc.txt\")")
}

///|
test "unstructured_text_inline_roundtrip" {
  let original = UnstructuredText::from_inline("hello world")
  let element = @schema.to_element_value_as(original)
  let restored : UnstructuredText = @schema.from_element_value_as(element)
  inspect(restored, content="Inline(text=\"hello world\", language_code=None)")
}

///|
test "unstructured_text_inline_with_language_roundtrip" {
  let original = UnstructuredText::from_inline(
    "hello world",
    language_code=Some("en"),
  )
  let element = @schema.to_element_value_as(original)
  let restored : UnstructuredText = @schema.from_element_value_as(element)
  inspect(
    restored,
    content="Inline(text=\"hello world\", language_code=Some(\"en\"))",
  )
}

///|
test "unstructured_text_schema" {
  inspect(
    @schema.schema_of(UnstructuredText::from_url("")),
    content="UnstructuredText({restrictions: None})",
  )
}

///|
test "unstructured_binary_url_roundtrip" {
  let original = UnstructuredBinary::from_url("https://example.com/image.png")
  let element = @schema.to_element_value_as(original)
  let restored : UnstructuredBinary = @schema.from_element_value_as(element)
  inspect(restored, content="Url(\"https://example.com/image.png\")")
}

///|
test "unstructured_binary_inline_roundtrip" {
  let original = UnstructuredBinary::from_inline(
    b"\x89PNG",
    mime_type="image/png",
  )
  let element = @schema.to_element_value_as(original)
  let restored : UnstructuredBinary = @schema.from_element_value_as(element)
  inspect(
    restored,
    content="Inline(data=b\"\\x89PNG\", mime_type=\"image/png\")",
  )
}

///|
test "unstructured_binary_schema" {
  inspect(
    @schema.schema_of(UnstructuredBinary::from_url("")),
    content="UnstructuredBinary({restrictions: None})",
  )
}

///|
test "unstructured_text_schema_with_restrictions" {
  let schema = @schema.schema_of_tag(
    (@schema.TypeTag::{  } : @schema.TypeTag[UnstructuredText]),
    options={ text_languages: ["en", "de"], binary_mime_types: [] },
  )
  inspect(
    schema,
    content="UnstructuredText({restrictions: Some([{language_code: \"en\"}, {language_code: \"de\"}])})",
  )
}

///|
test "unstructured_binary_schema_with_restrictions" {
  let schema = @schema.schema_of_tag(
    (@schema.TypeTag::{  } : @schema.TypeTag[UnstructuredBinary]),
    options={
      text_languages: [],
      binary_mime_types: ["image/png", "image/jpeg"],
    },
  )
  inspect(
    schema,
    content="UnstructuredBinary({restrictions: Some([{mime_type: \"image/png\"}, {mime_type: \"image/jpeg\"}])})",
  )
}

///|
test "schema_options_ignored_for_component_model" {
  let schema = @schema.schema_of_tag(
    (@schema.TypeTag::{  } : @schema.TypeTag[String]),
    options={ text_languages: ["en"], binary_mime_types: [] },
  )
  inspect(
    schema,
    content="ComponentModel({nodes: [{name: None, owner: None, type_: PrimStringType}]})",
  )
}

///|
test "unstructured_text_from_wrong_element_value" {
  let element = @common.ElementValue::ComponentModel({ nodes: [] })
  let result : Result[UnstructuredText, _] = try? @schema.from_element_value_as(
    element,
  )
  inspect(
    result,
    content="Err(InvalidInput(\"Expected UnstructuredText ElementValue\"))",
  )
}

///|
test "unstructured_binary_from_wrong_element_value" {
  let element = @common.ElementValue::ComponentModel({ nodes: [] })
  let result : Result[UnstructuredBinary, _] = try? @schema.from_element_value_as(
    element,
  )
  inspect(
    result,
    content="Err(InvalidInput(\"Expected UnstructuredBinary ElementValue\"))",
  )
}
