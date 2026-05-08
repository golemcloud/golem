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
  let element = @types.ElementValue::ComponentModel(wit_value)
  let result : String = from_element_value_as(element)
  inspect(result, content="hello")
}

///|
test "uint64_from_element_value" {
  let wit_value = @builder.Builder::new().u64(42)
  let element = @types.ElementValue::ComponentModel(wit_value)
  let result : UInt64 = from_element_value_as(element)
  inspect(result, content="42")
}

///|
test "option_some_from_element_value" {
  let wit_value = @builder.Builder::new().option_some(builder => builder.s32(99))
  let element = @types.ElementValue::ComponentModel(wit_value)
  let result : Int? = from_element_value_as(element)
  inspect(result, content="Some(99)")
}

///|
test "option_none_from_element_value" {
  let wit_value = @builder.Builder::new().option_none()
  let element = @types.ElementValue::ComponentModel(wit_value)
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
  let element = @types.ElementValue::ComponentModel(wit_value)
  let result : Array[Int] = from_element_value_as(element)
  inspect(result, content="[1, 2, 3]")
}

///|
test "option_array_string_schema_nested" {
  let v : Array[String]? = None
  inspect(
    schema_of(v),
    content=(
      #|ComponentModel({nodes: [{name: None, owner: None, type_: OptionType(1)}, {name: None, owner: None, type_: ListType(2)}, {name: None, owner: None, type_: PrimStringType}]})
    ),
  )
}

///|
test "option_array_string_roundtrip" {
  let val : Array[String]? = Some(["a", "b"])
  let elem = to_element_value_as(val)
  let result : Array[String]? = from_element_value_as(elem)
  inspect(
    result,
    content=(
      #|Some(["a", "b"])
    ),
  )
}

///|
test "bytes_roundtrip" {
  let original : Bytes = b"\x01\x02\x03\x04"
  let element = to_element_value_as(original)
  let restored : Bytes = from_element_value_as(element)
  assert_eq(restored, original)
}

///|
test "bytes_schema" {
  inspect(
    schema_of_tag((@schema.TypeTag::{  } : @schema.TypeTag[Bytes])),
    content=(
      #|ComponentModel({nodes: [{name: None, owner: None, type_: ListType(1)}, {name: None, owner: None, type_: PrimU8Type}]})
    ),
  )
}

///|
test "record_with_option_field_schema" {
  let schema = make_record_schema([
    ("name", schema_of("")),
    ("tags", schema_of(([] : Array[String]))),
    ("note", schema_of((None : String?))),
  ])
  inspect(
    schema,
    content=(
      #|ComponentModel({nodes: [{name: None, owner: None, type_: RecordType([("name", 1), ("tags", 2), ("note", 4)])}, {name: None, owner: None, type_: PrimStringType}, {name: None, owner: None, type_: ListType(3)}, {name: None, owner: None, type_: PrimStringType}, {name: None, owner: None, type_: OptionType(5)}, {name: None, owner: None, type_: PrimStringType}]})
    ),
  )
}
