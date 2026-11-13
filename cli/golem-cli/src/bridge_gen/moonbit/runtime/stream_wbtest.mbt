// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1.

///|
fn u8_public_codec() -> PublicValueCodec raise {
  public_value_codec("{\"root\":{\"kind\":\"u8\",\"value\":{}}}")
}

///|
fn[T] raises(action : () -> T raise) -> Bool {
  try {
    ignore(action())
    false
  } catch {
    _ => true
  }
}

///|
test "public codec quantity restrictions and canonical u64" {
  let quantity = public_value_codec(
    "{\"root\":{\"kind\":\"quantity\",\"value\":{\"spec\":{\"baseUnit\":\"m\",\"allowedSuffixes\":[\"cm\"],\"min\":{\"mantissa\":\"1\",\"scale\":0,\"unit\":\"m\"},\"max\":{\"mantissa\":\"200\",\"scale\":0,\"unit\":\"m\"}}}}}",
  )
  let value = QuantityValue(150L, 0, "cm")
  assert_true(quantity.decode(quantity.encode(value)) == value)
  let u64 = public_value_codec("{\"root\":{\"kind\":\"u64\",\"value\":{}}}")
  assert_true(
    u64.decode(
      @json.parse("{\"kind\":\"u64\",\"value\":\"18446744073709551615\"}"),
    ) ==
    U64Value(18446744073709551615UL),
  )
}

///|
test "public codec rejects quantity comparison overflow" {
  let quantity = public_value_codec(
    "{\"root\":{\"kind\":\"quantity\",\"value\":{\"spec\":{\"baseUnit\":\"m\",\"allowedSuffixes\":[],\"min\":{\"mantissa\":\"1\",\"scale\":-2147483648,\"unit\":\"m\"}}}}}",
  )
  inspect(
    raises(() => {
      quantity.decode(
        @json.parse(
          "{\"kind\":\"quantity\",\"value\":{\"mantissa\":\"1\",\"scale\":2147483647,\"unit\":\"m\"}}",
        ),
      )
    }),
    content="true",
  )
}

///|
test "public codec accepts non-hierarchical URLs without host restrictions" {
  let unrestricted = public_value_codec(
    "{\"root\":{\"kind\":\"url\",\"value\":{\"restrictions\":{\"allowedSchemes\":[\"mailto\"]}}}}",
  )
  assert_true(
    unrestricted.decode(
      @json.parse(
        "{\"kind\":\"url\",\"value\":{\"url\":\"mailto:user@example.com\"}}",
      ),
    ) ==
    UrlValue("mailto:user@example.com"),
  )
  let hosted = public_value_codec(
    "{\"root\":{\"kind\":\"url\",\"value\":{\"restrictions\":{\"allowedHosts\":[\"example.com\"]}}}}",
  )
  inspect(
    raises(() => {
      hosted.decode(
        @json.parse(
          "{\"kind\":\"url\",\"value\":{\"url\":\"mailto:user@example.com\"}}",
        ),
      )
    }),
    content="true",
  )
}

///|
test "full u32 channel is rendered without sign loss" {
  inspect(json_uint(4294967295U).stringify(), content="4294967295")
  inspect(
    @string.parse_uint(number_literal(json_uint(4294967295U))),
    content="4294967295",
  )
}

///|
test "public codec preserves record s64 binary and structural wire shapes" {
  let codec = public_value_codec(
    "{\"root\":{\"kind\":\"record\",\"value\":{\"fields\":[{\"name\":\"count\",\"body\":{\"kind\":\"s64\",\"value\":{}}},{\"name\":\"data\",\"body\":{\"kind\":\"binary\",\"value\":{}}},{\"name\":\"choice\",\"body\":{\"kind\":\"option\",\"value\":{\"inner\":{\"kind\":\"string\",\"value\":{}}}}}]}}}",
  )
  let value = RecordValue([
    S64Value(-9L),
    BinaryValue(b"\x00\xff", Some("application/octet-stream")),
    OptionValue(Some(StringValue("yes"))),
  ])
  let json = codec.encode(value)
  inspect(
    json.stringify(),
    content=(
      #|{"kind":"record","value":{"fields":[{"kind":"s64","value":"-9"},{"kind":"binary","value":{"bytes":[0,255],"mimeType":"application/octet-stream"}},{"kind":"option","value":{"inner":{"kind":"string","value":"yes"}}}]}}
    ),
  )
  assert_true(codec.decode(json) == value)
}

