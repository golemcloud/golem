///|
test "tool invoke error mapping preserves every protocol variant" {
  let errors : Array[ToolInvokeError[Int]] = [
    InvalidToolName("missing"),
    InvalidCommandPath(["nested", "command"]),
    InvalidInput("input"),
    ConstraintViolation("constraint"),
    InvalidResult("result"),
  ]
  for error in errors {
    let mapped : ToolInvokeError[String] = error.map_tool(value => {
      value.to_string()
    })
    match mapped {
      Tool(_) => fail("protocol error was mapped as a custom tool error")
      _ => ()
    }
  }
  match ToolInvokeError::Tool(42).map_tool(value => value.to_string()) {
    Tool("42") => ()
    _ => fail("custom tool error was not mapped")
  }
}

///|
test "wire protocol errors retain their exact middleware variants" {
  let decode = fn(error) {
    decode_tool_error(error, (_ : @model.TypedSchemaValue) => Ok(0))
  }
  assert_true(
    decode(@toolCommon.ToolError::InvalidToolName("missing"))
    is InvalidToolName("missing"),
  )
  assert_true(
    decode(@toolCommon.ToolError::InvalidCommandPath(["a", "b"]))
    is InvalidCommandPath(["a", "b"]),
  )
  assert_true(
    decode(@toolCommon.ToolError::InvalidInput("bad")) is InvalidInput("bad"),
  )
  assert_true(
    decode(@toolCommon.ToolError::ConstraintViolation("denied"))
    is ConstraintViolation("denied"),
  )
  assert_true(
    decode(@toolCommon.ToolError::InvalidResult("bad result"))
    is InvalidResult("bad result"),
  )
}

///|
test "underlying copies share overlap revocation and exactly-once drop state" {
  let drops : Ref[Int] = Ref(0)
  let underlying = new_underlying_tool(
    (_, _, _) => Err(@toolCommon.ToolError::InvalidInput("unused")),
    fn() { drops.val = drops.val + 1 },
  )
  let copy = underlying
  let started : Result[Unit, ToolInvokeError[Unit]] = underlying.ownership.begin_call()
  assert_true(started is Ok(_))
  let overlap : Result[Unit, ToolInvokeError[Unit]] = copy.ownership.begin_call()
  assert_true(overlap is Err(InvalidInput(_)))
  copy.revoke()
  assert_eq(drops.val, 0)
  underlying.ownership.finish_call()
  assert_eq(drops.val, 1)
  copy.revoke()
  assert_eq(drops.val, 1)
  let after_return : Result[Unit, ToolInvokeError[Unit]] = underlying.ownership.begin_call()
  assert_true(after_return is Err(InvalidInput(_)))
}

///|
fn counted_stream(drops : Ref[Int]) -> @async-core.Stream[Byte] {
  @async-core.Stream::produce(_ => (), on_unstarted_drop=fn() {
    drops.val = drops.val + 1
  })
}

///|
fn no_raw_drop() -> Unit {
  ()
}

///|
fn run_component_async(body : async () -> Unit) -> Unit raise {
  let mut callback = @async-core.with_waitableset(async fn() {
    body()
    @async-core.task_returned()
  })
  let mut steps = 0
  while callback != 0 {
    steps = steps + 1
    if steps > 1000 {
      fail("component async test did not complete")
    }
    if (callback & 0xf) != 1 {
      fail("component async test unexpectedly waited on a host event")
    }
    callback = @async-core.cb(0, 0, 0)
  }
}

///|
priv suberror FakeBackendFailure derive(Debug)

///|
extern "wasm" fn reset_resource_drop_counts() = "golem:test" "reset-resource-drop-counts"

///|
extern "wasm" fn resource_drop_count(kind : Int) -> Int = "golem:test" "resource-drop-count"

///|
fn assert_resource_drop_counts(
  secret : Int,
  quota_token : Int,
  permission_card : Int,
) -> Unit raise {
  assert_eq(resource_drop_count(0), secret)
  assert_eq(resource_drop_count(1), quota_token)
  assert_eq(resource_drop_count(2), permission_card)
}

///|
test "resource acquisitions make exact terminal transitions" {
  let raw_drops = Ref(0)
  let resource_drops = Ref(0)
  let context = OwnershipContext::new(fn() { raw_drops.val = raw_drops.val + 1 })
  let underlying = context.acquire_resources([
    resource_candidate(Secret, 1, fn() {
      resource_drops.val = resource_drops.val + 1
    }),
  ])
  let returned = context.acquire_resources([
    resource_candidate(QuotaToken, 2, fn() {
      resource_drops.val = resource_drops.val + 1
    }),
  ])
  let abandoned = context.acquire_resources([
    resource_candidate(PermissionCard, 3, fn() {
      resource_drops.val = resource_drops.val + 1
    }),
  ])
  assert_true(underlying.transfer_underlying() is Ok(_))
  assert_true(returned.transfer_final() is Ok(_))
  assert_true(abandoned.drop() is Ok(_))
  assert_eq(resource_drops.val, 1)
  assert_true(underlying.transfer_underlying() is Err(_))
  assert_true(returned.drop() is Err(_))
  assert_true(abandoned.drop() is Err(_))
  assert_eq(resource_drops.val, 1)
  context.revoke()
  assert_eq(resource_drops.val, 1)
  assert_eq(raw_drops.val, 1)
  context.revoke()
  assert_eq(raw_drops.val, 1)
}

