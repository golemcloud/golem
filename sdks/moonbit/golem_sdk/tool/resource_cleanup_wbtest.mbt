///|
fn cleanup_drops() -> @model.SchemaValueResourceDrops {
  {
    drop_secret: fn(_) {  },
    drop_quota_token: fn(_) {  },
    drop_stream: fn(_) {  },
    drop_native_stream: fn(_) {  },
  }
}

///|
fn cleanup_string_graph() -> @model.SchemaGraph raise {
  @schema.schema_graph_of_tag((@schema.type_tag() : @schema.TypeTag[String]))
}

///|
test "parameter failure rolls back an earlier decoded stream" {
  let first_source = @model.GuestSchemaValueStreamHandle::from_wrapped(
    @types.SchemaValueStream::SchemaValueStream(1),
  )
  let second_source = @model.GuestSchemaValueStreamHandle::from_wrapped(
    @types.SchemaValueStream::SchemaValueStream(2),
  )
  let fields = [
    @model.SchemaValue::Stream(first_source),
    @model.SchemaValue::Stream(second_source),
  ]
  let dropped = [0]
  let drops : @model.SchemaValueResourceDrops = {
    drop_secret: fn(_) {  },
    drop_quota_token: fn(_) {  },
    drop_stream: fn(_) { dropped[0] += 1 },
    drop_native_stream: fn(_) {  },
  }
  let first : @schema.AgentStream[Int] = decode_param_with_resource_drops(
    fields, 0, "partial transfer", drops,
  )
  guard @schema.to_value_as(first) is @model.SchemaValue::Stream(first_owned) else {
    fail("expected the first stream to decode")
  }
  assert_true(first_source.is_present())
  assert_true(first_owned.is_present())
  let result : Result[Int, _] = try? decode_param_with_resource_drops(
    fields, 1, "partial transfer", drops,
  )
  guard result is Err(@toolCommon.ToolError::InvalidInput(_)) else {
    fail("decoding the second stream as Int must fail")
  }
  assert_false(first_owned.is_present())
  assert_false(first_source.is_present())
  assert_false(second_source.is_present())
  @model.SchemaValue::Stream(first_owned).release_resources(drops)
  @model.SchemaValue::Record(fields).release_resources(drops)
  assert_eq(dropped[0], 2)
}

///|
test "canonical field rejection releases undecoded streams" {
  let source = @model.GuestSchemaValueStreamHandle::from_wrapped(
    @types.SchemaValueStream::SchemaValueStream(3),
  )
  let fields : Array[CanonicalInputValue] = [
    {
      name: "stream",
      aliases: [],
      type_: cleanup_string_graph(),
      value: @model.SchemaValue::Stream(source),
    },
  ]
  let result : Result[Int, _] = try? decode_canonical_field_with_resource_drops(
    fields,
    "missing",
    "canonical cleanup",
    cleanup_drops(),
  )
  guard result is Err(@toolCommon.ToolError::InvalidInput(_)) else {
    fail("a missing canonical field must fail")
  }
  assert_false(source.is_present())
}

///|
test "subtool routing rejection releases unforwarded streams" {
  run_async_test(() => {
    let source = @model.GuestSchemaValueStreamHandle::from_wrapped(
      @types.SchemaValueStream::SchemaValueStream(4),
    )
    let result = invoke_registered_subtool_with_resource_drops(
      "missing-cleanup-tool",
      [],
      [
        {
          name: "stream",
          aliases: [],
          type_: cleanup_string_graph(),
          value: @model.SchemaValue::Stream(source),
        },
      ],
      None,
      @agentCommon.Principal::Anonymous,
      cleanup_drops(),
    )
    guard result is Err(@toolCommon.ToolError::InvalidToolName(_)) else {
      fail("an unregistered subtool must fail")
    }
    assert_false(source.is_present())
  })
}

///|
test "top-level routing rejection drops wire stream resources" {
  run_async_test(() => {
    let dropped = [0]
    let drops : @model.SchemaValueResourceDrops = {
      drop_secret: fn(_) {  },
      drop_quota_token: fn(_) {  },
      drop_stream: fn(_) { dropped[0] += 1 },
      drop_native_stream: fn(_) {  },
    }
    let input : @types.TypedSchemaValue = {
      graph: @model.schema_graph_to_wit(cleanup_string_graph()),
      value: {
        value_nodes: [
          @types.StreamValue(@types.SchemaValueStream::SchemaValueStream(14)),
        ],
        root: 0,
      },
    }
    let result = invoke_registered_tool_with_resource_drops(
      "missing-wire-cleanup-tool",
      [],
      input,
      None,
      @agentCommon.Principal::Anonymous,
      drops,
    )
    guard result is Err(@toolCommon.ToolError::InvalidToolName(_)) else {
      fail("an unregistered top-level tool must fail")
    }
    assert_eq(dropped[0], 1)
  })
}