///|
test "public codec binds nested stream element codecs" {
  let codec = public_value_codec(
    "{\"root\":{\"kind\":\"stream\",\"value\":{\"inner\":{\"kind\":\"s64\",\"value\":{}}}}}",
  )
  let token = "opaque-output"
  ignore(
    codec.decode(
      @json.parse(
        "{\"kind\":\"stream\",\"value\":{\"streamToken\":\"opaque-output\"}}",
      ),
    ),
  )
  guard codec.stream_elements.get(token) is Some(element) else {
    fail("nested stream codec was not bound")
  }
  assert_true(
    element.decode(@json.parse("{\"kind\":\"s64\",\"value\":\"42\"}")) ==
    S64Value(42L),
  )
}

///|
async test "AgentStream delivers items once and becomes terminal" {
  let values = [Some(7), None]
  let stream = AgentStream::from_next(fn() { values.remove(0) })
  debug_inspect(stream.next(), content="Some(7)")
  debug_inspect(stream.next(), content="None")
  let failed = try {
    ignore(stream.next())
    false
  } catch {
    _ => true
  }
  inspect(failed, content="true")
}

///|
async test "AgentStream cancel is affine and reports its reason once" {
  let reasons : Array[String] = []
  let stream : AgentStream[Int32] = AgentStream::from_next(fn() { None }, on_cancel=fn(
    reason,
  ) {
    reasons.push(reason)
  })
  stream.cancel()
  stream.cancel()
  debug_inspect(reasons, content="[\"cancelled\"]")
}

///|
test "stream protocol message is canonical v1 JSON" {
  inspect(
    protocol_message("stream-end", [
      ("channel", json_int(2)),
      ("direction", Json::string("output")),
      (
        "outcome",
        obj([
          ("details", Json::string("consumer-drop")),
          ("kind", Json::string("consumer-cancelled")),
        ]),
      ),
      ("sequence", Json::string("0")),
    ]),
    content=(
      #|{"channel":2,"direction":"output","outcome":{"details":"consumer-drop","kind":"consumer-cancelled"},"sequence":"0","type":"stream-end","version":1}
    ),
  )
}

///|
test "canonical unsigned decimal rejects leading zero and overflow" {
  inspect(parse_canonical_u64(Json::string("0")), content="0")
  inspect(raises(() => parse_canonical_u64(Json::string("01"))), content="true")
  inspect(
    raises(() => parse_canonical_u64(Json::string("18446744073709551616"))),
    content="true",
  )
}

///|
test "binary envelope round trips raw payload" {
  let metadata = obj([
    ("channel", json_int(1)),
    ("itemCount", Json::string("3")),
    ("kind", Json::string("input-u8")),
    ("sequence", Json::string("0")),
    ("version", json_int(1)),
  ])
  let frame = encode_binary_envelope(metadata, b"\x00\x7f\xff")
  let (decoded, payload) = decode_binary_envelope(frame)
  inspect(
    decoded.stringify(),
    content="{\"channel\":1,\"itemCount\":\"3\",\"kind\":\"input-u8\",\"sequence\":\"0\",\"version\":1}",
  )
  inspect(payload, content="b\"\\x00\\x7f\\xff\"")
}

///|
test "binary envelope rejects truncated metadata" {
  inspect(
    raises(() => decode_binary_envelope(b"\x00\x00\x00\x10{}")),
    content="true",
  )
}

