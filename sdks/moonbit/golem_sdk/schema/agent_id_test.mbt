///|
test "agent ID text form is the UUID" {
  let text = "0000002a-0000-8011-8000-00000000001d"
  let id = @schema.AgentId::parse(text)
  inspect(id, content=text)
  assert_eq(id.uuid(), @schema.Uuid::parse(text))
  assert_eq(@schema.AgentId::parse(id.to_string()), id)
  debug_inspect(
    id,
    content=(
      #|AgentId("0000002a-0000-8011-8000-00000000001d")
    ),
  )
}

///|
test "agent ID converts to and from the WIT record" {
  let id = @schema.AgentId::from_uuid(@schema.Uuid::from_bits(42UL, 17UL))
  let wit : @types.AgentId = { uuid: { high_bits: 42UL, low_bits: 17UL, }, }
  assert_eq(id.to_wit(), wit)
  assert_eq(@schema.AgentId::from_wit(wit), id)
}

///|
test "agent ID parse rejects a non-UUID text" {
  try @schema.AgentId::parse("Counter(\"a\")") catch {
    @schema.UuidParseError(message) =>
      inspect(
        message,
        content="invalid UUID 'Counter(\"a\")': expected 36 characters",
      )
  } noraise {
    _ => fail("expected a parse error")
  }
}