///|
test "recursive owned capability cleanup canonicalizes duplicate raw handles" {
  reset_resource_drop_counts()
  let shared_secret = @model.GuestSecretHandle::new(@types.Secret::Secret(101))
  let duplicate_secret = @model.GuestSecretHandle::new(
    @types.Secret::Secret(101),
  )
  let quota_token = @model.GuestQuotaTokenHandle::new(
    @types.QuotaToken::QuotaToken(102),
  )
  let permission_card = @model.GuestPermissionCardHandle::new(
    @types.PermissionCard::PermissionCard(103),
  )
  let recursive = @model.SchemaValue::Tuple([
    @model.SchemaValue::Secret(shared_secret),
    @model.SchemaValue::List([
      @model.SchemaValue::Secret(shared_secret),
      @model.SchemaValue::QuotaToken(quota_token),
    ]),
    @model.SchemaValue::Map([
      {
        key: @model.SchemaValue::String("capability"),
        value: @model.SchemaValue::PermissionCard(permission_card),
      },
    ]),
    @model.SchemaValue::Secret(duplicate_secret),
  ])
  @model.drop_owned_capabilities(recursive)
  assert_resource_drop_counts(1, 1, 1)
  @model.drop_owned_capabilities(recursive)
  assert_resource_drop_counts(1, 1, 1)
}

///|
test "malformed wire custom error drops each distinct capability exactly once" {
  reset_resource_drop_counts()
  let malformed : @types.TypedSchemaValue = {
    graph: { type_nodes: [], defs: [], root: 0 },
    value: {
      value_nodes: [
        @types.SecretValue(@types.Secret::Secret(201)),
        @types.SecretValue(@types.Secret::Secret(201)),
        @types.QuotaTokenHandle(@types.QuotaToken::QuotaToken(202)),
        @types.PermissionCardHandle(@types.PermissionCard::PermissionCard(203)),
        @types.TupleValue([0, 1, 2, 3]),
      ],
      root: 4,
    },
  }
  let decoded : ToolInvokeError[Unit] = decode_tool_error(
    @toolCommon.ToolError::CustomError(malformed),
    _ => Ok(()),
  )
  assert_true(decoded is InvalidResult(_))
  assert_resource_drop_counts(1, 1, 1)
}

///|
test "rejected custom error decoder drops lifted capabilities" {
  reset_resource_drop_counts()
  let unit = @schema.try_into_typed_schema_value(())
  let encoded = @model.typed_schema_value_to_wit({
    graph: unit.graph,
    value: @model.SchemaValue::Secret(
      @model.GuestSecretHandle::new(@types.Secret::Secret(301)),
    ),
  })
  let decoded : ToolInvokeError[Unit] = decode_tool_error(
    @toolCommon.ToolError::CustomError(encoded),
    _ => Err("custom error payload rejected"),
  )
  assert_true(decoded is InvalidResult("custom error payload rejected"))
  assert_resource_drop_counts(1, 0, 0)
}

///|
test "successful custom error decode remains owned until invocation teardown" {
  run_component_async(async fn() {
    reset_resource_drop_counts()
    let underlying = new_underlying_tool(
      (_, _, _) => {
        Err(@toolCommon.ToolError::CustomError(raw_secret_value(351)))
      },
      no_raw_drop,
    )
    let input = @schema.try_into_typed_schema_value(())
    let result = underlying.invoke_with([], input, None, value => Ok(value))
    guard result is Err(Tool(decoded)) else {
      fail("backend custom error did not decode successfully")
    }
    assert_resource_drop_counts(0, 0, 0)
    underlying.revoke()
    assert_resource_drop_counts(1, 0, 0)
    @model.drop_owned_capabilities(decoded.value)
    assert_resource_drop_counts(1, 0, 0)
  })
}

///|
test "failed custom error encoding drops untransferred capabilities" {
  reset_resource_drop_counts()
  let unit = @schema.try_into_typed_schema_value(())
  let malformed : @model.TypedSchemaValue = {
    graph: unit.graph,
    value: @model.SchemaValue::Tuple([
      @model.SchemaValue::PermissionCard(
        @model.GuestPermissionCardHandle::new(
          @types.PermissionCard::PermissionCard(401),
        ),
      ),
      @model.SchemaValue::S8(128),
    ]),
  }
  let encoded = encode_tool_error(Tool(malformed))
  assert_true(encoded is @toolCommon.ToolError::InvalidResult(_))
  assert_resource_drop_counts(0, 0, 1)
}

///|
fn raw_unit_value() -> @types.TypedSchemaValue raise {
  @model.typed_schema_value_to_wit(@schema.try_into_typed_schema_value(()))
}

///|
fn raw_secret_value(index : Int) -> @types.TypedSchemaValue raise {
  let unit = @schema.try_into_typed_schema_value(())
  @model.typed_schema_value_to_wit({
    graph: unit.graph,
    value: @model.SchemaValue::Secret(
      @model.GuestSecretHandle::new(@types.Secret::Secret(index)),
    ),
  })
}

