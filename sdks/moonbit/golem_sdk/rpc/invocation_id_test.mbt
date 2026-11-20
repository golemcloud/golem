///|
test "invocation id text form round trips the agent ID and the invocation UUID" {
  let agent_id = @schema.AgentId::parse("0000002a-0000-8011-8000-00000000001d")
  let uuid = @schema.Uuid::parse("0b9f3d9e-5f43-4c3a-9e7c-0d6f1f2a3b4c")
  let id = @rpc.InvocationId::new(agent_id, uuid)
  let text = id.to_string()
  inspect(
    text,
    content="0000002a-0000-8011-8000-00000000001d.0b9f3d9e-5f43-4c3a-9e7c-0d6f1f2a3b4c",
  )
  inspect(id, content=text)
  let parsed = @rpc.InvocationId::parse(text)
  assert_eq(parsed, id)
  assert_eq(parsed.agent_id(), agent_id)
  assert_eq(parsed.uuid(), uuid)
}

///|
test "invocation id parse rejects a missing dot, a bad uuid and a readable agent id" {
  for
    text in [
      "0000002a-0000-8011-8000-00000000001d-0b9f3d9e-5f43-4c3a-9e7c-0d6f1f2a3b4c",
      "0000002a-0000-8011-8000-00000000001d.0b9f3d9e-5f43-4c3a-9e7c-0d6f1f2a3b4z",
      "0000002a-0000-8011-8000-00000000001z.0b9f3d9e-5f43-4c3a-9e7c-0d6f1f2a3b4c",
      "Counter(\"a\").0b9f3d9e-5f43-4c3a-9e7c-0d6f1f2a3b4c", ".0b9f3d9e-5f43-4c3a-9e7c-0d6f1f2a3b4c",
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
    "0000002A-0000-8011-8000-00000000001D.0B9F3D9E-5F43-4C3A-9E7C-0D6F1F2A3B4C",
  )
  assert_eq(
    parsed.agent_id(),
    @schema.AgentId::parse("0000002a-0000-8011-8000-00000000001d"),
  )
  assert_eq(
    parsed.uuid(),
    @schema.Uuid::parse("0b9f3d9e-5f43-4c3a-9e7c-0d6f1f2a3b4c"),
  )
  inspect(
    parsed,
    content="0000002a-0000-8011-8000-00000000001d.0b9f3d9e-5f43-4c3a-9e7c-0d6f1f2a3b4c",
  )
}
