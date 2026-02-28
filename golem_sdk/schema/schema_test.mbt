///|
test "string_schema" {
  inspect(
    schema_of(""),
    content="ComponentModel({nodes: [{name: None, owner: None, type_: PrimStringType}]})",
  )
}

///|
test "uint64_schema" {
  inspect(
    schema_of(0UL),
    content="ComponentModel({nodes: [{name: None, owner: None, type_: PrimU64Type}]})",
  )
}

///|
test "option_string_schema" {
  let v : String? = None
  inspect(
    schema_of(v),
    content="ComponentModel({nodes: [{name: None, owner: None, type_: OptionType(1)}, {name: None, owner: None, type_: PrimStringType}]})",
  )
}

///|
test "array_int_schema" {
  let v : Array[Int] = []
  inspect(
    schema_of(v),
    content="ComponentModel({nodes: [{name: None, owner: None, type_: ListType(1)}, {name: None, owner: None, type_: PrimS32Type}]})",
  )
}

///|
test "string_from_element_value" {
  let wit_value = @builder.Builder::new().string("hello")
  let element = @common.ElementValue::ComponentModel(wit_value)
  let result : String = from_element_value_as(element)
  inspect(result, content="hello")
}

///|
test "uint64_from_element_value" {
  let wit_value = @builder.Builder::new().u64(42)
  let element = @common.ElementValue::ComponentModel(wit_value)
  let result : UInt64 = from_element_value_as(element)
  inspect(result, content="42")
}

///|
test "option_some_from_element_value" {
  let wit_value = @builder.Builder::new().option_some(builder => builder.s32(99))
  let element = @common.ElementValue::ComponentModel(wit_value)
  let result : Int? = from_element_value_as(element)
  inspect(result, content="Some(99)")
}

///|
test "option_none_from_element_value" {
  let wit_value = @builder.Builder::new().option_none()
  let element = @common.ElementValue::ComponentModel(wit_value)
  let result : Int? = from_element_value_as(element)
  inspect(result, content="None")
}

///|
test "array_from_element_value" {
  let wit_value = @builder.Builder::new().list(builder => {
    builder.s32(1)
    builder.s32(2)
    builder.s32(3)
  })
  let element = @common.ElementValue::ComponentModel(wit_value)
  let result : Array[Int] = from_element_value_as(element)
  inspect(result, content="[1, 2, 3]")
}
