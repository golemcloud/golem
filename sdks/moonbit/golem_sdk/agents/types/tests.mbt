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
  let schema = @schema.schema_of_tag_with_options(
    (@schema.TypeTag::{  } : @schema.TypeTag[UnstructuredText]),
    { text_languages: ["en", "de"], binary_mime_types: [] },
  )
  inspect(
    schema,
    content="UnstructuredText({restrictions: Some([{language_code: \"en\"}, {language_code: \"de\"}])})",
  )
}

///|
test "unstructured_binary_schema_with_restrictions" {
  let schema = @schema.schema_of_tag_with_options(
    (@schema.TypeTag::{  } : @schema.TypeTag[UnstructuredBinary]),
    { text_languages: [], binary_mime_types: ["image/png", "image/jpeg"] },
  )
  inspect(
    schema,
    content="UnstructuredBinary({restrictions: Some([{mime_type: \"image/png\"}, {mime_type: \"image/jpeg\"}])})",
  )
}

///|
test "schema_options_rejected_for_component_model" {
  let result : Result[_, _] = try? @schema.schema_of_tag_with_options(
    (@schema.TypeTag::{  } : @schema.TypeTag[String]),
    { text_languages: ["en"], binary_mime_types: [] },
  )
  inspect(
    result,
    content=(
      #|Err(InvalidInput("#derive.text_languages requires the parameter type to implement HasElementSchema with UnstructuredText schema"))
    ),
  )
}

///|
test "option_unstructured_text_deserialization_rejected" {
  // Build a valid Option(Some(string)) WitValue, then try to deserialize as Option[UnstructuredText]
  let wit_value = @builder.Builder::new().option_some(builder => {
    builder.string("hello")
  })
  let element = @rpcTypes.ElementValue::ComponentModel(wit_value)
  let result : Result[UnstructuredText?, _] = try? @schema.from_element_value_as(
    element,
  )
  inspect(
    result,
    content="Err(InvalidInput(\"UnstructuredText does not use the Extractor path\"))",
  )
}

///|
test "array_unstructured_binary_deserialization_rejected" {
  // Build a valid List([u8]) WitValue, then try to deserialize as Array[UnstructuredBinary]
  let wit_value = @builder.Builder::new().list(builder => builder.u8(0))
  let element = @rpcTypes.ElementValue::ComponentModel(wit_value)
  let result : Result[Array[UnstructuredBinary], _] = try? @schema.from_element_value_as(
    element,
  )
  inspect(
    result,
    content="Err(InvalidInput(\"UnstructuredBinary does not use the Extractor path\"))",
  )
}

///|
test "option_unstructured_text_serialization_rejected" {
  let val : UnstructuredText? = Some(
    UnstructuredText::from_url("https://x.com"),
  )
  let result : Result[_, _] = try? @schema.to_element_value_as(val)
  inspect(
    result,
    content="Err(InvalidInput(\"Expected ComponentModel for Option inner value\"))",
  )
}

///|
test "array_unstructured_binary_serialization_rejected" {
  let val : Array[UnstructuredBinary] = [
    UnstructuredBinary::from_url("https://x.com"),
  ]
  let result : Result[_, _] = try? @schema.to_element_value_as(val)
  inspect(
    result,
    content="Err(InvalidInput(\"Expected ComponentModel for Array element\"))",
  )
}

///|
test "schema_of_tag_rejects_mime_types_on_string" {
  let result : Result[_, _] = try? @schema.schema_of_tag_with_options(
    (@schema.TypeTag::{  } : @schema.TypeTag[String]),
    { text_languages: [], binary_mime_types: ["image/png"] },
  )
  inspect(
    result,
    content=(
      #|Err(InvalidInput("#derive.mime_types requires the parameter type to implement HasElementSchema with UnstructuredBinary schema"))
    ),
  )
}

///|
test "schema_of_tag_rejects_text_languages_on_unstructured_binary" {
  let result : Result[_, _] = try? @schema.schema_of_tag_with_options(
    (@schema.TypeTag::{  } : @schema.TypeTag[UnstructuredBinary]),
    { text_languages: ["en"], binary_mime_types: [] },
  )
  inspect(
    result,
    content=(
      #|Err(InvalidInput("#derive.text_languages requires the parameter type to implement HasElementSchema with UnstructuredText schema"))
    ),
  )
}

///|
test "schema_of_tag_rejects_mime_types_on_unstructured_text" {
  let result : Result[_, _] = try? @schema.schema_of_tag_with_options(
    (@schema.TypeTag::{  } : @schema.TypeTag[UnstructuredText]),
    { text_languages: [], binary_mime_types: ["image/png"] },
  )
  inspect(
    result,
    content=(
      #|Err(InvalidInput("#derive.mime_types requires the parameter type to implement HasElementSchema with UnstructuredBinary schema"))
    ),
  )
}

///|
test "unstructured_text_from_wrong_element_value" {
  let element = @rpcTypes.ElementValue::ComponentModel({ nodes: [] })
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
  let element = @rpcTypes.ElementValue::ComponentModel({ nodes: [] })
  let result : Result[UnstructuredBinary, _] = try? @schema.from_element_value_as(
    element,
  )
  inspect(
    result,
    content="Err(InvalidInput(\"Expected UnstructuredBinary ElementValue\"))",
  )
}

///|
test "multimodal_text_or_binary_roundtrip" {
  let original : Multimodal[TextOrBinary] = Multimodal::new([
    TextOrBinary::Text(
      UnstructuredText::from_url("https://example.com/doc.txt"),
    ),
    TextOrBinary::Binary(
      UnstructuredBinary::from_inline(b"\x89PNG", mime_type="image/png"),
    ),
  ])
  let data = original.to_data_value()
  let restored : Multimodal[TextOrBinary] = Multimodal::from_data_value(data)
  assert_eq(restored, original)
}

///|
test "multimodal_custom_modality_string_roundtrip" {
  let original : Multimodal[CustomModality[String]] = Multimodal::new([
    CustomModality::Text(UnstructuredText::from_inline("hello")),
    CustomModality::Structured("world"),
    CustomModality::Binary(
      UnstructuredBinary::from_url("https://example.com/bin"),
    ),
  ])
  let data = original.to_data_value()
  let restored : Multimodal[CustomModality[String]] = Multimodal::from_data_value(
    data,
  )
  assert_eq(restored, original)
}

///|
test "multimodal_from_tuple_data_value_raises_error" {
  let data = @rpcTypes.DataValue::Tuple([])
  let result : Result[Multimodal[TextOrBinary], _] = try? Multimodal::from_data_value(
    data,
  )
  inspect(
    result,
    content="Err(InvalidInput(\"Expected Multimodal DataValue\"))",
  )
}