///|
fn drop_raw_value_resources(value : @types.TypedSchemaValue) -> Unit {
  for node in value.value.value_nodes {
    match node {
      @types.SecretValue(secret) => secret.drop()
      @types.QuotaTokenHandle(token) => token.drop()
      @types.PermissionCardHandle(card) => card.drop()
      _ => ()
    }
  }
}

///|
test "raw typed values share take-once state even without resources" {
  let context = OwnershipContext::new(no_raw_drop)
  let raw = context.own_raw_typed_schema_value(raw_unit_value())
  let copy = raw
  assert_true(raw.take_for_final(context) is Ok(_))
  assert_true(copy.take_for_final(context) is Err(_))
  assert_true(copy.drop() is Err(_))
  context.revoke()
}

///|
test "raw resource values transfer exact acquisitions and reject duplicates" {
  reset_resource_drop_counts()
  let context = OwnershipContext::new(no_raw_drop)
  let raw = context.own_raw_typed_schema_value(raw_secret_value(601))
  let transferred = raw.take_for_final(context)
  assert_true(transferred is Ok(_))
  context.revoke()
  assert_resource_drop_counts(0, 0, 0)
  match transferred {
    Ok(value) => drop_raw_value_resources(value)
    Err(_) => fail("raw resource transfer unexpectedly failed")
  }
  assert_resource_drop_counts(1, 0, 0)

  reset_resource_drop_counts()
  let malformed_context = OwnershipContext::new(no_raw_drop)
  let malformed : @types.TypedSchemaValue = {
    graph: { type_nodes: [], defs: [], root: 0 },
    value: {
      value_nodes: [
        @types.SecretValue(@types.Secret::Secret(602)),
        @types.SecretValue(@types.Secret::Secret(602)),
      ],
      root: 0,
    },
  }
  let duplicate = malformed_context.own_raw_typed_schema_value(malformed)
  assert_true(duplicate.take_for_underlying(malformed_context) is Err(_))
  malformed_context.revoke()
  assert_resource_drop_counts(1, 0, 0)
  malformed_context.revoke()
  assert_resource_drop_counts(1, 0, 0)
}

///|
test "local typed decode transfers ownership to context-managed model values" {
  reset_resource_drop_counts()
  let context = OwnershipContext::new(no_raw_drop)
  let raw = context.own_raw_typed_schema_value(raw_secret_value(610))
  let copy = raw
  let decoded = raw.decode_model_for_local(context)
  assert_true(decoded is Ok(_))
  assert_true(copy.take_for_final(context) is Err(_))
  assert_resource_drop_counts(0, 0, 0)
  context.revoke()
  assert_resource_drop_counts(1, 0, 0)
  context.revoke()
  assert_resource_drop_counts(1, 0, 0)
}

///|
test "failed local typed decode drops raw capabilities exactly once" {
  reset_resource_drop_counts()
  let context = OwnershipContext::new(no_raw_drop)
  let malformed : @types.TypedSchemaValue = {
    graph: { type_nodes: [], defs: [], root: 0 },
    value: {
      value_nodes: [@types.SecretValue(@types.Secret::Secret(611))],
      root: 1,
    },
  }
  let raw = context.own_raw_typed_schema_value(malformed)
  assert_true(raw.decode_model_for_local(context) is Err(_))
  assert_resource_drop_counts(1, 0, 0)
  context.revoke()
  assert_resource_drop_counts(1, 0, 0)
}

///|
test "raw invocation results transfer result and stdout together once" {
  let stdout_drops = Ref(0)
  let context = OwnershipContext::new(no_raw_drop)
  let result = context.own_raw_invocation_result({
    result: Some(raw_unit_value()),
    stdout: Some(counted_stream(stdout_drops)),
  })
  let copy = result
  let transferred = result.take_for_final(context)
  assert_true(transferred is Ok(_))
  assert_true(copy.take_for_final(context) is Err(_))
  assert_true(copy.drop() is Err(_))
  context.revoke()
  assert_eq(stdout_drops.val, 0)
  match transferred {
    Ok(result) => {
      assert_true(result.result is Some(_))
      match result.stdout {
        Some(stdout) => stdout.drop_sync()
        None => fail("raw result lost stdout during final transfer")
      }
    }
    Err(_) => fail("raw result transfer unexpectedly failed")
  }
  assert_eq(stdout_drops.val, 1)
}

///|
test "raw custom errors lower once and preserve resource ownership" {
  reset_resource_drop_counts()
  let context = OwnershipContext::new(no_raw_drop)
  let payload = context.own_raw_typed_schema_value(raw_secret_value(603))
  let lowered = encode_raw_tool_error(Tool(payload), context)
  match lowered {
    @toolCommon.ToolError::CustomError(value) => drop_raw_value_resources(value)
    _ => fail("raw custom error was not preserved")
  }
  assert_resource_drop_counts(1, 0, 0)
  assert_true(payload.take_for_final(context) is Err(_))
  context.revoke()
  assert_resource_drop_counts(1, 0, 0)
}

