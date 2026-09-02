///|
fn test_schema(body : @model.SchemaTypeBody) -> SchemaRef {
  SchemaRef::new({ defs: [], root: @model.schema_type(body), })
}

///|
test "record JSON round-trips through reflected SchemaValue" {
  let metadata = @model.empty_metadata()
  let schema = test_schema(
    Record([
      { name: "message", body: @model.schema_type(String), metadata, },
      { name: "count", body: @model.schema_type(U32(None)), metadata, },
    ]),
  )
  let input = Json::object({
    "message": Json::string("hello"),
    "count": Json::number(3.0),
  })
  assert_eq(schema.unpack_json(schema.pack_json(input)), input)
}

///|
test "flags JSON uses names while SchemaValue uses positions" {
  let schema = test_schema(Flags(["read", "write", "admin"]))
  let input = Json::array([Json::string("write"), Json::string("admin")])
  assert_eq(
    schema.pack_json(input),
    @model.SchemaValue::Flags([false, true, true]),
  )
  assert_eq(schema.unpack_json(schema.pack_json(input)), input)
}

///|
test "record JSON rejects unknown fields" {
  let schema = test_schema(Record([]))
  try schema.pack_json(Json::object({ "extra": Json::number(1.0) })) catch {
    _ => ()
  } noraise {
    _ => fail("expected unknown field to be rejected")
  }
}
