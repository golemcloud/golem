///|
test "the agent ID comes from GOLEM_AGENT_ID" {
  let agent_id = golem_agent_id_from([
    ("GOLEM_COMPONENT_ID", "component"),
    ("GOLEM_AGENT_ID", "Counter(\"a\")"),
  ])
  assert_eq(agent_id, "Counter(\"a\")")
}

///|
test "a missing GOLEM_AGENT_ID names the card grant the agent needs" {
  let result = try
    golem_agent_id_from([("GOLEM_COMPONENT_ID", "component")]) |> Ok
  catch {
    error => Err(error)
  }
  guard result is Err(SnapshotLoadError(message)) else {
    fail("expected the agent ID lookup to fail")
  }
  assert_eq(
    message, "GOLEM_AGENT_ID is not readable: the agent's permission card must allow env(?agent) @ ?agent : read : GOLEM_AGENT_ID",
  )
}