///|
test "raw final lowering rejects foreign results and errors without consuming them" {
  reset_resource_drop_counts()
  let expected_context = OwnershipContext::new(no_raw_drop)
  let foreign_context = OwnershipContext::new(no_raw_drop)
  let stdout_drops = Ref(0)
  let result = foreign_context.own_raw_invocation_result({
    result: Some(raw_secret_value(607)),
    stdout: Some(counted_stream(stdout_drops)),
  })
  assert_true(result.take_for_final(expected_context) is Err(_))
  assert_resource_drop_counts(0, 0, 0)
  assert_eq(stdout_drops.val, 0)
  match result.take_for_final(foreign_context) {
    Ok(result) => {
      match result.result {
        Some(value) => drop_raw_value_resources(value)
        None => fail("foreign result was consumed on rejection")
      }
      match result.stdout {
        Some(stdout) => stdout.drop_sync()
        None => fail("foreign stdout was consumed on rejection")
      }
    }
    Err(_) => fail("foreign result was consumed on rejection")
  }
  assert_resource_drop_counts(1, 0, 0)
  assert_eq(stdout_drops.val, 1)

  let payload = foreign_context.own_raw_typed_schema_value(
    raw_secret_value(608),
  )
  assert_true(
    encode_raw_tool_error(Tool(payload), expected_context)
    is @toolCommon.ToolError::InvalidResult(_),
  )
  assert_resource_drop_counts(1, 0, 0)
  match payload.take_for_final(foreign_context) {
    Ok(value) => drop_raw_value_resources(value)
    Err(_) => fail("foreign custom error was consumed on rejection")
  }
  assert_resource_drop_counts(2, 0, 0)
  expected_context.revoke()
  foreign_context.revoke()
  assert_resource_drop_counts(2, 0, 0)
}

///|
test "failed raw result transfer retains nested resources and stdout for teardown" {
  reset_resource_drop_counts()
  let context = OwnershipContext::new(no_raw_drop)
  let stdout_drops = Ref(0)
  let malformed : @types.TypedSchemaValue = {
    graph: { type_nodes: [], defs: [], root: 0 },
    value: {
      value_nodes: [
        @types.SecretValue(@types.Secret::Secret(609)),
        @types.SecretValue(@types.Secret::Secret(609)),
      ],
      root: 0,
    },
  }
  let result = context.own_raw_invocation_result({
    result: Some(malformed),
    stdout: Some(counted_stream(stdout_drops)),
  })
  assert_true(result.take_for_final(context) is Err(_))
  assert_resource_drop_counts(0, 0, 0)
  assert_eq(stdout_drops.val, 0)
  context.revoke()
  assert_resource_drop_counts(1, 0, 0)
  assert_eq(stdout_drops.val, 1)
  context.revoke()
  assert_resource_drop_counts(1, 0, 0)
  assert_eq(stdout_drops.val, 1)
}

///|
test "raw invoke rejects foreign input without consuming it" {
  run_component_async(async fn() {
    reset_resource_drop_counts()
    let backend_calls = Ref(0)
    let underlying = new_underlying_tool(
      (_, _, _) => {
        backend_calls.val = backend_calls.val + 1
        Ok({ result: None, stdout: None })
      },
      no_raw_drop,
    )
    let foreign_context = OwnershipContext::new(no_raw_drop)
    let input = foreign_context.own_raw_typed_schema_value(
      raw_secret_value(604),
    )
    let result = underlying.invoke_raw([], input, None)
    assert_true(result is Err(InvalidInput(_)))
    assert_eq(backend_calls.val, 0)
    let returned = input.take_for_final(foreign_context)
    assert_true(returned is Ok(_))
    match returned {
      Ok(value) => drop_raw_value_resources(value)
      Err(_) => fail("foreign input was consumed on rejection")
    }
    assert_resource_drop_counts(1, 0, 0)
    foreign_context.revoke()
    underlying.revoke()
    assert_resource_drop_counts(1, 0, 0)
  })
}

///|
test "raw invoke preflights stdin before transferring input" {
  run_component_async(async fn() {
    reset_resource_drop_counts()
    let backend_calls = Ref(0)
    let underlying = new_underlying_tool(
      (_, _, _) => {
        backend_calls.val = backend_calls.val + 1
        Ok({ result: None, stdout: None })
      },
      no_raw_drop,
    )
    let stdin_drops = Ref(0)
    let stale = underlying.ownership.own_stdin(counted_stream(stdin_drops))
    let transferred_stdin = stale.take_for_underlying()
    assert_true(transferred_stdin is Ok(_))
    let input = underlying.ownership.own_raw_typed_schema_value(
      raw_secret_value(605),
    )
    let result = underlying.invoke_raw([], input, Some(stale))
    assert_true(result is Err(InvalidInput(_)))
    assert_eq(backend_calls.val, 0)
    let returned = input.take_for_final(underlying.ownership)
    assert_true(returned is Ok(_))
    match returned {
      Ok(value) => drop_raw_value_resources(value)
      Err(_) => fail("raw input was consumed before stdin rejection")
    }
    assert_resource_drop_counts(1, 0, 0)
    match transferred_stdin {
      Ok(stream) => stream.drop_sync()
      Err(_) => fail("initial stdin transfer unexpectedly failed")
    }
    assert_eq(stdin_drops.val, 1)
    underlying.revoke()
  })
}

