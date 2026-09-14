// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1.

///|
test "successful handshake requires the actual selected subprotocol header" {
  inspect(
    selected_subprotocol_is_valid(
      { "sec-websocket-protocol": "golem.agent-invocation.v1" },
      "golem.agent-invocation.v1",
    ),
    content="true",
  )
  inspect(
    selected_subprotocol_is_valid(Map([]), "golem.agent-invocation.v1"),
    content="false",
  )
  inspect(
    selected_subprotocol_is_valid(
      { "sec-websocket-protocol": "different.protocol" },
      "golem.agent-invocation.v1",
    ),
    content="false",
  )
}
