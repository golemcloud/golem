///|
fn json_snapshot(payload : String) -> @host.Snapshot {
  {
    payload: FixedArray::from_array(
      payload.to_array().map(fn(char) { char.to_int().to_byte() }),
    ),
    mime_type: "application/json",
  }
}

///|
fn binary_snapshot(payload : FixedArray[Byte]) -> @host.Snapshot {
  { payload, mime_type: "application/octet-stream", }
}

///|
fn snapshot_parse_error(snapshot : @host.Snapshot) -> String raise {
  let result = try parse_snapshot_envelope(snapshot) |> Ok catch {
    error => Err(error)
  }
  guard result is Err(SnapshotLoadError(message)) else {
    fail("expected snapshot envelope parsing to fail")
  }
  message
}

///|
test "JSON snapshot envelope requires version, principal, and state" {
  let missing_version =
    #|{"principal":{"tag":"anonymous"},"state":{}}
  let unsupported_version =
    #|{"version":2,"principal":{"tag":"anonymous"},"state":{}}
  let missing_principal =
    #|{"version":1,"state":{}}
  let missing_state =
    #|{"version":1,"principal":{"tag":"anonymous"}}
  assert_true(
    snapshot_parse_error(json_snapshot(missing_version)).contains("version"),
  )
  assert_true(
    snapshot_parse_error(json_snapshot(unsupported_version)).contains("version"),
  )
  assert_true(
    snapshot_parse_error(json_snapshot(missing_principal)).contains("principal"),
  )
  assert_true(
    snapshot_parse_error(json_snapshot(missing_state)).contains("state"),
  )
}

///|
test "JSON snapshot envelope preserves version 1 state" {
  let envelope =
    #|{"version":1,"principal":{"tag":"anonymous"},"state":{"value":42}}
  let (principal, state) = parse_snapshot_envelope(json_snapshot(envelope))
  debug_inspect(principal, content="Anonymous")
  assert_eq(state, b"{\"value\":42}")
}

///|
test "binary snapshot envelope validates version 2 bounds" {
  assert_true(
    snapshot_parse_error(binary_snapshot([])).contains("Empty binary snapshot"),
  )
  assert_true(
    snapshot_parse_error(binary_snapshot([b'\x02', b'\x00'])).contains("header"),
  )
  assert_true(
    snapshot_parse_error(
      binary_snapshot([b'\x02', b'\x00', b'\x00', b'\x00', b'\x02', b'{']),
    ).contains("principal data"),
  )
  assert_true(
    snapshot_parse_error(
      binary_snapshot([b'\x02', b'\xff', b'\x00', b'\x00', b'\x00']),
    ).contains("supported size"),
  )
  assert_true(
    snapshot_parse_error(binary_snapshot([b'\x03'])).contains("version"),
  )
}

///|
test "binary snapshot envelope preserves versions 1 and 2" {
  let (legacy_principal, legacy_state) = parse_snapshot_envelope(
    binary_snapshot([b'\x01', b'\x2a']),
  )
  debug_inspect(legacy_principal, content="Anonymous")
  assert_eq(legacy_state, b"*")
  let principal =
    #|{"tag":"anonymous"}
  let principal_bytes = principal
    .to_array()
    .map(fn(char) { char.to_int().to_byte() })
  let payload = FixedArray::make(5 + principal_bytes.length() + 1, b'\x00')
  payload[0] = b'\x02'
  payload[4] = principal_bytes.length().to_byte()
  for index, byte in principal_bytes {
    payload[5 + index] = byte
  }
  payload[payload.length() - 1] = b'\x2a'
  let (restored_principal, restored_state) = parse_snapshot_envelope(
    binary_snapshot(payload),
  )
  debug_inspect(restored_principal, content="Anonymous")
  assert_eq(restored_state, b"*")
}
