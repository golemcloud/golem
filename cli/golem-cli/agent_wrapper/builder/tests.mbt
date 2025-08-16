///|
test "primitive" {
  let wit_value = Builder::new().u64(11)
  inspect(wit_value, content="{nodes: [PrimU64(11)]}")
}

///|
test "single_record" {
  let wit_value = Builder::new().record(builder => {
    builder.u8(1)
    builder.enum_value(2)
    builder.flags([true, false, true])
  })
  inspect(
    wit_value,
    content="{nodes: [RecordValue([1, 2, 3]), PrimU8(b'\\x01'), EnumValue(2), FlagsValue([true, false, true])]}",
  )
}

///|
test "deep_record" {
  let wit_value = Builder::new().record(builder => builder.list(builder => {
    builder.record(builder => {
      builder.s32(10)
      builder.s32(-11)
    })
    builder.record(builder => {
      builder.s32(100)
      builder.s32(200)
    })
  }))
  inspect(
    wit_value,
    content="{nodes: [RecordValue([1]), ListValue([2, 5]), RecordValue([3, 4]), PrimS32(10), PrimS32(-11), RecordValue([6, 7]), PrimS32(100), PrimS32(200)]}",
  )
}

///|
test "option" {
  let wit_value = Builder::new().option_some(builder => builder.option_some(builder => builder.option_none()))
  inspect(
    wit_value,
    content="{nodes: [OptionValue(Some(1)), OptionValue(Some(2)), OptionValue(None)]}",
  )
}