///|
test "ACK gating accepts only the pending range and preserves replay evidence" {
  validate_ack_window(7UL, 7UL, 1UL, 7UL)
  validate_ack_window(7UL, 7UL, 1UL, 8UL)
  inspect(raises(() => validate_ack_window(7UL, 7UL, 1UL, 6UL)), content="true")
  inspect(raises(() => validate_ack_window(7UL, 7UL, 1UL, 9UL)), content="true")
}

///|
test "binary MIME validation is strict and u8 metadata forbids MIME" {
  inspect(validate_mime_type("image/png"), content="image/png")
  inspect(raises(() => validate_mime_type("image png")), content="true")
  inspect(raises(() => validate_mime_type("image/")), content="true")
}

///|
test "packed u8 envelope carries every logical byte in one prompt batch" {
  let metadata = obj([
    ("channel", json_int(3)),
    ("cursorToken", Json::string("cursor")),
    ("itemCount", Json::string("3")),
    ("kind", Json::string("output-u8")),
    ("sequence", Json::string("9")),
    ("version", json_int(1)),
  ])
  let (decoded, payload) = decode_binary_envelope(
    encode_binary_envelope(metadata, b"\x01\x02\x03"),
  )
  inspect(
    get_field(expect_object(decoded), "itemCount").stringify(),
    content="\"3\"",
  )
  inspect(payload, content="b\"\\x01\\x02\\x03\"")
}

///|
test "server input failures preserve their typed outcome" {
  for kind in ["trapped", "interrupted"] {
    let outcome = Json::object({
      "kind": Json::string("failed"),
      "error": Json::object({
        "kind": Json::string(kind),
        "message": Json::string("callee failed"),
      }),
    })
    let message = expect_object(
      Json::object({
        "version": json_uint(1U),
        "type": Json::string("stream-end"),
        "channel": json_uint(1U),
        "direction": Json::string("input"),
        "sequence": Json::string("0"),
        "outcome": outcome,
      }),
    )
    assert_eq(validate_server_stream_end(message), "input")
  }
}

///|
test "strict protocol JSON rejects unknown fields and variants" {
  let valid = expect_object(
    @json.parse(
      (
        #|{"channel":1,"direction":"input","outcome":{"kind":"consumer-cancelled"},"sequence":"0","type":"stream-end","version":1}
      ),
    ),
  )
  validate_message_fields(valid, "stream-end")
  let unknown = expect_object(
    @json.parse(
      (
        #|{"channel":1,"direction":"input","extra":true,"outcome":{"kind":"consumer-cancelled"},"sequence":"0","type":"stream-end","version":1}
      ),
    ),
  )
  inspect(
    raises(() => validate_message_fields(unknown, "stream-end")),
    content="true",
  )
  inspect(raises(() => validate_message_fields(valid, "bogus")), content="true")
}

///|
test "stream references use the public recursive representation" {
  let provisional = StreamReferenceValue(
    Some("0dff1c71-f12f-4bb1-996c-23d693bdc825"),
    None,
  )
  inspect(
    schema_value_to_json(provisional).stringify(),
    content=(
      #|{"kind":"stream","value":{"provisionalRef":"0dff1c71-f12f-4bb1-996c-23d693bdc825"}}
    ),
  )
  assert_true(
    schema_value_from_json(schema_value_to_json(provisional)) == provisional,
  )
  inspect(
    raises(() => {
      schema_value_from_json(
        @json.parse(
          (
            #|{"kind":"stream","value":{"provisionalRef":"x","streamToken":"y"}}
          ),
        ),
      )
    }),
    content="true",
  )
}

///|
test "stream encode contexts isolate registrations" {
  let left = StreamEncodeContext::new()
  let right = StreamEncodeContext::new()
  let left_stream = AgentStream::from_next(fn() { None })
  let right_stream = AgentStream::from_next(fn() { None })
  ignore(
    register_input_stream(
      left,
      left_stream,
      fn(value) { U8Value(value) },
      u8_public_codec(),
    ),
  )
  ignore(
    register_input_stream(
      right,
      right_stream,
      fn(value) { U8Value(value) },
      u8_public_codec(),
    ),
  )
  inspect(left.inputs.length(), content="1")
  inspect(right.inputs.length(), content="1")
  inspect(left.inputs[0].id == right.inputs[0].id, content="false")
}