///|
test "raw invoke supports sequential calls and raw backend errors" {
  run_component_async(async fn() {
    reset_resource_drop_counts()
    let calls = Ref(0)
    let underlying = new_underlying_tool(
      (_, _, _) => {
        calls.val = calls.val + 1
        if calls.val == 3 {
          Err(@toolCommon.ToolError::CustomError(raw_secret_value(606)))
        } else {
          Ok({ result: Some(raw_unit_value()), stdout: None })
        }
      },
      no_raw_drop,
    )
    for index = 0; index < 2; index = index + 1 {
      ignore(index)
      let input = underlying.ownership.own_raw_typed_schema_value(
        raw_unit_value(),
      )
      match underlying.invoke_raw([], input, None) {
        Ok(result) => assert_true(result.drop() is Ok(_))
        Err(_) => fail("sequential raw invocation unexpectedly failed")
      }
    }
    let input = underlying.ownership.own_raw_typed_schema_value(
      raw_unit_value(),
    )
    match underlying.invoke_raw([], input, None) {
      Err(Tool(payload)) =>
        match encode_raw_tool_error(Tool(payload), underlying.ownership) {
          @toolCommon.ToolError::CustomError(value) =>
            drop_raw_value_resources(value)
          _ => fail("raw backend custom error changed protocol variant")
        }
      _ => fail("raw backend custom error was not returned")
    }
    assert_eq(calls.val, 3)
    assert_true(!underlying.ownership.state.val.active)
    assert_resource_drop_counts(1, 0, 0)
    underlying.revoke()
    assert_resource_drop_counts(1, 0, 0)
  })
}

///|
test "duplicate resource nodes retain one owner and drop once" {
  let drops = Ref(0)
  let context = OwnershipContext::new(no_raw_drop)
  let carrier = context.acquire_resources([
    resource_candidate(Secret, 7, fn() { drops.val = drops.val + 1 }),
    resource_candidate(Secret, 7, fn() { drops.val = drops.val + 1 }),
  ])
  assert_eq(carrier.tokens.length(), 1)
  assert_true(!carrier.valid)
  assert_true(carrier.transfer_underlying() is Err(_))
  assert_true(carrier.drop() is Ok(_))
  assert_eq(drops.val, 1)
  assert_true(carrier.drop() is Err(_))
  context.revoke()
  assert_eq(drops.val, 1)
}

///|
test "an overlapping carrier cannot claim an older available acquisition" {
  let old_drops = Ref(0)
  let new_drops = Ref(0)
  let context = OwnershipContext::new(no_raw_drop)
  let older = context.acquire_resources([
    resource_candidate(QuotaToken, 11, fn() {
      old_drops.val = old_drops.val + 1
    }),
  ])
  let overlapping = context.acquire_resources([
    resource_candidate(QuotaToken, 11, fn() {
      new_drops.val = new_drops.val + 1
    }),
  ])
  assert_true(!overlapping.valid)
  assert_eq(overlapping.tokens.length(), 0)
  assert_true(overlapping.transfer_underlying() is Err(_))
  assert_true(overlapping.drop() is Ok(_))
  assert_eq(old_drops.val, 0)
  assert_eq(new_drops.val, 0)
  assert_true(older.drop() is Ok(_))
  assert_eq(old_drops.val, 1)
}

///|
test "overlapping raw schema streams cannot create two owners" {
  let context = OwnershipContext::new(no_raw_drop)
  let raw_stream = @types.SchemaValueStream::SchemaValueStream(17)
  let make_carrier = fn() -> RawTypedSchemaValue {
    context.own_raw_typed_schema_value({
      graph: { type_nodes: [], defs: [], root: 0 },
      value: { value_nodes: [@types.StreamValue(raw_stream)], root: 0 },
    })
  }
  let older = make_carrier()
  let overlapping = make_carrier()
  assert_true(older.take_for_final(context) is Ok(_))
  assert_true(overlapping.take_for_final(context) is Err(_))
  context.revoke()
}

///|
test "raw index reuse mints a new generation and rejects the stale carrier" {
  let old_drops = Ref(0)
  let new_drops = Ref(0)
  let context = OwnershipContext::new(no_raw_drop)
  let stale = context.acquire_resources([
    resource_candidate(PermissionCard, 19, fn() {
      old_drops.val = old_drops.val + 1
    }),
  ])
  let stale_generation = stale.tokens[0].generation
  assert_true(stale.drop() is Ok(_))
  let current = context.acquire_resources([
    resource_candidate(PermissionCard, 19, fn() {
      new_drops.val = new_drops.val + 1
    }),
  ])
  assert_eq(current.tokens[0].generation, stale_generation + 1)
  assert_true(stale.transfer_final() is Err(_))
  assert_true(stale.drop() is Err(_))
  assert_true(current.transfer_final() is Ok(_))
  assert_eq(old_drops.val, 1)
  assert_eq(new_drops.val, 0)
  context.revoke()
  assert_eq(new_drops.val, 0)
}

