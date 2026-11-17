///|
test "the agent ID comes from GOLEM_AGENT_ID" {
  let agent_id = get_golem_env_var(
    [
      ("GOLEM_AGENT_TYPE", "Counter"),
      ("GOLEM_AGENT_ID", "0192f0a4-5f1e-8c3b-9d2a-6b7c8d9e0f12"),
    ],
    "GOLEM_AGENT_ID",
  )
  assert_eq(agent_id, "0192f0a4-5f1e-8c3b-9d2a-6b7c8d9e0f12")
}

///|
test "a missing GOLEM_AGENT_TYPE names the card grant the agent needs" {
  let result = try
    get_golem_env_var(
      [("GOLEM_AGENT_ID", "0192f0a4-5f1e-8c3b-9d2a-6b7c8d9e0f12")],
      "GOLEM_AGENT_TYPE",
    )
    |> Ok
  catch {
    error => Err(error)
  }
  guard result is Err(SnapshotLoadError(message)) else {
    fail("expected the agent type lookup to fail")
  }
  assert_eq(
    message, "GOLEM_AGENT_TYPE is not readable: the agent's permission card must allow env(?agent) @ ?agent : read : GOLEM_AGENT_TYPE",
  )
}
