///|
let resolved_reconnect : ReconnectPolicy = __RECONNECT__

///|
async fn run_recovery() -> Unit {
  let codec = public_value_codec(
    "{\"root\":{\"kind\":\"tuple\",\"value\":{\"elements\":[]}}}",
  )
  let output_codec = public_value_codec(
    "{\"root\":{\"kind\":\"stream\",\"value\":{\"inner\":{\"kind\":\"u8\",\"value\":{}}}}}",
  )
  let resolved : ResolvedAgent = {
    configuration: {
      server: Custom("http://127.0.0.1:__PORT__", "test"),
      app_name: "app",
      env_name: "env",
      reconnect: resolved_reconnect,
    },
    agent_type_name: "RecoveryAgent",
    parameters: TupleValue([]),
    phantom_id: None,
    config: [],
    agent_id: None,
  }
  let result = invoke_streaming_agent(
    StreamEncodeContext::new(),
    codec,
    codec,
    output_codec,
    resolved,
    "run",
    TupleValue([]),
  )
  let stream = open_output_stream(
    result.result.unwrap(),
    value => as_u8(value),
    u8_public_codec(),
  )
  assert_eq(stream.next(), Some(b'\x2a'))
  ignore(@http.get("http://127.0.0.1:__PORT__/consumed"))
  assert_eq(stream.next(), Some(b'\x2b'))
  assert_true(stream.next() is None)
}

///|
async test "streaming recovery rejects or completes" {
  connect_attempt_timeout_ms.val = 200
  let expected = "__ERROR__"
  try run_recovery() catch {
    Unreachable(endpoint~, window_ms~, ..) => {
      assert_eq(expected, "unreachable")
      assert_true(endpoint.contains("127.0.0.1:__PORT__/"))
      assert_eq(Within(window_ms), resolved_reconnect)
    }
    BridgeError(message) => assert_eq(message, expected)
    error => raise error
  } noraise {
    _ => assert_eq(expected, "")
  }
}