///|
test "stdin is dropped on short circuit and transfers at most once" {
  let short_circuit_drops = Ref(0)
  let short_circuit = OwnershipContext::new(no_raw_drop).own_stdin(
    counted_stream(short_circuit_drops),
  )
  short_circuit.context.revoke()
  assert_eq(short_circuit_drops.val, 1)
  assert_true(short_circuit.take_for_underlying() is Err(_))
  assert_eq(short_circuit_drops.val, 1)

  let forwarded_drops = Ref(0)
  let context = OwnershipContext::new(no_raw_drop)
  let forwarded = context.own_stdin(counted_stream(forwarded_drops))
  let transferred = forwarded.take_for_underlying()
  assert_true(transferred is Ok(_))
  assert_true(forwarded.take_for_underlying() is Err(_))
  assert_true(forwarded.drop() is Err(_))
  context.revoke()
  assert_eq(forwarded_drops.val, 0)
  match transferred {
    Ok(stream) => stream.drop_sync()
    Err(_) => fail("stdin transfer unexpectedly failed")
  }
  assert_eq(forwarded_drops.val, 1)
}

///|
test "stale stdin is rejected before fresh input capabilities transfer" {
  run_component_async(async fn() {
    reset_resource_drop_counts()
    let backend_calls = Ref(0)
    let underlying = new_underlying_tool(
      (_, _, _) => {
        backend_calls.val = backend_calls.val + 1
        Ok({ result: None, stdout: None })
      },
      no_raw_drop,
    )
    let stdin_drops = Ref(0)
    let stale = underlying.ownership.own_stdin(counted_stream(stdin_drops))
    let transferred_stdin = stale.take_for_underlying()
    assert_true(transferred_stdin is Ok(_))
    let secret = @model.GuestSecretHandle::new(@types.Secret::Secret(501))
    let unit = @schema.try_into_typed_schema_value(())
    let input : @model.TypedSchemaValue = {
      graph: unit.graph,
      value: @model.SchemaValue::Secret(secret),
    }
    let result = underlying.invoke([], input, Some(stale))
    assert_true(result is Err(InvalidInput(_)))
    assert_eq(backend_calls.val, 0)
    assert_true(secret.is_present())
    assert_resource_drop_counts(0, 0, 0)
    @model.drop_owned_capabilities(input.value)
    assert_true(!secret.is_present())
    assert_resource_drop_counts(1, 0, 0)
    @model.drop_owned_capabilities(input.value)
    assert_resource_drop_counts(1, 0, 0)
    match transferred_stdin {
      Ok(stream) => stream.drop_sync()
      Err(_) => fail("initial stdin transfer unexpectedly failed")
    }
    assert_eq(stdin_drops.val, 1)
  })
}

///|
test "abandoned and explicitly rejected results synchronously clean up" {
  let abandoned_drops = Ref(0)
  let context = OwnershipContext::new(no_raw_drop)
  let abandoned = context.own_result_with_cleanup(
    { result: None, stdout: Some(counted_stream(abandoned_drops)) },
    fn(result) {
      match result.stdout {
        Some(stdout) => stdout.drop_sync()
        None => ()
      }
    },
  )
  context.revoke()
  assert_eq(abandoned_drops.val, 1)
  assert_true(abandoned.take_for_projection() is Err(_))

  let rejected_drops = Ref(0)
  let rejected_context = OwnershipContext::new(no_raw_drop)
  let rejected = rejected_context.own_result_with_cleanup(
    { result: None, stdout: Some(counted_stream(rejected_drops)) },
    fn(result) {
      match result.stdout {
        Some(stdout) => stdout.drop_sync()
        None => ()
      }
    },
  )
  assert_true(rejected.drop() is Ok(_))
  assert_eq(rejected_drops.val, 1)
  rejected_context.revoke()
  assert_eq(rejected_drops.val, 1)
}

///|
test "projection rejection and malformed wire results release stdout" {
  let rejected_drops = Ref(0)
  let rejected_context = OwnershipContext::new(no_raw_drop)
  let rejected = rejected_context.own_result({
    result: None,
    stdout: Some(counted_stream(rejected_drops)),
  })
  let projected : Result[Int, ToolInvokeError[Unit]] = decode_result_value(
    rejected,
  )
  assert_true(projected is Err(InvalidResult(_)))
  assert_eq(rejected_drops.val, 1)

  let malformed_drops = Ref(0)
  let malformed : @types.TypedSchemaValue = {
    graph: { type_nodes: [], defs: [], root: 0 },
    value: { value_nodes: [], root: 0 },
  }
  let decoded = @toolCore.decode_invocation_result({
    result: Some(malformed),
    stdout: Some(counted_stream(malformed_drops)),
  })
  assert_true(decoded is Err(_))
  assert_eq(malformed_drops.val, 1)
}

