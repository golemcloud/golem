// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1.

///|
fn u8_public_codec() -> PublicValueCodec raise {
  public_value_codec("{\"root\":{\"kind\":\"u8\",\"value\":{}}}")
}

///|
test "public codec quantity restrictions and canonical u64" {
  let quantity = public_value_codec("{\"root\":{\"kind\":\"quantity\",\"value\":{\"spec\":{\"baseUnit\":\"m\",\"allowedSuffixes\":[\"cm\"],\"min\":{\"mantissa\":\"1\",\"scale\":0,\"unit\":\"m\"},\"max\":{\"mantissa\":\"200\",\"scale\":0,\"unit\":\"m\"}}}}}")
  let value = QuantityValue(150L, 0, "cm")
  assert_true(quantity.decode(quantity.encode(value)) == value)
  let u64 = public_value_codec("{\"root\":{\"kind\":\"u64\",\"value\":{}}}")
  assert_true(u64.decode(Json::string("18446744073709551615")) == U64Value(18446744073709551615UL))
}

///|
test "public codec rejects quantity comparison overflow" {
  let quantity = public_value_codec("{\"root\":{\"kind\":\"quantity\",\"value\":{\"spec\":{\"baseUnit\":\"m\",\"allowedSuffixes\":[],\"min\":{\"mantissa\":\"1\",\"scale\":-2147483648,\"unit\":\"m\"}}}}}")
  inspect(
    (try? quantity.decode(@json.parse("{\"mantissa\":\"1\",\"scale\":2147483647,\"unit\":\"m\"}")))
    is Err(_),
    content="true",
  )
}

///|
test "public codec accepts non-hierarchical URLs without host restrictions" {
  let unrestricted = public_value_codec("{\"root\":{\"kind\":\"url\",\"value\":{\"restrictions\":{\"allowedSchemes\":[\"mailto\"]}}}}")
  assert_true(
    unrestricted.decode(Json::string("mailto:user@example.com")) ==
    UrlValue("mailto:user@example.com"),
  )
  let hosted = public_value_codec("{\"root\":{\"kind\":\"url\",\"value\":{\"restrictions\":{\"allowedHosts\":[\"example.com\"]}}}}")
  inspect(
    (try? hosted.decode(Json::string("mailto:user@example.com"))) is Err(_),
    content="true",
  )
}

///|
test "full u32 channel is rendered without sign loss" {
  inspect(json_uint(4294967295U).stringify(), content="4294967295")
  inspect(@string.parse_uint(number_literal(json_uint(4294967295U))), content="4294967295")
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
      #|{"count":"-9","data":{"bytes":"AP8=","mimeType":"application/octet-stream"},"choice":{"$option":"some","value":"yes"}}
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
  ignore(codec.decode(@json.parse("{\"$stream\":{\"streamToken\":\"opaque-output\"}}")))
  guard codec.stream_elements.get(token) is Some(element) else {
    fail("nested stream codec was not bound")
  }
  assert_true(element.decode(Json::string("42")) == S64Value(42L))
}

///|
async test "AgentStream delivers items once and becomes terminal" {
  let values = [Some(7), None]
  let stream = AgentStream::from_next(async fn() { values.remove(0) })
  inspect(stream.next(), content="Some(7)")
  inspect(stream.next(), content="None")
  let failed = try? stream.next()
  inspect(failed is Err(_), content="true")
}

///|
async test "AgentStream cancel is affine and reports its reason once" {
  let reasons : Array[String] = []
  let stream : AgentStream[Int] = AgentStream::from_next(async fn() { None }, on_cancel=async fn(
    reason,
  ) {
    reasons.push(reason)
  })
  stream.cancel()
  stream.cancel()
  inspect(reasons, content="[cancelled]")
}

///|
test "stream protocol message is canonical v1 JSON" {
  inspect(
    protocol_message("streamCancel", [
      ("channel", json_int(2)),
      ("reason", Json::string("consumer-drop")),
    ]),
    content=(
      #|{"channel":2,"reason":"consumer-drop","type":"streamCancel","version":1}
    ),
  )
}

///|
test "canonical unsigned decimal rejects leading zero and overflow" {
  inspect(parse_canonical_u64(Json::string("0")), content="0")
  inspect(
    (try? parse_canonical_u64(Json::string("01"))) is Err(_),
    content="true",
  )
  inspect(
    (try? parse_canonical_u64(Json::string("18446744073709551616"))) is Err(_),
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
    (try? decode_binary_envelope(b"\x00\x00\x00\x10{}")) is Err(_),
    content="true",
  )
}

