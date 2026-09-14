// Round-trip tests for the standard `IntoSchema` / `FromSchema` impls.
//
// Each round-trip exercises the full pipeline a value travels at runtime:
//   value --IntoSchema--> recursive TypedSchemaValue
//         --wit codec---> flat WIT carrier
//         <--wit codec--- recursive TypedSchemaValue
//         <--FromSchema-- value
// and asserts the decoded value (and the schema graph) survive unchanged.

///|
#warnings("-unused_field")
struct StreamThenInt {
  stream : AgentStream[Int]
  value : Int
}

///|
impl FromSchema for StreamThenInt with fn from_value(value) {
  let fields = expect_record(value, "StreamThenInt")
  let stream : AgentStream[Int] = from_value_as(
    record_field(fields, 0, "StreamThenInt"),
  )
  let decoded : Int = from_value_as(record_field(fields, 1, "StreamThenInt"))
  { stream, value: decoded, }
}

///|
/// Full encode -> WIT -> decode round-trip of a value through the schema layer.
fn[T : IntoSchema + FromSchema] roundtrip(v : T) -> T raise {
  let typed = try_into_typed_schema_value(v)
  let wit = @model.typed_schema_value_to_wit(typed)
  let back = @model.typed_schema_value_from_wit(wit, {
    drop_secret: fn(_) {  },
    drop_quota_token: fn(_) {  },
    drop_stream: fn(_) {  },
    drop_native_stream: fn(_) {  },
  })
  // The schema graph must survive the WIT round-trip unchanged.
  assert_eq(back.graph, typed.graph)
  FromSchema::from_value(back.value)
}

///|
test "primitive round-trips" {
  assert_eq(roundtrip(42), 42)
  assert_eq(roundtrip(0xFFFFFFFFU), 0xFFFFFFFFU)
  assert_eq(roundtrip(-9_000_000_000L), -9_000_000_000L)
  assert_eq(roundtrip(18_000_000_000UL), 18_000_000_000UL)
  assert_eq(roundtrip((1.5 : Float)), (1.5 : Float))
  assert_eq(roundtrip(2.5), 2.5)
  assert_eq(roundtrip((0xAB : Byte)), (0xAB : Byte))
  assert_eq(roundtrip('Z'), 'Z')
  assert_eq(roundtrip(true), true)
  assert_eq(roundtrip(false), false)
  assert_eq(roundtrip("hello"), "hello")
}

///|
test "bytes round-trip" {
  assert_eq(roundtrip(b"\x00\x01\xFE\xFF"), b"\x00\x01\xFE\xFF")
  assert_eq(roundtrip(b""), b"")
}

///|
test "option round-trips" {
  assert_eq(roundtrip((Some(7) : Int?)), Some(7))
  assert_eq(roundtrip((None : Int?)), None)
  assert_eq(roundtrip((Some("x") : String?)), Some("x"))
}

///|
test "array round-trips" {
  assert_eq(roundtrip(([1, 2, 3] : Array[Int])), [1, 2, 3])
  assert_eq(roundtrip(([] : Array[Int])), [])
  assert_eq(roundtrip((["a", "b"] : Array[String])), ["a", "b"])
}

///|
test "result round-trips (non-unit)" {
  assert_eq(roundtrip((Ok(1) : Result[Int, String])), Ok(1))
  assert_eq(roundtrip((Err("boom") : Result[Int, String])), Err("boom"))
}

///|
test "result round-trips (unit ok / err omit payload)" {
  assert_eq(roundtrip((Ok(()) : Result[Unit, String])), Ok(()))
  assert_eq(roundtrip((Err("e") : Result[Unit, String])), Err("e"))
  assert_eq(roundtrip((Ok(5) : Result[Int, Unit])), Ok(5))
  assert_eq(roundtrip((Err(()) : Result[Int, Unit])), Err(()))
}

///|
test "map round-trip" {
  let m : Map[String, Int] = { "a": 1, "b": 2 }
  assert_eq(roundtrip(m), m)
}

///|
test "tuple round-trips" {
  assert_eq(roundtrip((1, "two")), (1, "two"))
  assert_eq(roundtrip((1, "two", true)), (1, "two", true))
  assert_eq(roundtrip((1, 2, 3, 4, 5)), (1, 2, 3, 4, 5))
}