///|
test "projected stdout and resources remain owned until teardown" {
  let stdout_drops = Ref(0)
  let stdout_context = OwnershipContext::new(no_raw_drop)
  let stdout_result = stdout_context.own_result({
    result: None,
    stdout: Some(counted_stream(stdout_drops)),
  })
  let projected_stdout : Result[@async-core.Stream[Byte], ToolInvokeError[Unit]] = decode_result_stdout_only(
    stdout_result,
  )
  assert_true(projected_stdout is Ok(_))
  assert_eq(stdout_drops.val, 0)
  stdout_context.revoke()
  assert_eq(stdout_drops.val, 1)
  match projected_stdout {
    Ok(stdout) => stdout.drop_sync()
    Err(_) => fail("stdout projection unexpectedly failed")
  }
  assert_eq(stdout_drops.val, 1)

  reset_resource_drop_counts()
  let resource_context = OwnershipContext::new(no_raw_drop)
  let unit = @schema.try_into_typed_schema_value(())
  let resource_result = resource_context.own_result({
    result: Some({
      graph: unit.graph,
      value: @model.SchemaValue::Secret(
        @model.GuestSecretHandle::new(@types.Secret::Secret(751)),
      ),
    }),
    stdout: None,
  })
  let projected_resource = resource_result.take_for_projection()
  assert_true(projected_resource is Ok(_))
  assert_resource_drop_counts(0, 0, 0)
  resource_context.revoke()
  assert_resource_drop_counts(1, 0, 0)
  match projected_resource {
    Ok({ result: Some(result), .. }) =>
      @model.drop_owned_capabilities(result.value)
    _ => fail("resource result projection unexpectedly failed")
  }
  assert_resource_drop_counts(1, 0, 0)
}

///|
test "projected resource transferred to final encoding is not double-dropped" {
  reset_resource_drop_counts()
  let context = OwnershipContext::new(no_raw_drop)
  let unit = @schema.try_into_typed_schema_value(())
  let owned = context.own_result({
    result: Some({
      graph: unit.graph,
      value: @model.SchemaValue::Secret(
        @model.GuestSecretHandle::new(@types.Secret::Secret(752)),
      ),
    }),
    stdout: None,
  })
  let projected = owned.take_for_projection()
  let transferred = match projected {
    Ok({ result: Some(result), .. }) => @model.typed_schema_value_to_wit(result)
    _ => {
      fail("resource result projection unexpectedly failed")
      raw_unit_value()
    }
  }
  context.revoke()
  assert_resource_drop_counts(0, 0, 0)
  drop_raw_value_resources(transferred)
  assert_resource_drop_counts(1, 0, 0)
  context.revoke()
  assert_resource_drop_counts(1, 0, 0)
}

///|
test "final result transfer precedes revocation and preserves stdout" {
  let stdout_drops = Ref(0)
  let context = OwnershipContext::new(no_raw_drop)
  let owned = context.own_result_with_cleanup(
    { result: None, stdout: Some(counted_stream(stdout_drops)) },
    fn(result) {
      match result.stdout {
        Some(stdout) => stdout.drop_sync()
        None => ()
      }
    },
  )
  let returned = owned.take_for_final()
  assert_true(returned is Ok(_))
  context.revoke()
  assert_eq(stdout_drops.val, 0)
  match returned {
    Ok(result) =>
      match result.stdout {
        Some(stdout) => stdout.drop_sync()
        None => fail("final result did not preserve stdout")
      }
    Err(_) => fail("final result transfer unexpectedly failed")
  }
  assert_eq(stdout_drops.val, 1)
}

///|
test "fake backend supports sequential calls and clears active state" {
  run_component_async(async fn() {
    let calls = Ref(0)
    let underlying = new_underlying_tool(
      (_, _, _) => {
        calls.val = calls.val + 1
        Ok({ result: None, stdout: None })
      },
      no_raw_drop,
    )
    let input = @schema.try_into_typed_schema_value(())
    let first = underlying.invoke([], input, None)
    let second = underlying.invoke([], input, None)
    assert_true(first is Ok(_))
    assert_true(second is Ok(_))
    assert_eq(calls.val, 2)
    assert_true(!underlying.ownership.state.val.active)
    match first {
      Ok(result) => ignore(result.drop())
      Err(_) => ()
    }
    match second {
      Ok(result) => ignore(result.drop())
      Err(_) => ()
    }
  })
}

///|
test "fake backend overlap is rejected while the first call is suspended" {
  run_component_async(async fn() {
    let (started, started_promise) = @async-core.Future::new()
    let (release, release_promise) = @async-core.Future::new()
    let underlying = new_underlying_tool(
      async fn(_, _, _) {
        ignore(started_promise.complete(()))
        release.get()
        Ok({ result: None, stdout: None })
      },
      no_raw_drop,
    )
    let input = @schema.try_into_typed_schema_value(())
    @async-core.with_task_group(async fn(group) {
      let first = group.spawn(async fn() { underlying.invoke([], input, None) })
      started.get()
      let overlap = underlying.invoke([], input, None)
      assert_true(overlap is Err(InvalidInput(_)))
      ignore(release_promise.complete(()))
      match first.wait() {
        Ok(result) => ignore(result.drop())
        Err(_) => fail("first invocation unexpectedly failed")
      }
    })
    assert_true(!underlying.ownership.state.val.active)
  })
}

///|
test "revocation during an active call delays and deduplicates raw drop" {
  run_component_async(async fn() {
    let raw_drops = Ref(0)
    let (started, started_promise) = @async-core.Future::new()
    let (release, release_promise) = @async-core.Future::new()
    let underlying = new_underlying_tool(
      async fn(_, _, _) {
        ignore(started_promise.complete(()))
        release.get()
        Ok({ result: None, stdout: None })
      },
      fn() { raw_drops.val = raw_drops.val + 1 },
    )
    let input = @schema.try_into_typed_schema_value(())
    @async-core.with_task_group(async fn(group) {
      let call = group.spawn(async fn() { underlying.invoke([], input, None) })
      started.get()
      underlying.revoke()
      assert_eq(raw_drops.val, 0)
      ignore(release_promise.complete(()))
      match call.wait() {
        Ok(result) => assert_true(result.drop() is Err(_))
        Err(_) => fail("active invocation unexpectedly failed")
      }
    })
    assert_true(!underlying.ownership.state.val.active)
    assert_eq(raw_drops.val, 1)
    underlying.revoke()
    assert_eq(raw_drops.val, 1)
  })
}