///|
test "ACK gating accepts only the pending range and preserves replay evidence" {
  validate_ack_window(7UL, 7UL, 1UL, 7UL)
  validate_ack_window(7UL, 7UL, 1UL, 8UL)
  inspect(
    (try? validate_ack_window(7UL, 7UL, 1UL, 6UL)) is Err(_),
    content="true",
  )
  inspect(
    (try? validate_ack_window(7UL, 7UL, 1UL, 9UL)) is Err(_),
    content="true",
  )
}

///|
test "binary MIME validation is strict and u8 metadata forbids MIME" {
  inspect(validate_mime_type("image/png"), content="image/png")
  inspect((try? validate_mime_type("image png")) is Err(_), content="true")
  inspect((try? validate_mime_type("image/")) is Err(_), content="true")
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
  inspect(get_field(expect_object(decoded), "itemCount"), content="String(3)")
  inspect(payload, content="b\"\\x01\\x02\\x03\"")
}

///|
test "strict protocol JSON rejects unknown fields and variants" {
  let valid = expect_object(
    @json.parse(
      (
        #|{"channel":1,"reason":"cancelled","type":"streamCancel","version":1}
      ),
    ),
  )
  validate_message_fields(valid, "streamCancel")
  let unknown = expect_object(
    @json.parse(
      (
        #|{"channel":1,"extra":true,"reason":"cancelled","type":"streamCancel","version":1}
      ),
    ),
  )
  inspect(
    (try? validate_message_fields(unknown, "streamCancel")) is Err(_),
    content="true",
  )
  inspect(
    (try? validate_message_fields(valid, "bogus")) is Err(_),
    content="true",
  )
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
      #|{"$stream":{"provisionalRef":"0dff1c71-f12f-4bb1-996c-23d693bdc825"}}
    ),
  )
  assert_true(
    schema_value_from_json(schema_value_to_json(provisional)) == provisional,
  )
  inspect(
    (try? schema_value_from_json(
      @json.parse(
        (
          #|{"$stream":{"provisionalRef":"x","streamToken":"y"}}
        ),
      ),
    ))
    is Err(_),
    content="true",
  )
}

///|
test "stream encode contexts isolate registrations" {
  let left = StreamEncodeContext::new()
  let right = StreamEncodeContext::new()
  let left_stream = AgentStream::from_next(async fn() { None })
  let right_stream = AgentStream::from_next(async fn() { None })
  ignore(register_input_stream(left, left_stream, async fn(value) { U8Value(value) }, u8_public_codec()))
  ignore(register_input_stream(right, right_stream, async fn(value) { U8Value(value) }, u8_public_codec()))
  inspect(left.inputs.length(), content="1")
  inspect(right.inputs.length(), content="1")
  inspect(left.inputs[0].id == right.inputs[0].id, content="false")
}

///|
test "duplicate stream registration is an affine error" {
  let context = StreamEncodeContext::new()
  let stream = AgentStream::from_next(async fn() { None })
  ignore(register_input_stream(context, stream, async fn(value) { U8Value(value) }, u8_public_codec()))
  inspect(
    (try? register_input_stream(context, stream, async fn(value) { U8Value(value) }, u8_public_codec()))
    is Err(_),
    content="true",
  )
}

///|
test "strict protocol JSON rejects duplicate members at every depth" {
  inspect(
    (try? parse_strict_json("{\"a\":1,\"a\":2}")) is Err(_),
    content="true",
  )
  inspect(
    (try? parse_strict_json("{\"a\":{\"b\":1,\"\\u0062\":2}}")) is Err(_),
    content="true",
  )
  inspect(
    parse_strict_json("{\"a\":[{\"b\":1},{\"b\":2}]}").stringify(),
    content="{\"a\":[{\"b\":1},{\"b\":2}]}",
  )
}

///|
async test "MemoryReader owns a detached receive actor" {
  let delivered : @aqueue.Queue[Int] = @aqueue.Queue(kind=@aqueue.Blocking(1))
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
  let stream = AgentStream::from_next(async fn() { values.remove(0) })
  let context = StreamEncodeContext::new()
  ignore(
    register_input_stream(
      context,
      stream,
      async fn(value) { U8Value(value) },
      u8_public_codec(),
      wire_kind="u8",
    ),
  )
  let input = context.inputs[0]
  inspect((input.next)() is Some(U8Value(1)), content="true")
  @async.sleep(1)
  let prefetched : Array[Int] = []
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
  inspect(prefetched, content="[2, 3]")
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