///|
test "duplicate stream registration is an affine error" {
  let context = StreamEncodeContext::new()
  let stream = AgentStream::from_next(fn() { None })
  ignore(
    register_input_stream(
      context,
      stream,
      fn(value) { U8Value(value) },
      u8_public_codec(),
    ),
  )
  inspect(
    raises(() => {
      register_input_stream(
        context,
        stream,
        fn(value) { U8Value(value) },
        u8_public_codec(),
      )
    }),
    content="true",
  )
}

///|
test "strict protocol JSON rejects duplicate members at every depth" {
  inspect(raises(() => parse_strict_json("{\"a\":1,\"a\":2}")), content="true")
  inspect(
    raises(() => parse_strict_json("{\"a\":{\"b\":1,\"\\u0062\":2}}")),
    content="true",
  )
  inspect(
    parse_strict_json("{\"a\":[{\"b\":1},{\"b\":2}]}").stringify(),
    content="{\"a\":[{\"b\":1},{\"b\":2}]}",
  )
}

///|
async test "MemoryReader owns a detached receive actor" {
  let delivered : @aqueue.Queue[Int32] = @aqueue.Queue(kind=@aqueue.Blocking(1))
  let owner = @io.MemoryReader(async fn(_) {
    @async.sleep(1)
    delivered.put(7)
  })
  inspect(delivered.get(), content="7")
  owner.close()
}

///|
async test "packed u8 input prefetch is bounded and non-blocking" {
  let values = [Some(1), Some(2), Some(3), None]
  let stream = AgentStream::from_next(fn() { values.remove(0) })
  let context = StreamEncodeContext::new()
  ignore(
    register_input_stream(
      context,
      stream,
      fn(value) { U8Value(value) },
      u8_public_codec(),
      wire_kind="u8",
    ),
  )
  let input = context.inputs[0]
  inspect((input.next)() is Some(U8Value(1)), content="true")
  @async.sleep(1)
  let prefetched : Array[Int32] = []
  for ;; {
    match (input.try_next)() {
      Some(Some(U8Value(value))) => prefetched.push(value)
      Some(None) => break
      Some(Some(_)) => fail("expected u8")
      None => {
        @async.sleep(1)
        continue
      }
    }
  }
  debug_inspect(prefetched, content="[2, 3]")
}

///|
test "partial packed ACK retains exactly the undelivered suffix" {
  let pending = {
    start: 0UL,
    count: 3UL,
    payload: PendingU8(b"\x01\x02\x03"),
    bytes: 3,
  }
  inspect(trim_pending_u8(pending, 1UL), content="1")
  inspect(pending.start, content="1")
  inspect(pending.count, content="2")
  inspect(pending.bytes, content="2")
  guard pending.payload is PendingU8(suffix) else {
    fail("expected packed suffix")
  }
  inspect(suffix, content="b\"\\x02\\x03\"")
}

