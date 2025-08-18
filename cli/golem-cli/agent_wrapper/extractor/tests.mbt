///|
test "primitive" {
  let wit_value = @builder.Builder::new().s32(42)
  assert_eq(extract(wit_value).s32(), Some(42))
}

///|
test "single_record" {
  let wit_value = @builder.Builder::new().record(builder => {
    builder.u8(1)
    builder.enum_value(2)
    builder.flags([true, false, true])
  })
  assert_eq(extract(wit_value).field(0).unwrap().u8(), Some(1))
  assert_eq(extract(wit_value).field(1).unwrap().enum_value(), Some(2))
  assert_eq(extract(wit_value).field(2).unwrap().flags().unwrap(), [
    true, false, true,
  ])
}

///|
test "deep_record" {
  let wit_value = @builder.Builder::new().record(builder => builder.record(builder => {
    builder.s32(10)
    builder.string("hello")
  }))
  let inner = extract(wit_value).field(0).unwrap()
  assert_eq(inner.field(0).unwrap().s32(), Some(10))
  assert_eq(inner.field(1).unwrap().string(), Some("hello"))
}

///|
test "variant1" {
  let wit_value = @builder.Builder::new().variant(2, builder => builder.s32(42))
  let variant = extract(wit_value).variant().unwrap()
  assert_eq(variant.0, 2)
  assert_eq(variant.1.unwrap().s32(), Some(42))
}

///|
test "variant2" {
  let wit_value = @builder.Builder::new().variant_unit(0)
  let variant = extract(wit_value).variant().unwrap()
  assert_eq(variant.0, 0)
  assert_eq(variant.1.is_empty(), true)
}

///|
test "enum1" {
  let wit_value = @builder.Builder::new().enum_value(2)
  assert_eq(extract(wit_value).enum_value(), Some(2))
}

///|
test "flags" {
  let wit_value = @builder.Builder::new().flags([true, false, true])
  assert_eq(extract(wit_value).flags().unwrap(), [true, false, true])
}

///|
test "tuple" {
  let wit_value = @builder.Builder::new().tuple(builder => {
    builder.s32(42)
    builder.string("hello")
    builder.record(builder => builder.string("world"))
  })
  assert_eq(extract(wit_value).tuple_element(0).unwrap().s32(), Some(42))
  assert_eq(
    extract(wit_value).tuple_element(1).unwrap().string(),
    Some("hello"),
  )
  assert_eq(
    extract(wit_value).tuple_element(2).unwrap().field(0).unwrap().string(),
    Some("world"),
  )
}

///|
test "list" {
  let wit_value = @builder.Builder::new().list(builder => {
    builder.s32(1)
    builder.s32(2)
    builder.s32(3)
    builder.s32(4)
  })
  assert_eq(
    extract(wit_value).list_elements().unwrap().map(e => e.s32().unwrap()),
    [1, 2, 3, 4],
  )
}

///|
test "option1" {
  let wit_value = @builder.Builder::new().option_none()
  assert_eq(extract(wit_value).option().unwrap().is_empty(), true)
}

///|
test "option2" {
  let wit_value = @builder.Builder::new().option_some(builder => builder.s32(42))
  assert_eq(extract(wit_value).option().unwrap().unwrap().s32(), Some(42))
}

///|
test "result1" {
  let wit_value = @builder.Builder::new().result_ok(builder => builder.s32(42))
  assert_eq(
    extract(wit_value).result().unwrap().unwrap().unwrap().s32(),
    Some(42),
  )
}

///|
test "result2" {
  let wit_value = @builder.Builder::new().result_err(builder => builder.s32(42))
  assert_eq(
    extract(wit_value).result().unwrap().unwrap_err().unwrap().s32(),
    Some(42),
  )
}

///|
test "result3" {
  let wit_value = @builder.Builder::new().result_ok_unit()
  assert_eq(extract(wit_value).result().unwrap().unwrap().is_empty(), true)
}

///|
test "result4" {
  let wit_value = @builder.Builder::new().result_err_unit()
  assert_eq(extract(wit_value).result().unwrap().unwrap_err().is_empty(), true)
}

///|
test "handle" {
  let wit_value = @builder.Builder::new().handle(
    @types.Uri::{ value: "wit://test".to_string() },
    42,
  )
  assert_eq(
    extract(wit_value).handle().unwrap(),
    (@types.Uri::{ value: "wit://test".to_string() }, 42),
  )
}
