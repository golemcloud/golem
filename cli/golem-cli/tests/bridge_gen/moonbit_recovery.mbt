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
  let expected = "__ERROR__"
  try run_recovery() catch {
    BridgeError(message) => assert_eq(message, expected)
    error => raise error
  } noraise {
    _ => assert_eq(expected, "")
  }
}