///|
test "subtool with no invoker drops source before forwarding" {
  run_async_test(() => {
    let base = canonical_test_def()
    let command = base.commands[0]
    let def : ToolDef = {
      version: base.version,
      commands: [
        {
          name: "cleanup-no-invoker",
          aliases: command.aliases,
          doc: command.doc,
          globals: command.globals,
          subcommands: [],
          body: command.body,
        },
      ],
    }
    register_tool_def(def)
    let source = @model.GuestSchemaValueStreamHandle::from_wrapped(
      @types.SchemaValueStream::SchemaValueStream(15),
    )
    let dropped = [0]
    let drops : @model.SchemaValueResourceDrops = {
      drop_secret: fn(_) {  },
      drop_quota_token: fn(_) {  },
      drop_stream: fn(_) { dropped[0] += 1 },
      drop_native_stream: fn(_) {  },
    }
    let result = invoke_registered_subtool_with_resource_drops(
      "cleanup-no-invoker",
      [],
      [
        {
          name: "global",
          aliases: [],
          type_: cleanup_string_graph(),
          value: @model.SchemaValue::Stream(source),
        },
      ],
      None,
      @agentCommon.Principal::Anonymous,
      drops,
    )
    guard result is Err(@toolCommon.ToolError::InvalidToolName(_)) else {
      fail("a subtool without an invoker must fail")
    }
    assert_false(source.is_present())
    assert_eq(dropped[0], 1)
  })
}

///|
test "tool result conversion failure releases decoded streams" {
  let source = @model.GuestSchemaValueStreamHandle::from_wrapped(
    @types.SchemaValueStream::SchemaValueStream(5),
  )
  let result : Result[Int, ToolError[NoToolError]] = decode_result_value_with_resource_drops(
    {
      result: Some({
        graph: cleanup_string_graph(),
        value: @model.SchemaValue::Stream(source),
      }),
      stdout: None,
    },
    cleanup_drops(),
  )
  guard result is Err(ToolError::Rpc(RpcError::Protocol(_))) else {
    fail("decoding a stream result as Int must fail")
  }
  assert_false(source.is_present())
}

///|
test "tool result validates stdout before transferring its value" {
  let source = @model.GuestSchemaValueStreamHandle::from_wrapped(
    @types.SchemaValueStream::SchemaValueStream(6),
  )
  let result : Result[
    (@schema.AgentStream[Int], @async-core.Stream[Byte]),
    ToolError[NoToolError],
  ] = decode_result_with_stdout_with_resource_drops(
    {
      result: Some({
        graph: cleanup_string_graph(),
        value: @model.SchemaValue::Stream(source),
      }),
      stdout: None,
    },
    cleanup_drops(),
  )
  guard result is Err(ToolError::Rpc(RpcError::Protocol(_))) else {
    fail("a missing stdout stream must fail")
  }
  assert_false(source.is_present())
}

///|
test "tool input encoding failure drops value streams and stdin" {
  run_async_test(() => {
    let (value_reader, value_sink) = @async-core.Stream::new()
    let value_source = @model.GuestSchemaValueStreamHandle::from_native(
      value_reader,
    )
    let (stdin, stdin_sink) = @async-core.Stream::new()
    let invalid_graph : @model.SchemaGraph = {
      defs: [],
      root: @model.schema_type(@model.SchemaTypeBody::Ref("missing-type")),
    }
    let result = invoke_and_await_no_tool_error(
      @toolHost.ToolRpc::ToolRpc(0),
      [],
      { graph: invalid_graph, value: @model.SchemaValue::Stream(value_source) },
      Some(stdin),
    )
    guard result is Err(ToolError::Rpc(RpcError::Protocol(_))) else {
      fail("an invalid input graph must fail before invoking the host")
    }
    assert_false(value_source.is_present())
    assert_false(value_sink.is_open())
    assert_false(stdin_sink.is_open())
  })
}