///|
test "stream terminal preserves every producer failure and cancellation details" {
  assert_true(
    decode_stream_outcome(parse_strict_json("{\"kind\":\"finished\"}")) is None,
  )
  for details in ["", ",\"details\":null", ",\"details\":\"dropped\""] {
    let error = decode_stream_outcome(
      parse_strict_json("{\"kind\":\"consumer-cancelled\"\{details}}"),
    )
    assert_true(error is Some(ConsumerCancelled(_)))
  }
  for
    entry in [
      ("trapped", StreamFailure::Trapped("diagnostic")),
      ("interrupted", StreamFailure::Interrupted("diagnostic")),
      ("resource-exhausted", StreamFailure::ResourceExhausted("diagnostic")),
      ("cancelled", StreamFailure::Cancelled),
      ("abandoned", StreamFailure::Abandoned),
    ] {
    let (kind, expected) = entry
    let fields = [("kind", Json::string(kind))]
    if kind != "cancelled" && kind != "abandoned" {
      fields.push(("message", Json::string("diagnostic")))
    }
    let error = decode_stream_outcome(
      obj([("kind", Json::string("failed")), ("error", obj(fields))]),
    )
    assert_eq(error, Some(StreamFailed(expected)))
  }
  let graph = parse_strict_json("{\"root\":{\"kind\":\"string\",\"value\":{}}}")
  let value = parse_strict_json(
    "{\"kind\":\"string\",\"value\":\"application failure\"}",
  )
  let error = decode_stream_outcome(
    obj([
      ("kind", Json::string("failed")),
      (
        "error",
        obj([
          ("kind", Json::string("failed")),
          ("value", obj([("graph", graph), ("value", value)])),
        ]),
      ),
    ]),
  )
  assert_eq(error, Some(StreamFailed(Failed(graph~, value~))))
  assert_true(Show::to_string(error.unwrap()).contains("application failure"))
  let cancelled : BridgeError = ConsumerCancelled(Some("consumer diagnostic"))
  assert_eq(
    Show::to_string(cancelled),
    "Stream consumer cancelled: consumer diagnostic",
  )
}

///|
test "stream terminal rejects unknown outcomes errors and malformed payloads" {
  for
    value in [
      "{\"kind\":\"ok\"}", "{\"kind\":\"finished\",\"message\":\"extra\"}", "{\"kind\":\"consumer-cancelled\",\"details\":1}",
      "{\"kind\":\"failed\",\"error\":{\"kind\":\"unknown\"}}", "{\"kind\":\"failed\",\"error\":{\"kind\":\"trapped\"}}",
      "{\"kind\":\"failed\",\"error\":{\"kind\":\"cancelled\",\"message\":\"extra\"}}",
      "{\"kind\":\"failed\",\"error\":{\"kind\":\"failed\",\"value\":{\"graph\":{\"root\":{\"kind\":\"string\",\"value\":{}}},\"value\":{\"kind\":\"u8\",\"value\":42}}}}",
    ] {
    assert_true(raises(() => decode_stream_outcome(parse_strict_json(value))))
  }
}

///|
test "server stream terminal uses explicit direction and canonical sequence" {
  let message = expect_object(
    parse_strict_json(
      "{\"version\":1,\"type\":\"stream-end\",\"channel\":1,\"direction\":\"input\",\"sequence\":\"0\",\"outcome\":{\"kind\":\"consumer-cancelled\"}}",
    ),
  )
  assert_eq(validate_server_stream_end(message), "input")
  message["outcome"] = obj([("kind", Json::string("finished"))])
  assert_eq(validate_server_stream_end(message), "input")
  message["direction"] = Json::string("output")
  message["cursorToken"] = Json::null()
  assert_eq(validate_server_stream_end(message), "output")
  message["sequence"] = Json::string("01")
  assert_true(raises(() => validate_server_stream_end(message)))
  message["sequence"] = Json::string("0")
  message["direction"] = Json::string("unknown")
  assert_true(raises(() => validate_server_stream_end(message)))
}

///|
async test "packed input failure transfers the diagnostic out of its prefetch queue" {
  for prefix in [false, true] {
    let pulled = Ref(false)
    let source : AgentStream[Byte] = AgentStream::from_next(() => {
      if prefix && !pulled.val {
        pulled.val = true
        Some(b'\x2a')
      } else {
        raise BridgeError("界🦄")
      }
    })
    let context = StreamEncodeContext::new()
    ignore(
      register_input_stream(
        context,
        source,
        value => U8Value(value.to_int()),
        u8_public_codec(),
        wire_kind="u8",
      ),
    )
    let input = context.inputs[0]
    @async.sleep(1)
    assert_true(input.failure.bytes > 0)
    assert_eq(context.replay_bytes.val, input.failure.bytes)
    assert_false(input.source_failed)
    assert_true(input.failure.diagnostic.contains("界🦄"))
    if prefix {
      assert_eq((input.next)(), Some(U8Value(42)))
      try (input.try_next)() catch {
        @aqueue.QueueAlreadyClosed => ()
        error => raise error
      } noraise {
        _ => fail("expected the prefetched failure")
      }
    } else {
      try (input.next)() catch {
        @aqueue.QueueAlreadyClosed => ()
        error => raise error
      } noraise {
        _ => fail("expected the initial failure")
      }
    }
    try (input.try_next)() catch {
      @aqueue.QueueAlreadyClosed => ()
      error => raise error
    } noraise {
      _ => fail("the prefetch queue must not retain the diagnostic")
    }
    (input.cancel)()
    assert_eq(context.replay_bytes.val, 0)
    assert_eq(input.failure.diagnostic, "")
  }
}

