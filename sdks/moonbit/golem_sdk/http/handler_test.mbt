///|
fn run_http_async(body : async () -> Unit) -> Unit raise {
  let mut callback = @asyncCore.with_waitableset(async fn() {
    body()
    @asyncCore.task_returned()
  })
  let mut steps = 0
  while callback != 0 {
    steps += 1
    if steps > 1000 || (callback & 0xf) != 1 {
      fail("HTTP test stalled")
    }
    callback = @asyncCore.cb(0, 0, 0)
  }
}

///|
test "H01 canonical schemas use list u8 and u16 rather than binary or u32" {
  let request = @schema.into_schema_graph(
    (@schema.type_tag() : @schema.TypeTag[@http.HttpRequest]),
  )
  guard request.root.body is @model.Record(fields) else {
    fail("request not a record")
  }
  assert_eq(fields.map(f => f.name), [
    "method", "scheme", "authority", "path", "query", "headers", "body",
  ])
  assert_eq(
    fields[6].body.body,
    @model.Stream(
      Some(@model.schema_type(@model.List(@model.schema_type(@model.U8(None))))),
    ),
  )
  let response = @schema.into_schema_graph(
    (@schema.type_tag() : @schema.TypeTag[@http.HttpResponse]),
  )
  guard response.root.body is @model.Record(fields) else {
    fail("response not a record")
  }
  assert_eq(fields.map(f => f.name), ["status", "headers", "body"])
  assert_eq(fields[0].body.body, @model.U16(None))
  let max : UInt16 = 65535
  assert_eq(@schema.to_value_as(max), @model.U16(65535))
  let decoded : UInt16 = @schema.from_value_as(@model.U16(65535))
  assert_eq(decoded, max)
  try {
    let _ : UInt16 = @schema.from_value_as(@model.U16(65536))
  } catch {
    @schema.FromSchemaError::Custom(_) => ()
    _ => fail("wrong u16 overflow category")
  } noraise {
    _ => fail("u16 overflow accepted")
  }
}

///|
test "envelope-extension-and-bytes and envelope-cookie-order survive codecs" {
  run_http_async(async fn() {
    let (writer, body) : (
      @schema.AgentStreamWriter[Array[Byte]],
      @schema.AgentStream[Array[Byte]],
    ) = @schema.AgentStream::new()
    let request : @http.HttpRequest = {
      method: "MiXeD",
      scheme: "https",
      authority: "example.com:8443",
      path: "/site/%252f/",
      query: Some(""),
      headers: [
        { name: "cookie", value: [0x80, 0xff], },
        { name: "cookie", value: [0x61], },
      ],
      body,
    }
    let decoded : @http.HttpRequest = @schema.from_value_as(
      @schema.to_value_as(request),
    )
    assert_eq(decoded.method, "MiXeD")
    assert_eq(decoded.path, "/site/%252f/")
    assert_eq(decoded.query, Some(""))
    assert_true(decoded.headers == request.headers)
    @asyncCore.with_task_group(async fn(group) {
      let producer = group.spawn(async fn() {
        assert_true(writer.write_one([]) is @schema.Accepted)
        assert_true(writer.write_one([0xff, 0, 0x81]) is @schema.Accepted)
        writer.close()
      })
      assert_eq(decoded.body.read(), Some([]))
      assert_eq(decoded.body.read(), Some([0xff, 0, 0x81]))
      assert_eq(decoded.body.read(), None)
      producer.wait()
    })
  })
}

///|
test "lifecycle-output-backpressure waits for body demand and propagates cancellation" {
  run_http_async(async fn() {
    let (writer, body) : (
      @schema.AgentStreamWriter[Array[Byte]],
      @schema.AgentStream[Array[Byte]],
    ) = @schema.AgentStream::new()
    let response : @http.HttpResponse = { status: 200, headers: [], body, }
    let response : @http.HttpResponse = @schema.from_value_as(
      @schema.to_value_as(response),
    )
    let (started, notify) = @asyncCore.Future::new()
    let completed = Ref(false)
    @asyncCore.with_task_group(async fn(group) {
      let producer = group.spawn(async fn() {
        ignore(notify.complete(()))
        let _ = writer.write_one([0x42])
        completed.val = true
      })
      started.get()
      assert_false(completed.val)
      producer.cancel()
      try producer.wait() catch {
        @asyncCore.Cancelled::Cancelled => ()
        _ => fail("cancellation changed category")
      } noraise {
        _ => fail("cancelled writer succeeded")
      }
      assert_false(completed.val)
    })
    response.body.drop()
  })
}

///|
test "envelope-head-unpolled disposal releases unstarted producer exactly once" {
  let started = Ref(false)
  let drops = Ref(0)
  let body : @schema.AgentStream[Array[Byte]] = @schema.AgentStream::produce(
    async fn(_) { started.val = true },
    on_unstarted_drop=() => drops.val += 1,
  )
  let response : @http.HttpResponse = { status: 204, headers: [], body, }
  let decoded : @http.HttpResponse = @schema.from_value_as(
    @schema.to_value_as(response),
  )
  decoded.body.drop()
  decoded.body.drop()
  assert_false(started.val)
  assert_eq(drops.val, 1)
}

///|
test "OpenAPI JSON canonical ordering preserves opaque examples and arrays" {
  let document : Json = {
    "z": [2, 1],
    "a": { "$ref": "not a reference here", "example": "é" },
  }
  assert_eq(
    @http.openapi_json(document),
    "{\"a\":{\"$ref\":\"not a reference here\",\"example\":\"é\"},\"z\":[2,1]}",
  )
}

///|
test "OpenAPI JSON byte and depth limits are inclusive" {
  let text = "a".repeat(1048568)
  assert_eq(
    @utf8.encode(@http.openapi_json({ "x": text.to_json() })).length(),
    1048576,
  )
  try @http.openapi_json({ "x": (text + "é").to_json() }) catch {
    @http.OpenApiJsonError("provider-size") => ()
    _ => fail("wrong byte-limit error")
  } noraise {
    _ => fail("oversize JSON accepted")
  }
  let mut nested : Json = null
  for _ in 0..<63 {
    nested = { "x": nested }
  }
  let _ = @http.openapi_json({ "x": nested })
  nested = { "x": nested }
  try @http.openapi_json({ "x": nested }) catch {
    @http.OpenApiJsonError("json-depth") => ()
    _ => fail("wrong depth error")
  } noraise {
    _ => fail("overdeep JSON accepted")
  }
}

///|
test "OpenAPI numbers preserve precision without trusting raw representations" {
  for token in ["0,\"x\":1", "[]", "01", "+1", "1.", "1e", "--1", " 0", "0 "] {
    try @http.openapi_json({ "x": Json::number(0.0, repr=token) }) catch {
      @http.OpenApiJsonError("invalid-number") => ()
      _ => fail("wrong number error")
    } noraise {
      _ => fail("invalid numeric representation accepted: " + token)
    }
  }
  for token in ["9007199254740993", "-0", "-1.25e+4", "0.01E-2"] {
    assert_eq(
      @http.openapi_json({ "x": Json::number(0.0, repr=token) }),
      "{\"x\":" + token + "}",
    )
  }
  try @http.openapi_json({ "x": Json::number(1.0 / 0.0) }) catch {
    @http.OpenApiJsonError("invalid-number") => ()
    _ => fail("wrong non-finite error")
  } noraise {
    _ => fail("infinity accepted")
  }
}
