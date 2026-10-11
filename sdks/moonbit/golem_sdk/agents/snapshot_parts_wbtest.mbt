///|
fn snapshot_test_wire(
  parts : Array[(String, String, Bytes)],
  boundary? : String = "fixture-boundary",
  newline? : String = "\r\n",
) -> Bytes {
  let out = @buffer.Buffer()
  for entry in parts {
    let (name, mime, bytes) = entry
    out.write_string_utf8(
      "--\{boundary}\{newline}Content-Disposition: attachment; name=\"\{name}\"\{newline}Content-Type: \{mime}\{newline}\{newline}",
    )
    out.write_bytes(bytes)
    out.write_string_utf8(newline)
  }
  out.write_string_utf8("--\{boundary}--\{newline}")
  out.to_bytes()
}

///|
fn snapshot_test_envelope() -> Bytes {
  b"{\"version\":1,\"principal\":{\"tag\":\"anonymous\"},\"state\":null}"
}

///|
test "multipart framing preserves payload tails and exact delimiter near misses" {
  for
    bytes in [
      b"", b"X\r", b"X\n", b"X\r\n", b"X--fixture-boundaryY", b"X\r\n--fixture-boundaryextra\r\nY",
      b"X\r\n--fixture-boundary--extra\r\nY", b"\x00\xff\r\n",
    ] {
    for newline in ["\r\n", "\n"] {
      let wire = snapshot_test_wire(
        [
          ("part:index", "application/octet-stream", bytes),
          ("state", "application/json", snapshot_test_envelope()),
        ],
        newline~,
      )
      guard decode_multipart_snapshot(
          wire, "multipart/mixed; boundary=fixture-boundary",
        )
        is Ok((_, saved)) else {
        fail("valid wire rejected")
      }
      assert_eq(
        saved.require_part("index", "application/octet-stream"),
        Ok(bytes),
      )
    }
  }
}

///|
priv struct SnapshotSavedState {
  revision : Int
  label : String
} derive(ToJson, FromJson, Debug, Eq)

///|
test "multipart carrier roundtrip Unicode dynamic names and all bytes" {
  let bytes = Bytes::from_array([ for i in 0..<256 => i.to_byte() ]) + b"\r\n"
  let state = SnapshotSavedState::{ revision: 7, label: "索引 🦀", }
  let saved = MultipartSnapshot::from_state(state, {
    "state": { bytes, content_type: "APPLICATION/OCTET-STREAM", },
    "__proto__": { bytes: b"", content_type: "image/png", },
    "Index-2.1": { bytes: b"\xff\x00", content_type: "image/jpeg", },
  })
  let principal = principal_from_json(
    @json.parse(
      "{\"tag\":\"oidc\",\"val\":{\"sub\":\"用户 🦀\",\"issuer\":\"发行者\",\"claims\":\"{}\"}}",
    ),
  )
  guard encode_multipart_snapshot(saved, principal) is Ok((wire, mime)) else {
    fail("save failed")
  }
  guard decode_multipart_snapshot(wire, mime)
    is Ok((restored_principal, restored)) else {
    fail("restore failed")
  }
  assert_eq(restored_principal, principal)
  let restored_state : Result[SnapshotSavedState, String] = restored.decode_state()
  assert_eq(restored_state, Ok(state))
  assert_eq(restored.parts.length(), 3)
  assert_eq(
    restored.require_part("state", "application/octet-stream"),
    Ok(bytes),
  )
  assert_true(
    restored.require_part("State", "application/octet-stream") is Err(_),
  )
  assert_true(restored.require_part("state", "image/png") is Err(_))
  assert_true(
    restored.require_part("state", "application/octet-stream; charset=utf8")
    is Err(_),
  )
}

///|
test "multipart empty bundle stays multipart and boundary collisions regenerate" {
  let empty = MultipartSnapshot::from_state(Json::null(), Map([]))
  guard encode_multipart_snapshot(empty, @common.Principal::Anonymous)
    is Ok((wire, mime)) else {
    fail("save failed")
  }
  assert_true(mime.has_prefix("multipart/mixed"))
  guard decode_multipart_snapshot(wire, mime) is Ok((_, restored)) else {
    fail("restore failed")
  }
  assert_eq(restored, empty)
  for
    bytes in [
      b"--golem-snapshot-0", b"--golem-snapshot-0--", b"X\r\n--golem-snapshot-0\r\nY",
    ] {
    let saved = MultipartSnapshot::from_state(Json::null(), {
      "x": { bytes, content_type: "application/octet-stream", },
    })
    guard encode_multipart_snapshot(saved, @common.Principal::Anonymous)
      is Ok((wire, mime)) else {
      fail("save failed")
    }
    assert_true(mime.contains("golem-snapshot-1"))
    guard decode_multipart_snapshot(wire, mime) is Ok((_, restored)) else {
      fail("restore failed")
    }
    assert_eq(restored, saved)
  }
}

