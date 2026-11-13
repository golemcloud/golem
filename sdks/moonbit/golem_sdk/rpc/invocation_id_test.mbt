///|
test "invocation id text form round trips an agent id with dots, slashes and quotes" {
  let agent_id = "shop.v2/cart(\"a.b\", \"x/y\")"
  let uuid = @schema.Uuid::parse("0b9f3d9e-5f43-4c3a-9e7c-0d6f1f2a3b4c")
  let id = @rpc.InvocationId::new(agent_id, uuid)
  let text = id.to_string()
  inspect(
    text,
    content=(
      #|shop.v2/cart("a.b", "x/y").0b9f3d9e-5f43-4c3a-9e7c-0d6f1f2a3b4c
    ),
  )
  inspect(id, content=text)
  let parsed = @rpc.InvocationId::parse(text)
  assert_eq(parsed, id)
  assert_eq(parsed.agent_id(), agent_id)
  assert_eq(parsed.uuid(), uuid)
}

///|
test "invocation id parse rejects a missing dot, a bad uuid and an empty agent id" {
  for
    text in [
      "agent-0b9f3d9e-5f43-4c3a-9e7c-0d6f1f2a3b4c", "agent.0b9f3d9e-5f43-4c3a-9e7c-0d6f1f2a3b4z",
      "agent.0b9f3d9e+5f43-4c3a-9e7c-0d6f1f2a3b4c", ".0b9f3d9e-5f43-4c3a-9e7c-0d6f1f2a3b4c",
      "0b9f3d9e-5f43-4c3a-9e7c-0d6f1f2a3b4c", "agent", "",
    ] {
    try @rpc.InvocationId::parse(text) catch {
      @rpc.InvocationIdParseError(message) =>
        assert_true(message.contains("expected <agent-id>.<uuid>"))
    } noraise {
      _ => fail("expected '\{text}' to be rejected")
    }
  }
}

///|
test "invocation id parse accepts upper-case uuid hex and prints it lower-case" {
  let parsed = @rpc.InvocationId::parse(
    "shop.v2/Cart(\"A.B\").0B9F3D9E-5F43-4C3A-9E7C-0D6F1F2A3B4C",
  )
  assert_eq(parsed.agent_id(), "shop.v2/Cart(\"A.B\")")
  assert_eq(
    parsed.uuid(),
    @schema.Uuid::parse("0b9f3d9e-5f43-4c3a-9e7c-0d6f1f2a3b4c"),
  )
  inspect(
    parsed,
    content=(
      #|shop.v2/Cart("A.B").0b9f3d9e-5f43-4c3a-9e7c-0d6f1f2a3b4c
    ),
  )
}

///|
test "invocation id parse rejects a non-hyphenated uuid" {
  for
    text in [
      "agent.0b9f3d9e5f434c3a9e7c0d6f1f2a3b4c", "agent.0b9f3d9e5f434c3a9e7c0d6f1f2a3b4c0000",
    ] {
    try @rpc.InvocationId::parse(text) catch {
      @rpc.InvocationIdParseError(message) =>
        assert_true(message.contains("expected <agent-id>.<uuid>"))
    } noraise {
      _ => fail("expected '\{text}' to be rejected")
    }
  }
}
