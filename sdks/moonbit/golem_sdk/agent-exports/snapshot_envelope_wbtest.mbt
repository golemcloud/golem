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
test "JSON snapshot envelope requires version and state" {
  let missing_version =
    #|{"state":{}}
  let unsupported_version =
    #|{"version":3,"state":{}}
  let missing_state =
    #|{"version":2}
  assert_true(
    snapshot_parse_error(json_snapshot(missing_version)).contains("version"),
  )
  assert_true(
    snapshot_parse_error(json_snapshot(unsupported_version)).contains("version"),
  )
  assert_true(
    snapshot_parse_error(json_snapshot(missing_state)).contains("state"),
  )
}

///|
test "JSON snapshot envelope reads version 2 and refuses the 1.6 envelope" {
  let current =
    #|{"version":2,"state":{"value":42}}
  let state = parse_snapshot_envelope(json_snapshot(current))
  assert_eq(state, @agents.SnapshotData::Json(b"{\"value\":42}"))
  let legacy =
    #|{"version":1,"principal":{"tag":"anonymous"},"state":{"value":42}}
  assert_true(
    snapshot_parse_error(json_snapshot(legacy)).contains(
      "Unsupported JSON snapshot version",
    ),
  )
}

///|
test "binary snapshot envelope reads version 3 and refuses every other version" {
  assert_true(
    snapshot_parse_error(binary_snapshot([])).contains("Empty binary snapshot"),
  )
  let current_state = parse_snapshot_envelope(
    binary_snapshot([b'\x03', b'\x2a']),
  )
  assert_eq(current_state, @agents.SnapshotData::Binary(b"*"))
  // The 1.6 envelopes (versions 1 and 2) are unknown versions.
  for
    payload in [
      [b'\x01', b'\x2a'],
      [b'\x02', b'\x00', b'\x00', b'\x00', b'\x02', b'{', b'}', b'\x2a'],
      [b'\x04'],
    ] {
    assert_true(
      snapshot_parse_error(binary_snapshot(FixedArray::from_array(payload))).contains(
        "Unsupported binary snapshot version",
      ),
    )
  }
}

///|
test "the JSON snapshot writer builds a version 2 envelope that the reader accepts" {
  let state = b"{\"value\":42}"
  let payload = json_snapshot_envelope(state)
  let text = @utf8.decode(Bytes::from_fixedarray(payload))
  assert_true(text.contains("\"version\":2"))
  assert_false(text.contains("\"version\":1"))
  assert_false(text.contains("principal"))
  assert_eq(
    parse_snapshot_envelope(json_snapshot_from(payload)),
    @agents.SnapshotData::Json(state),
  )
}

///|
fn json_snapshot_from(payload : FixedArray[Byte]) -> @host.Snapshot {
  { payload, mime_type: "application/json", }
}

///|
test "the binary snapshot writer puts the byte 0x03 before the state" {
  let state = b"\x01\x02\x03"
  let payload = binary_snapshot_envelope(state)
  assert_eq(payload.length(), 4)
  assert_eq(payload[0], b'\x03')
  assert_eq(payload[1], b'\x01')
  assert_eq(payload[2], b'\x02')
  assert_eq(payload[3], b'\x03')
  assert_eq(
    parse_snapshot_envelope(binary_snapshot(payload)),
    @agents.SnapshotData::Binary(state),
  )
}

///|
test "the binary snapshot writer wraps an empty state" {
  let payload = binary_snapshot_envelope(b"")
  assert_eq(payload.length(), 1)
  assert_eq(payload[0], b'\x03')
}
