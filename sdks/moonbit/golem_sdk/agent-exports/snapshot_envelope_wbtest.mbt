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
test "JSON snapshot envelope requires version, state, and a principal in a 1.6 envelope" {
  let missing_version =
    #|{"principal":{"tag":"anonymous"},"state":{}}
  let unsupported_version =
    #|{"version":3,"state":{}}
  let missing_principal =
    #|{"version":1,"state":{}}
  let missing_state =
    #|{"version":2}
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
test "JSON snapshot envelope preserves version 1 and version 2 state" {
  let legacy =
    #|{"version":1,"principal":{"tag":"anonymous"},"state":{"value":42}}
  let state = parse_snapshot_envelope(json_snapshot(legacy))
  assert_eq(state, @agents.SnapshotData::Json(b"{\"value\":42}"))
  let current =
    #|{"version":2,"state":{"value":42}}
  let state = parse_snapshot_envelope(json_snapshot(current))
  assert_eq(state, @agents.SnapshotData::Json(b"{\"value\":42}"))
  let malformed =
    #|{"version":1,"principal":{"tag":"agent","val":{"componentId":"nope","agentId":"a"}},"state":{}}
  assert_true(
    snapshot_parse_error(json_snapshot(malformed)).contains("principal"),
  )
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
    snapshot_parse_error(binary_snapshot([b'\x04'])).contains("version"),
  )
}

///|
test "binary snapshot envelope preserves versions 1, 2 and 3" {
  let legacy_state = parse_snapshot_envelope(
    binary_snapshot([b'\x01', b'\x2a']),
  )
  assert_eq(legacy_state, @agents.SnapshotData::Binary(b"*"))
  let current_state = parse_snapshot_envelope(
    binary_snapshot([b'\x03', b'\x2a']),
  )
  assert_eq(current_state, @agents.SnapshotData::Binary(b"*"))
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
  let restored_state = parse_snapshot_envelope(binary_snapshot(payload))
  assert_eq(restored_state, @agents.SnapshotData::Binary(b"*"))
}
