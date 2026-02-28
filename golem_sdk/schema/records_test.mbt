///|
test "record_schema_primitive_fields" {
  let schema = make_record_schema([
    ("name", schema_of("")),
    ("active", schema_of(true)),
  ])
  inspect(
    schema,
    content=(
      #|ComponentModel({nodes: [{name: None, owner: None, type_: RecordType([("name", 1), ("active", 2)])}, {name: None, owner: None, type_: PrimStringType}, {name: None, owner: None, type_: PrimBoolType}]})
    ),
  )
}

///|
test "record_schema_compound_fields" {
  let opt_int : Int? = None
  let arr_str : Array[String] = []
  let schema = make_record_schema([
    ("opt_field", schema_of(opt_int)),
    ("list_field", schema_of(arr_str)),
  ])
  inspect(
    schema,
    content=(
      #|ComponentModel({nodes: [{name: None, owner: None, type_: RecordType([("opt_field", 1), ("list_field", 3)])}, {name: None, owner: None, type_: OptionType(2)}, {name: None, owner: None, type_: PrimS32Type}, {name: None, owner: None, type_: ListType(4)}, {name: None, owner: None, type_: PrimStringType}]})
    ),
  )
}

///|
test "record_value_roundtrip" {
  let str_val = to_element_value_as("hello")
  let bool_val = to_element_value_as(true)
  let record = make_record_value([str_val, bool_val])
  let e = @extractor.extract_component_model_value(record)
  let name : String = extract_field(e, 0)
  let active : Bool = extract_field(e, 1)
  inspect(name, content="hello")
  inspect(active, content="true")
}

///|
test "enum_schema" {
  let schema = make_enum_schema(["Red", "Green", "Blue"])
  inspect(
    schema,
    content=(
      #|ComponentModel({nodes: [{name: None, owner: None, type_: EnumType(["Red", "Green", "Blue"])}]})
    ),
  )
}

///|
test "enum_value_roundtrip" {
  let val = make_enum_value(1)
  let e = @extractor.extract_component_model_value(val)
  let idx = extract_enum(e)
  inspect(idx, content="1")
}

///|
test "variant_schema_mixed_cases" {
  let schema = make_variant_schema([
    ("None", None),
    ("Some", Some(schema_of(""))),
    ("Error", Some(schema_of(0))),
  ])
  inspect(
    schema,
    content=(
      #|ComponentModel({nodes: [{name: None, owner: None, type_: VariantType([("None", None), ("Some", Some(1)), ("Error", Some(2))])}, {name: None, owner: None, type_: PrimStringType}, {name: None, owner: None, type_: PrimS32Type}]})
    ),
  )
}

///|
test "variant_value_unit_roundtrip" {
  let val = make_variant_value(0, None)
  let e = @extractor.extract_component_model_value(val)
  let (idx, payload) = extract_variant(e)
  inspect(idx, content="0")
  inspect(payload is None, content="true")
}

///|
test "variant_value_payload_roundtrip" {
  let inner = to_element_value_as("hello")
  let val = make_variant_value(1, Some(inner))
  let e = @extractor.extract_component_model_value(val)
  let (idx, payload) = extract_variant(e)
  inspect(idx, content="1")
  guard payload is Some(p) else { fail("Expected payload") }
  guard p.string() is Some(s) else { fail("Expected string") }
  inspect(s, content="hello")
}
