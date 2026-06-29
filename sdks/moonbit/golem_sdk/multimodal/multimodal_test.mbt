// Host-free round-trip + schema-shape tests for `Multimodal[T]`. The
// `MultimodalModality` impl below mirrors exactly what `golem_sdk_tools`
// generates for a `#derive.multimodal` enum.

///|
enum TextOrImage {
  Text(String)
  Image(Bytes)
} derive(Eq, Show)

///|
pub impl MultimodalModality for TextOrImage with multimodal_type_id() {
  "test.TextOrImage"
}

///|
pub impl MultimodalModality for TextOrImage with multimodal_cases(b) {
  [
    @model.variant_case(
      "Text",
      payload=@schema.register_in_with(
        b,
        (@schema.TypeTag::{  } : @schema.TypeTag[String]),
      ),
    ),
    @model.variant_case(
      "Image",
      payload=@schema.register_in_with(
        b,
        (@schema.TypeTag::{  } : @schema.TypeTag[Bytes]),
      ),
    ),
  ]
}

///|
pub impl MultimodalModality for TextOrImage with to_modality_value(self) {
  match self {
    Text(v) => (0, @schema.to_value_as(v))
    Image(v) => (1, @schema.to_value_as(v))
  }
}

///|
pub impl MultimodalModality for TextOrImage with from_modality_value(
  case,
  payload,
) {
  match case {
    0 => Text(@schema.from_value_as(payload))
    1 => Image(@schema.from_value_as(payload))
    other => {
      let err = @schema.FromSchemaError::out_of_range(
        other, 2U, "test.TextOrImage",
      )
      raise err
    }
  }
}

///|
test "Multimodal schema is List(Variant) with role=Multimodal" {
  let graph = @schema.into_schema_graph(
    (@schema.TypeTag::{  } : @schema.TypeTag[Multimodal[TextOrImage]]),
  )
  // The root is a List node carrying role=Multimodal.
  assert_eq(graph.root.metadata.role, Some(@types.Role::Multimodal))
  match graph.root.body {
    List(element) =>
      match element.body {
        Variant(cases) => {
          assert_eq(cases.length(), 2)
          assert_eq(cases[0].name, "Text")
          assert_eq(cases[1].name, "Image")
        }
        _ => fail("expected the multimodal list element to be a Variant")
      }
    _ => fail("expected the multimodal root to be a List")
  }
}

///|
test "Multimodal value round-trips through to_value / from_value" {
  let original : Multimodal[TextOrImage] = Multimodal::new([
    Text("hello"),
    Image(b"\x01\x02\x03"),
    Text("world"),
  ])
  let value = @schema.to_value_as(original)
  // The encoded value is a List of Variant nodes.
  match value {
    List(items) => assert_eq(items.length(), 3)
    _ => fail("expected the multimodal value to encode as a List")
  }
  let back : Multimodal[TextOrImage] = @schema.from_value_as(value)
  assert_eq(back.items, original.items)
}