///|
async test "packed input cancellation releases an unobserved prefetched diagnostic" {
  let pulled = Ref(false)
  let source : AgentStream[Byte] = AgentStream::from_next(() => {
    if !pulled.val {
      pulled.val = true
      Some(b'\x2a')
    } else {
      raise BridgeError("界🦄")
    }
  })
  let context = StreamEncodeContext::new()
  ignore(
    register_input_stream(
      context,
      source,
      value => U8Value(value.to_int()),
      u8_public_codec(),
      wire_kind="u8",
    ),
  )
  let input = context.inputs[0]
  @async.sleep(1)
  assert_true(source.terminal)
  assert_true(context.replay_bytes.val > 0)
  (input.cancel)()
  assert_eq(context.replay_bytes.val, 0)
  assert_eq(input.failure.diagnostic, "")
  try (input.try_next)() catch {
    @aqueue.QueueAlreadyClosed => ()
    error => raise error
  } noraise {
    _ => fail("cancellation must release the prefix and diagnostic")
  }
}

///|
async test "prefetch admits combined diagnostics before attachment and rejects excess bytes" {
  let context = StreamEncodeContext::new()
  for _ in 0..<2 {
    let source : AgentStream[Byte] = AgentStream::from_next(() => {
      raise BridgeError("界".repeat(3 * 1024 * 1024))
    })
    ignore(
      register_input_stream(
        context,
        source,
        value => U8Value(value.to_int()),
        u8_public_codec(),
        wire_kind="u8",
      ),
    )
  }
  @async.sleep(1)
  let retained = context.inputs.filter(input => input.failure.bytes > 0)
  let rejected = context.inputs.filter(input => input.failure.rejected)
  assert_eq(retained.length(), 1)
  assert_eq(rejected.length(), 1)
  assert_eq(context.replay_bytes.val, retained[0].failure.bytes)
  assert_true(context.replay_bytes.val > 9 * 1024 * 1024)
  assert_true(context.replay_bytes.val <= MAX_INPUT_REPLAY_BYTES)
  assert_eq(rejected[0].failure.diagnostic, "")
  assert_true(
    raises(() => {
      retain_input_failure(rejected[0], BridgeError("rejected source"))
    }),
  )
  for input in context.inputs {
    (input.cancel)()
  }
  @async.sleep(1)
  assert_eq(context.replay_bytes.val, 0)
  for input in context.inputs {
    assert_eq(input.failure.bytes, 0)
    assert_eq(input.failure.diagnostic, "")
  }
}

///|
async test "cancelled prefetch owner cannot admit a late diagnostic" {
  let gate = @cond.Cond()
  let source : AgentStream[Byte] = AgentStream::from_next(async fn() {
    gate.wait()
    raise BridgeError("late 界🦄 failure")
  })
  let context = StreamEncodeContext::new()
  ignore(
    register_input_stream(
      context,
      source,
      value => U8Value(value.to_int()),
      u8_public_codec(),
      wire_kind="u8",
    ),
  )
  @async.sleep(1)
  let input = context.inputs[0]
  (input.cancel)()
  gate.broadcast()
  @async.sleep(1)
  assert_true(input.failure.disposed)
  assert_eq(context.replay_bytes.val, 0)
  assert_eq(input.failure.bytes, 0)
  assert_eq(input.failure.diagnostic, "")
}
