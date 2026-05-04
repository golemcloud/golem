///|
test "unwrap_wit_tuple - primitive value unchanged" {
  let value : @types.WitValue = { nodes: [@types.WitNode::PrimS32(42)] }
  let result = unwrap_wit_tuple(value)
  inspect(result.nodes.length(), content="1")
  inspect(result.nodes[0], content="PrimS32(42)")
}

///|
test "unwrap_wit_tuple - single element tuple" {
  let value : @types.WitValue = {
    nodes: [
      @types.WitNode::TupleValue([1]),
      @types.WitNode::PrimString("hello"),
    ],
  }
  let result = unwrap_wit_tuple(value)
  inspect(result.nodes.length(), content="1")
  inspect(result.nodes[0], content="PrimString(\"hello\")")
}

///|
test "unwrap_wit_tuple - multi element tuple unchanged" {
  let value : @types.WitValue = {
    nodes: [
      @types.WitNode::TupleValue([1, 2]),
      @types.WitNode::PrimS32(1),
      @types.WitNode::PrimString("hi"),
    ],
  }
  let result = unwrap_wit_tuple(value)
  inspect(result.nodes.length(), content="3")
}

///|
test "unwrap_wit_tuple - nested record" {
  let value : @types.WitValue = {
    nodes: [
      @types.WitNode::TupleValue([1]),
      @types.WitNode::RecordValue([2, 3]),
      @types.WitNode::PrimString("name"),
      @types.WitNode::PrimS32(42),
    ],
  }
  let result = unwrap_wit_tuple(value)
  inspect(result.nodes.length(), content="3")
  inspect(result.nodes[0], content="RecordValue([1, 2])")
  inspect(result.nodes[1], content="PrimString(\"name\")")
  inspect(result.nodes[2], content="PrimS32(42)")
}

///|
test "unwrap_wit_tuple - empty value" {
  let value : @types.WitValue = { nodes: [] }
  let result = unwrap_wit_tuple(value)
  inspect(result.nodes.length(), content="0")
}

///|
test "unstructured text roundtrip - url" {
  let text = @agentTypes.UnstructuredText::Url("https://example.com/doc.txt")
  let wv = unstructured_text_to_wit_value(text)
  let decoded = wit_value_to_unstructured_text(wv)
  assert_eq(decoded, text)
}

///|
test "unstructured text roundtrip - inline with language" {
  let text = @agentTypes.UnstructuredText::Inline(
    text="Hello, world!",
    language_code=Some("en"),
  )
  let wv = unstructured_text_to_wit_value(text)
  let decoded = wit_value_to_unstructured_text(wv)
  assert_eq(decoded, text)
}

///|
test "unstructured text roundtrip - inline without language" {
  let text = @agentTypes.UnstructuredText::Inline(
    text="Hello, world!",
    language_code=None,
  )
  let wv = unstructured_text_to_wit_value(text)
  let decoded = wit_value_to_unstructured_text(wv)
  assert_eq(decoded, text)
}

///|
test "unstructured binary roundtrip - url" {
  let binary = @agentTypes.UnstructuredBinary::Url(
    "https://example.com/image.png",
  )
  let wv = unstructured_binary_to_wit_value(binary)
  let decoded = wit_value_to_unstructured_binary(wv)
  assert_eq(decoded, binary)
}

///|
test "unstructured binary roundtrip - inline" {
  let binary = @agentTypes.UnstructuredBinary::Inline(
    data=Bytes::from_array([b'\x89', b'\x50', b'\x4E', b'\x47'][:]),
    mime_type="image/png",
  )
  let wv = unstructured_binary_to_wit_value(binary)
  let decoded = wit_value_to_unstructured_binary(wv)
  assert_eq(decoded, binary)
}

///|
test "unstructured text wit encoding - url structure" {
  let text = @agentTypes.UnstructuredText::Url("https://example.com")
  let wv = unstructured_text_to_wit_value(text)
  // Should be variant(0, string)
  inspect(wv.nodes[0], content="VariantValue((0, Some(1)))")
  inspect(wv.nodes[1], content="PrimString(\"https://example.com\")")
}

///|
test "unstructured text wit encoding - inline structure" {
  let text = @agentTypes.UnstructuredText::Inline(
    text="data",
    language_code=Some("en"),
  )
  let wv = unstructured_text_to_wit_value(text)
  // variant(1, record([string, option(record([string]))]))
  inspect(wv.nodes[0], content="VariantValue((1, Some(1)))")
  inspect(wv.nodes[1], content="RecordValue([2, 3])")
  inspect(wv.nodes[2], content="PrimString(\"data\")")
  inspect(wv.nodes[3], content="OptionValue(Some(4))")
  inspect(wv.nodes[4], content="RecordValue([5])")
  inspect(wv.nodes[5], content="PrimString(\"en\")")
}