///|
test "backend protocol and raised errors both clear active state" {
  run_component_async(async fn() {
    let input = @schema.try_into_typed_schema_value(())
    let protocol_error = new_underlying_tool(
      (_, _, _) => {
        Err(@toolCommon.ToolError::InvalidInput("backend rejected input"))
      },
      no_raw_drop,
    )
    let result = protocol_error.invoke([], input, None)
    assert_true(result is Err(InvalidInput("backend rejected input")))
    assert_true(!protocol_error.ownership.state.val.active)

    let raised_error = new_underlying_tool(
      (_, _, _) => raise FakeBackendFailure,
      no_raw_drop,
    )
    try raised_error.invoke([], input, None) catch {
      FakeBackendFailure => ()
      _ => fail("unexpected raised backend error")
    } noraise {
      _ => fail("raised backend error unexpectedly returned")
    }
    assert_true(!raised_error.ownership.state.val.active)
  })
}

///|
test "cancelling a suspended backend call clears active state" {
  run_component_async(async fn() {
    let (started, started_promise) = @async-core.Future::new()
    let (blocked, _blocked_promise) = @async-core.Future::new()
    let underlying = new_underlying_tool(
      async fn(_, _, _) {
        ignore(started_promise.complete(()))
        blocked.get()
        Ok({ result: None, stdout: None })
      },
      no_raw_drop,
    )
    let input = @schema.try_into_typed_schema_value(())
    @async-core.with_task_group(async fn(group) {
      let call = group.spawn(async fn() { underlying.invoke([], input, None) })
      started.get()
      call.cancel()
      try call.wait() catch {
        @async-core.Cancelled::Cancelled => ()
        _ => fail("unexpected cancellation error")
      } noraise {
        _ => fail("cancelled invocation unexpectedly returned")
      }
    })
    assert_true(!underlying.ownership.state.val.active)
  })
}

///|
test "scoped underlying teardown runs after normal and raised group exits" {
  run_component_async(async fn() {
    let normal_drops = Ref(0)
    @async-core.with_task_group(async fn(group) {
      let _underlying = new_scoped_underlying_tool(
        (_, _, _) => Ok({ result: None, stdout: None }),
        fn() { normal_drops.val = normal_drops.val + 1 },
        group,
      )
      assert_eq(normal_drops.val, 0)
    })
    assert_eq(normal_drops.val, 1)

    let raised_drops = Ref(0)
    try
      @async-core.with_task_group(async fn(group) {
        let _underlying = new_scoped_underlying_tool(
          (_, _, _) => Ok({ result: None, stdout: None }),
          fn() { raised_drops.val = raised_drops.val + 1 },
          group,
        )
        raise FakeBackendFailure
      })
    catch {
      FakeBackendFailure => ()
      _ => fail("unexpected scoped group error")
    } noraise {
      _ => fail("raised scoped group unexpectedly returned")
    }
    assert_eq(raised_drops.val, 1)
  })
}

///|
test "enclosing group cancellation settles active call before scoped raw drop" {
  run_component_async(async fn() {
    let raw_drops = Ref(0)
    let active_at_drop = Ref(true)
    let underlying_holder : Ref[UnderlyingTool?] = Ref(None)
    let (started, started_promise) = @async-core.Future::new()
    let (blocked, _blocked_promise) = @async-core.Future::new()
    @async-core.with_task_group(async fn(outer_group) {
      let scoped = outer_group.spawn(async fn() {
        @async-core.with_task_group(async fn(inner_group) {
          let underlying = new_scoped_underlying_tool(
            async fn(_, _, _) {
              ignore(started_promise.complete(()))
              blocked.get()
              Ok({ result: None, stdout: None })
            },
            fn() {
              match underlying_holder.val {
                Some(underlying) =>
                  active_at_drop.val = underlying.ownership.state.val.active
                None => active_at_drop.val = true
              }
              raw_drops.val = raw_drops.val + 1
            },
            inner_group,
          )
          underlying_holder.val = Some(underlying)
          let input = @schema.try_into_typed_schema_value(())
          try underlying.invoke([], input, None) catch {
            @async-core.Cancelled::Cancelled =>
              raise @async-core.Cancelled::Cancelled
            error => raise error
          } noraise {
            _ => ()
          }
        })
      })
      started.get()
      scoped.cancel()
      try scoped.wait() catch {
        @async-core.Cancelled::Cancelled => ()
        _ => fail("unexpected enclosing-group cancellation error")
      } noraise {
        _ => fail("cancelled scoped invocation unexpectedly returned")
      }
    })
    assert_eq(raw_drops.val, 1)
    assert_true(!active_at_drop.val)
  })
}
