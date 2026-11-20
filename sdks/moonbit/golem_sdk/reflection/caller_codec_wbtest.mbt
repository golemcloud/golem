///|
fn custom_rpc_error(value : String) -> ReflectError raise {
  let typed = @schema.try_into_typed_schema_value(value)
  rpc_error(
    @agentHost.RpcError::RemoteAgentError(
      @common.AgentError::CustomError(@model.typed_schema_value_to_wit(typed)),
    ),
  )
}

///|
test "agent RPC errors preserve variants and distinct custom payloads" {
  assert_eq(
    rpc_error(@agentHost.RpcError::ProtocolError("bad frame")),
    ReflectError::Remote(AgentRpcError::Protocol("bad frame")),
  )
  assert_eq(
    rpc_error(@agentHost.RpcError::Denied("no access")),
    ReflectError::Remote(AgentRpcError::Denied("no access")),
  )
  assert_eq(
    rpc_error(@agentHost.RpcError::NotFound("no agent")),
    ReflectError::Remote(AgentRpcError::NotFound("no agent")),
  )
  assert_eq(
    rpc_error(@agentHost.RpcError::Conflict("the key is current")),
    ReflectError::Remote(AgentRpcError::Conflict("the key is current")),
  )
  let first = custom_rpc_error("first")
  let second = custom_rpc_error("second")
  assert_true(first != second)
  match first {
    Remote(Custom(payload)) =>
      assert_eq(payload.value, @model.SchemaValue::String("first"))
    error => fail("unexpected custom RPC error: \{repr(error)}")
  }
}

///|
test "reflected output rejection releases stale and unexpected streams" {
  let string = SchemaRef::new({
    defs: [],
    root: @model.schema_type(@model.SchemaTypeBody::String),
  })
  let entry : AgentMethod = {
    name: "read",
    description: "",
    prompt_hint: None,
    input: SchemaRef::new({
      defs: [],
      root: @model.schema_type(@model.SchemaTypeBody::Record([])),
    }),
    output: Some(string),
  }
  let invocation = fn(id : Int32) -> Invocation[@model.SchemaValue?] {
    {
      metadata: {
        agent_id: @schema.AgentId::from_uuid(@schema.Uuid::from_bits(0, 2)),
        invocation_id: @rpc.InvocationId::new(
          @schema.AgentId::from_uuid(@schema.Uuid::from_bits(0, 2)),
          @schema.Uuid::from_bits(0, 1),
        ),
      },
      value: Some(
        @model.SchemaValue::Stream(
          @model.GuestSchemaValueStreamHandle::from_wrapped(
            @types.SchemaValueStream::SchemaValueStream(id),
          ),
        ),
      ),
    }
  }
  reset_reflection_resource_drop_counts()
  try validate_invocation_output(entry, invocation(301)) catch {
    Schema(_) => ()
    error => fail("unexpected stale-output failure: \{repr(error)}")
  } noraise {
    _ => fail("stale output unexpectedly validated")
  }
  assert_eq(reflection_resource_drop_count(3), 1)
  let unit_entry = { ..entry, output: None, }
  try validate_invocation_output(unit_entry, invocation(302)) catch {
    Schema(_) => ()
    error => fail("unexpected unit-output failure: \{repr(error)}")
  } noraise {
    _ => fail("unexpected unit output was accepted")
  }
  assert_eq(reflection_resource_drop_count(3), 2)
}

///|
test "awaited input encoding transfers a locally produced schema stream" {
  run_reflection_tool_async(async fn() {
    let (writer, stream) : (
      @schema.AgentStreamWriter[String],
      @schema.AgentStream[String],
    ) = @schema.AgentStream::new()
    writer.close()
    let input = @model.SchemaValue::Record([@schema.to_value_as(stream)])
    let wire = encode_awaited_input(input)
    let decoded = @model_host.schema_value_from_wit(wire)
    decoded.release_resources(@model_host.resource_drops())
  })
}

///|
test "caller-defined output rejection releases resources" {
  let stream_value = fn(id : Int32) -> @model.SchemaValue {
    @model.SchemaValue::Stream(
      @model.GuestSchemaValueStreamHandle::from_wrapped(
        @types.SchemaValueStream::SchemaValueStream(id),
      ),
    )
  }
  reset_reflection_resource_drop_counts()
  try (decode_typed_output(stream_value(401)) : String) catch {
    Schema(_) => ()
    error => fail("unexpected typed decode failure: \{repr(error)}")
  } noraise {
    _ => fail("stream unexpectedly decoded as String")
  }
  assert_eq(reflection_resource_drop_count(3), 1)
  try require_unit_output(Some(stream_value(402))) catch {
    Schema(_) => ()
    error => fail("unexpected unit decode failure: \{repr(error)}")
  } noraise {
    _ => fail("unit decoder accepted an unexpected value")
  }
  assert_eq(reflection_resource_drop_count(3), 2)
}