///|
test "multimodal roundtrip - TextOrBinary with text and binary" {
  let multimodal = @agentTypes.Multimodal::new([
    @agentTypes.TextOrBinary::Text(
      @agentTypes.UnstructuredText::Inline(
        text="hello world",
        language_code=Some("en"),
      ),
    ),
    @agentTypes.TextOrBinary::Binary(
      @agentTypes.UnstructuredBinary::Url("https://example.com/image.png"),
    ),
  ])
  let wv = multimodal_to_wit_value(multimodal)
  let decoded : @agentTypes.Multimodal[@agentTypes.TextOrBinary] = wit_value_to_multimodal(
    wv,
  )
  assert_eq(decoded.items.length(), 2)
  assert_eq(decoded.items[0], multimodal.items[0])
  assert_eq(decoded.items[1], multimodal.items[1])
}

///|
test "multimodal roundtrip - TextOrBinary with url text" {
  let multimodal = @agentTypes.Multimodal::new([
    @agentTypes.TextOrBinary::Text(
      @agentTypes.UnstructuredText::Url("https://example.com/doc.txt"),
    ),
  ])
  let wv = multimodal_to_wit_value(multimodal)
  let decoded : @agentTypes.Multimodal[@agentTypes.TextOrBinary] = wit_value_to_multimodal(
    wv,
  )
  assert_eq(decoded.items.length(), 1)
  assert_eq(decoded.items[0], multimodal.items[0])
}

///|
test "multimodal roundtrip - TextOrBinary with inline binary" {
  let multimodal = @agentTypes.Multimodal::new([
    @agentTypes.TextOrBinary::Binary(
      @agentTypes.UnstructuredBinary::Inline(
        data=Bytes::from_array([b'\x89', b'\x50', b'\x4E', b'\x47'][:]),
        mime_type="image/png",
      ),
    ),
  ])
  let wv = multimodal_to_wit_value(multimodal)
  let decoded : @agentTypes.Multimodal[@agentTypes.TextOrBinary] = wit_value_to_multimodal(
    wv,
  )
  assert_eq(decoded.items.length(), 1)
  assert_eq(decoded.items[0], multimodal.items[0])
}

///|
test "multimodal roundtrip - empty" {
  let multimodal : @agentTypes.Multimodal[@agentTypes.TextOrBinary] = @agentTypes.Multimodal::new([],
  )
  let wv = multimodal_to_wit_value(multimodal)
  let decoded : @agentTypes.Multimodal[@agentTypes.TextOrBinary] = wit_value_to_multimodal(
    wv,
  )
  assert_eq(decoded.items.length(), 0)
}

///|
test "multimodal wit encoding - structure" {
  let multimodal = @agentTypes.Multimodal::new([
    @agentTypes.TextOrBinary::Text(
      @agentTypes.UnstructuredText::Url("https://example.com"),
    ),
  ])
  let wv = multimodal_to_wit_value(multimodal)
  // Should be list([variant(0, text-reference)])
  inspect(wv.nodes[0], content="ListValue([1])")
  inspect(wv.nodes[1], content="VariantValue((0, Some(2)))")
  // The text-reference payload: variant(0, string)
  inspect(wv.nodes[2], content="VariantValue((0, Some(3)))")
  inspect(wv.nodes[3], content="PrimString(\"https://example.com\")")
}

///|
test "multimodal roundtrip - multiple items mixed" {
  let multimodal = @agentTypes.Multimodal::new([
    @agentTypes.TextOrBinary::Text(
      @agentTypes.UnstructuredText::Inline(text="first", language_code=None),
    ),
    @agentTypes.TextOrBinary::Binary(
      @agentTypes.UnstructuredBinary::Inline(
        data=Bytes::from_array([b'\x01', b'\x02'][:]),
        mime_type="application/octet-stream",
      ),
    ),
    @agentTypes.TextOrBinary::Text(
      @agentTypes.UnstructuredText::Url("https://example.com/text"),
    ),
  ])
  let wv = multimodal_to_wit_value(multimodal)
  let decoded : @agentTypes.Multimodal[@agentTypes.TextOrBinary] = wit_value_to_multimodal(
    wv,
  )
  assert_eq(decoded.items.length(), 3)
  assert_eq(decoded.items[0], multimodal.items[0])
  assert_eq(decoded.items[1], multimodal.items[1])
  assert_eq(decoded.items[2], multimodal.items[2])
}