///|
test "nested compound round-trips" {
  assert_eq(roundtrip(([Some(1), None, Some(3)] : Array[Int?])), [
    Some(1),
    None,
    Some(3),
  ])
  assert_eq(roundtrip((Some(["x", "y"]) : Array[String]?)), Some(["x", "y"]))
}

///|
test "structural type has no named definitions" {
  // A structural (inline) type produces no named definitions.
  let tag : TypeTag[Int] = type_tag()
  let g = into_schema_graph(tag)
  assert_eq(g.defs.length(), 0)
}

///|
test "shape mismatch surfaces a FromSchemaError" {
  let v : @model.SchemaValue = String("nope")
  let r : Result[Int, FromSchemaError] = try from_value_as(v) |> Ok catch {
    e => Err(e)
  }
  guard r is Err(ShapeMismatch(expected~, actual~, context~)) else {
    fail("expected ShapeMismatch, got \{repr(r)}")
  }
  assert_eq(expected, "s32")
  assert_eq(actual, "string")
  assert_eq(context, "Int")
}

///|
test "AgentStream decoding shares the take-once stream holder" {
  let source = @model.GuestSchemaValueStreamHandle::from_wrapped(
    @types.SchemaValueStream::SchemaValueStream(30),
  )
  let stream : AgentStream[Int] = from_value_as(
    @model.SchemaValue::Stream(source),
  )
  assert_true(source.is_present())
  guard to_value_as(stream) is @model.SchemaValue::Stream(destination) else {
    fail("expected an AgentStream schema value")
  }
  assert_true(destination.is_present())
  let dropped = [0]
  let drops : @model.SchemaValueResourceDrops = {
    drop_secret: fn(_) {  },
    drop_quota_token: fn(_) {  },
    drop_stream: fn(_) { dropped[0] += 1 },
    drop_native_stream: fn(_) {  },
  }
  @model.SchemaValue::Stream(destination).release_resources(drops)
  @model.SchemaValue::Stream(source).release_resources(drops)
  assert_eq(dropped[0], 1)
}

///|
test "compound conversion rollback retains ownership in the source tree" {
  let array_handle = @model.GuestSchemaValueStreamHandle::from_wrapped(
    @types.SchemaValueStream::SchemaValueStream(31),
  )
  let array_source = @model.SchemaValue::List([
    @model.SchemaValue::Stream(array_handle),
    @model.SchemaValue::S32(1),
  ])
  let array_result : Result[Array[AgentStream[Int]], FromSchemaError] = try
    from_value_as(array_source) |> Ok
  catch {
    e => Err(e)
  }
  guard array_result is Err(_) else {
    fail("the second array element must reject the conversion")
  }
  let tuple_handle = @model.GuestSchemaValueStreamHandle::from_wrapped(
    @types.SchemaValueStream::SchemaValueStream(32),
  )
  let tuple_source = @model.SchemaValue::Tuple([
    @model.SchemaValue::Stream(tuple_handle),
    @model.SchemaValue::String("not an int"),
  ])
  let tuple_result : Result[(AgentStream[Int], Int), FromSchemaError] = try
    from_value_as(tuple_source) |> Ok
  catch {
    e => Err(e)
  }
  guard tuple_result is Err(_) else {
    fail("the second tuple element must reject the conversion")
  }
  let record_handle = @model.GuestSchemaValueStreamHandle::from_wrapped(
    @types.SchemaValueStream::SchemaValueStream(33),
  )
  let record_source = @model.SchemaValue::Record([
    @model.SchemaValue::Stream(record_handle),
    @model.SchemaValue::String("not an int"),
  ])
  let record_result : Result[StreamThenInt, FromSchemaError] = try
    from_value_as(record_source) |> Ok
  catch {
    e => Err(e)
  }
  guard record_result is Err(_) else {
    fail("the second record field must reject the conversion")
  }
  let dropped = [0]
  let drops : @model.SchemaValueResourceDrops = {
    drop_secret: fn(_) {  },
    drop_quota_token: fn(_) {  },
    drop_stream: fn(_) { dropped[0] += 1 },
    drop_native_stream: fn(_) {  },
  }
  array_source.release_resources(drops)
  tuple_source.release_resources(drops)
  record_source.release_resources(drops)
  assert_false(array_handle.is_present())
  assert_false(tuple_handle.is_present())
  assert_false(record_handle.is_present())
  assert_eq(dropped[0], 3)
}