///|
test "multipart rejects SDK metadata shape duplicates and lexical version errors" {
  for
    text in [
      "[]", "{\"version\":1.0,\"principal\":{\"tag\":\"anonymous\"},\"state\":null}",
      "{\"version\":1e0,\"principal\":{\"tag\":\"anonymous\"},\"state\":null}", "{\"version\":1,\"version\":1,\"principal\":{\"tag\":\"anonymous\"},\"state\":null}",
      "{\"version\":1,\"principal\":[],\"state\":null}", "{\"version\":1,\"principal\":{\"tag\":\"anonymous\",\"t\\u0061g\":\"anonymous\"},\"state\":null}",
      "{\"version\":1,\"principal\":{\"tag\":\"oidc\",\"val\":[]},\"state\":null}",
      "{\"version\":1,\"principal\":{\"tag\":\"agent\",\"val\":[]},\"state\":null}",
      "{\"version\":1,\"principal\":{\"tag\":\"golem-user\",\"val\":[]},\"state\":null}",
      "{\"version\":1,\"principal\":{\"tag\":\"oidc\",\"val\":{\"sub\":\"a\",\"issuer\":\"b\",\"claims\":\"{}\",\"email\":17}},\"state\":null}",
      "{\"version\":1,\"principal\":{\"tag\":\"anonymous\"}}",
    ] {
    let wire = snapshot_test_wire([
      ("state", "application/json", @utf8.encode(text)),
    ])
    assert_true(
      decode_multipart_snapshot(
        wire, "multipart/mixed; boundary=fixture-boundary",
      )
      is Err(_),
    )
  }
  let invalid_utf8 = snapshot_test_wire([("state", "application/json", b"\xff")])
  assert_true(
    decode_multipart_snapshot(
      invalid_utf8, "multipart/mixed; boundary=fixture-boundary",
    )
    is Err(_),
  )
}

///|
test "multipart rejects malformed framing structural headers names MIME and namespaces" {
  let state = ("state", "application/json", snapshot_test_envelope())
  for
    part in [
      ("part:a", "text/plain; charset=utf8", b""),
      ("part:../x", "text/plain", b""),
      ("db:x", "application/x-sqlite3", b""),
      ("other:x", "text/plain", b""),
    ] {
    assert_true(
      decode_multipart_snapshot(
        snapshot_test_wire([state, part]),
        "multipart/mixed; boundary=fixture-boundary",
      )
      is Err(_),
    )
  }
  assert_true(
    decode_multipart_snapshot(
      snapshot_test_wire([state, state]),
      "multipart/mixed; boundary=fixture-boundary",
    )
    is Err(_),
  )
  let wire = snapshot_test_wire([state])
  for
    malformed in [
      wire[:wire.length() - 4].to_bytes(),
      wire + b"epilogue",
      b"preamble" + wire,
      b"--fixture-boundary\r\nContent-Type: application/json\r\nContent-Type: application/json\r\n\r\n{}\r\n--fixture-boundary--\r\n",
    ] {
    assert_true(
      decode_multipart_snapshot(
        malformed, "multipart/mixed; boundary=fixture-boundary",
      )
      is Err(_),
    )
  }
  for name in ["", "a/b", "a\r\nx", "../a"] {
    let saved = MultipartSnapshot::from_state(
      Json::null(),
      Map([(name, { bytes: b"", content_type: "text/plain", })]),
    )
    assert_true(
      encode_multipart_snapshot(saved, @common.Principal::Anonymous) is Err(_),
    )
  }
  let quoted = snapshot_test_wire([state], boundary="b:c")
  assert_true(
    decode_multipart_snapshot(quoted, "multipart/mixed; boundary=\"b:c\"")
    is Ok(_),
  )
  assert_true(
    decode_multipart_snapshot(quoted, "multipart/mixed; boundary=b:c") is Err(_),
  )
}
