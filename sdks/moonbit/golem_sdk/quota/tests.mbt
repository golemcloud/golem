// Tests for the schema-carrier quota-token impls.
//
// The runtime `QuotaToken` methods (`new`/`reserve`/`split`/`merge`/`drop` and
// `Reservation::commit`/`drop`) call WASM host imports and therefore cannot run
// in a unit test — calling them would trap the unbound `golem:quota/types` /
// `golem:core/types` resource-drop imports the test runtime cannot provide.
// These tests cover the type-side capability schema (`type_id` + `register_in`,
// both pure) and the value-side `to-value` / `from-value` mapping, which move an
// opaque handle around without touching any host import. The owned handle here
// wraps a fabricated resource index; it is never dropped or sent to a host.

///|
/// Fabricate an opaque quota-token handle for host-free value-mapping tests. The
/// generated `@types.QuotaToken` is a plain index wrapper, so a handle can be
/// built without a host call as long as the resource is never dropped or passed
/// to a host import.
fn fake_token(index : Int) -> QuotaToken {
  {
    handle: @model.GuestQuotaTokenHandle::new(
      @types.QuotaToken::QuotaToken(index),
    ),
  }
}

///|
test "QuotaToken schema is a single capability node with no defs" {
  let graph = @schema.into_schema_graph(
    (@schema.type_tag() : @schema.TypeTag[QuotaToken]),
  )
  inspect(graph.defs.length(), content="0")
  inspect(
    graph.root.body,
    content=(
      #|QuotaToken({resource_name: None})
    ),
  )
}

///|
test "QuotaToken schema survives the WIT round-trip" {
  let graph = @schema.into_schema_graph(
    (@schema.type_tag() : @schema.TypeTag[QuotaToken]),
  )
  let wit = @model.schema_graph_to_wit(graph)
  let back = @model.schema_graph_from_wit(wit)
  assert_eq(back, graph)
}

///|
test "QuotaToken lowers to an opaque quota-token schema value" {
  let token = fake_token(0)
  let sv = @schema.to_value_as(token)
  guard sv is QuotaToken(_) else {
    fail("expected a quota-token schema value, got \{sv}")
  }
}

///|
test "QuotaToken lifting shares the take-once holder" {
  let handle = @model.GuestQuotaTokenHandle::new(
    @types.QuotaToken::QuotaToken(0),
  )
  let sv : @model.SchemaValue = QuotaToken(handle)
  let token : QuotaToken = @schema.from_value_as(sv)
  assert_true(handle.is_present())
  assert_true(token.handle.is_present())
  let dropped = [0]
  let drops : @model.SchemaValueResourceDrops = {
    drop_secret: fn(_) {  },
    drop_quota_token: fn(_) { dropped[0] += 1 },
    drop_stream: fn(_) {  },
    drop_native_stream: fn(_) {  },
  }
  @model.SchemaValue::QuotaToken(token.handle).release_resources(drops)
  sv.release_resources(drops)
  assert_eq(dropped[0], 1)
}

///|
test "QuotaToken compound conversion rolls back through the source tree" {
  let handle = @model.GuestQuotaTokenHandle::new(
    @types.QuotaToken::QuotaToken(1),
  )
  let source = @model.SchemaValue::List([
    @model.SchemaValue::QuotaToken(handle),
    @model.SchemaValue::String("not a quota token"),
  ])
  let result : Result[Array[QuotaToken], @schema.FromSchemaError] = try? @schema.from_value_as(
    source,
  )
  guard result is Err(_) else {
    fail("the second array element must reject the conversion")
  }
  let dropped = [0]
  let drops : @model.SchemaValueResourceDrops = {
    drop_secret: fn(_) {  },
    drop_quota_token: fn(_) { dropped[0] += 1 },
    drop_stream: fn(_) {  },
    drop_native_stream: fn(_) {  },
  }
  source.release_resources(drops)
  assert_false(handle.is_present())
  assert_eq(dropped[0], 1)
}

///|
test "QuotaToken from_value rejects a non quota-token value" {
  let sv : @model.SchemaValue = String("not a token")
  let r : Result[QuotaToken, @schema.FromSchemaError] = try? @schema.from_value_as(
    sv,
  )
  guard r is Err(ShapeMismatch(_)) else {
    fail("expected a ShapeMismatch error")
  }
}
