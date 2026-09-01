///|
fn registry_doc(summary : String) -> @toolCommon.Doc {
  { summary, description: summary + " description", examples: [], }
}

///|
fn registry_tool(name : String) -> @toolCommon.Tool {
  {
    version: "0.1.0",
    commands: {
      nodes: [
        {
          name,
          aliases: [],
          doc: registry_doc(name),
          globals: { options: [], flags: [], },
          subcommands: [],
          body: None,
        },
      ],
    },
    schema: { type_nodes: [], defs: [], root: 0, },
  }
}

///|
fn universal_middleware(
  name : String,
  aliases : Array[String],
) -> ToolMiddleware {
  { name, aliases, doc: registry_doc(name), scope: Universal, }
}

///|
#warnings("-unused_async")
async fn empty_registry_invoker(
  invocation : RawToolInvocation,
  _underlying : UnderlyingTool,
) -> Result[RawInvocationResult, ToolInvokeError[RawTypedSchemaValue]] {
  Ok(invocation.empty_result())
}

///|
test "middleware registry sorts discovery and keeps aliases out of lookup" {
  clear_tool_middlewares_for_tests()
  register_tool_middleware(
    universal_middleware("zeta-policy", ["zeta-alias"]),
    empty_registry_invoker,
  )
  register_tool_middleware(
    {
      name: "alpha-policy",
      aliases: ["alpha-alias"],
      doc: registry_doc("alpha"),
      scope: Monomorphic({
        presented: registry_tool("presented"),
        expected: registry_tool("expected"),
      }),
    },
    empty_registry_invoker,
  )
  let discovered = discover_tool_middlewares()
  assert_eq(discovered.length(), 2)
  assert_eq(discovered[0].name, "alpha-policy")
  assert_eq(discovered[1].name, "zeta-policy")
  match discovered[0].scope {
    @toolCommon.ToolMiddlewareScope::Monomorphic(scope) => {
      assert_eq(scope.presented.commands.nodes[0].name, "presented")
      match scope.expected {
        Some(expected) => assert_eq(expected.commands.nodes[0].name, "expected")
        None => fail("monomorphic registry metadata lost expected tool")
      }
    }
    _ => fail("monomorphic registry metadata changed scope")
  }
  match get_registered_tool_middleware("alpha-policy") {
    Ok(middleware) => {
      assert_eq(middleware.aliases, ["alpha-alias"])
      assert_eq(middleware.doc.summary, "alpha")
    }
    Err(_) => fail("canonical middleware lookup failed")
  }
  assert_true(
    get_registered_tool_middleware("alpha-alias")
    is Err(@toolCommon.ToolError::InvalidToolName("alpha-alias")),
  )
  assert_true(tool_middleware_state.invokers.contains("alpha-policy"))
  assert_true(!tool_middleware_state.invokers.contains("alpha-alias"))
}

///|
test "middleware registration rejects every canonical and alias collision transactionally" {
  clear_tool_middlewares_for_tests()
  assert_true(
    register_tool_middleware_checked(
      universal_middleware("registered", ["registered-alias"]),
      empty_registry_invoker,
    )
    is Ok(_),
  )
  let collisions = [
    universal_middleware("registered", []),
    universal_middleware("registered-alias", []),
    universal_middleware("new-canonical", ["registered"]),
    universal_middleware("another-canonical", ["registered-alias"]),
    universal_middleware("self-collision", ["self-collision"]),
    universal_middleware("duplicate-alias", ["same", "same"]),
  ]
  for middleware in collisions {
    assert_true(
      register_tool_middleware_checked(middleware, empty_registry_invoker)
      is Err(_),
    )
  }
  assert_true(
    register_tool_middleware_checked(
      universal_middleware("", []),
      empty_registry_invoker,
    )
    is Err(_),
  )
  assert_eq(discover_tool_middlewares().length(), 1)
  assert_true(tool_middleware_state.invokers.contains("registered"))
}

///|
fn registry_counted_stream(drops : Ref[Int]) -> @async-core.Stream[Byte] {
  @async-core.Stream::produce(_ => (), on_unstarted_drop=fn() {
    drops.val = drops.val + 1
  })
}

///|
fn registry_raw_secret(index : Int) -> @types.TypedSchemaValue raise {
  let unit = @schema.try_into_typed_schema_value(())
  @model.typed_schema_value_to_wit({
    graph: unit.graph,
    value: @model.SchemaValue::Secret(
      @model.GuestSecretHandle::new(@types.Secret::Secret(index)),
    ),
  })
}

///|
fn drop_registry_raw_resources(value : @types.TypedSchemaValue) -> Unit {
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
#warnings("-unused_async")
test "guest boundary preserves raw carriers and tears down after final transfer" {
  clear_tool_middlewares_for_tests()
  reset_resource_drop_counts()
  let backend_calls = Ref(0)
  let raw_drops = Ref(0)
  let stdin_drops = Ref(0)
  let stdout_drops = Ref(0)
  register_tool_middleware(universal_middleware("pass-through", []), async fn(
    invocation,
    underlying,
  ) {
    assert_eq(invocation.tool_name(), "runtime-tool")
    assert_eq(
      invocation.tool_metadata().commands.nodes[0].name,
      "runtime-metadata",
    )
    assert_eq(invocation.command_path(), ["nested", "run"])
    assert_true(invocation.principal() is @agentCommon.Principal::Anonymous)
    underlying.invoke_raw(
      invocation.command_path(),
      invocation.input(),
      invocation.stdin(),
    )
  })
  run_component_async(async fn() {
    let final_result : Ref[@toolCommon.InvocationResult?] = Ref(None)
    @async-core.with_task_group(async fn(group) {
      let underlying = new_scoped_underlying_tool(
        async fn(command_path, input, stdin) {
          backend_calls.val = backend_calls.val + 1
          assert_eq(command_path, ["nested", "run"])
          match stdin {
            Some(stream) => stream.drop_sync()
            None => fail("guest boundary lost stdin")
          }
          Ok({
            result: Some(input),
            stdout: Some(registry_counted_stream(stdout_drops)),
          })
        },
        fn() { raw_drops.val = raw_drops.val + 1 },
        group,
      )
      match
        invoke_registered_tool_middleware(
          "pass-through",
          "runtime-tool",
          registry_tool("runtime-metadata"),
          ["nested", "run"],
          registry_raw_secret(701),
          Some(registry_counted_stream(stdin_drops)),
          @agentCommon.Principal::Anonymous,
          underlying,
        ) {
        Ok(result) => final_result.val = Some(result)
        Err(_) => fail("registered middleware invocation failed")
      }
      assert_eq(raw_drops.val, 0)
      assert_eq(stdout_drops.val, 0)
    })
    assert_eq(backend_calls.val, 1)
    assert_eq(raw_drops.val, 1)
    assert_eq(stdin_drops.val, 1)
    assert_eq(stdout_drops.val, 0)
    assert_resource_drop_counts(0, 0, 0)
    match final_result.val {
      Some(result) => {
        match result.result {
          Some(value) => drop_registry_raw_resources(value)
          None => fail("guest boundary lost raw result")
        }
        match result.stdout {
          Some(stdout) => stdout.drop_sync()
          None => fail("guest boundary lost stdout")
        }
      }
      None => fail("guest boundary did not return a result")
    }
    assert_resource_drop_counts(1, 0, 0)
    assert_eq(stdout_drops.val, 1)
  })
}

///|
test "universal middleware short-circuits without invoking and cleans raw input" {
  clear_tool_middlewares_for_tests()
  reset_resource_drop_counts()
  let backend_calls = Ref(0)
  let raw_drops = Ref(0)
  let stdin_drops = Ref(0)
  register_tool_middleware(
    universal_middleware("short-circuit", []),
    empty_registry_invoker,
  )
  run_component_async(async fn() {
    @async-core.with_task_group(async fn(group) {
      let underlying = new_scoped_underlying_tool(
        (_, _, _) => {
          backend_calls.val = backend_calls.val + 1
          Ok({ result: None, stdout: None, })
        },
        fn() { raw_drops.val = raw_drops.val + 1 },
        group,
      )
      let result = invoke_registered_tool_middleware(
        "short-circuit",
        "runtime-tool",
        registry_tool("runtime-metadata"),
        [],
        registry_raw_secret(705),
        Some(registry_counted_stream(stdin_drops)),
        @agentCommon.Principal::Anonymous,
        underlying,
      )
      assert_true(result is Ok({ result: None, stdout: None, }))
      assert_eq(backend_calls.val, 0)
      assert_eq(raw_drops.val, 0)
    })
    assert_eq(backend_calls.val, 0)
    assert_eq(raw_drops.val, 1)
    assert_eq(stdin_drops.val, 1)
    assert_resource_drop_counts(1, 0, 0)
  })
}

///|
test "universal middleware can retry the underlying capability sequentially" {
  clear_tool_middlewares_for_tests()
  let backend_calls = Ref(0)
  let raw_drops = Ref(0)
  register_tool_middleware(universal_middleware("retry", []), async fn(
    invocation,
    underlying,
  ) {
    let first = underlying.invoke(
      [],
      @schema.try_into_typed_schema_value(()),
      None,
    )
    assert_true(first is Err(ConstraintViolation("retry")))
    let second = underlying.invoke(
      [],
      @schema.try_into_typed_schema_value(()),
      None,
    )
    assert_true(second is Ok(_))
    Ok(invocation.empty_result())
  })
  run_component_async(async fn() {
    @async-core.with_task_group(async fn(group) {
      let underlying = new_scoped_underlying_tool(
        (_, _, _) => {
          backend_calls.val = backend_calls.val + 1
          if backend_calls.val == 1 {
            Err(@toolCommon.ToolError::ConstraintViolation("retry"))
          } else {
            Ok({ result: None, stdout: None, })
          }
        },
        fn() { raw_drops.val = raw_drops.val + 1 },
        group,
      )
      let result = invoke_registered_tool_middleware(
        "retry",
        "runtime-tool",
        registry_tool("runtime-metadata"),
        [],
        @model.typed_schema_value_to_wit(
          @schema.try_into_typed_schema_value(()),
        ),
        None,
        @agentCommon.Principal::Anonymous,
        underlying,
      )
      assert_true(result is Ok({ result: None, stdout: None, }))
      assert_eq(backend_calls.val, 2)
      assert_eq(raw_drops.val, 0)
    })
    assert_eq(raw_drops.val, 1)
  })
}

///|
test "guest boundary cleans uninvoked input stdin and underlying on lookup failure" {
  clear_tool_middlewares_for_tests()
  reset_resource_drop_counts()
  let backend_calls = Ref(0)
  let raw_drops = Ref(0)
  let stdin_drops = Ref(0)
  run_component_async(async fn() {
    @async-core.with_task_group(async fn(group) {
      let underlying = new_scoped_underlying_tool(
        (_, _, _) => {
          backend_calls.val = backend_calls.val + 1
          Ok({ result: None, stdout: None, })
        },
        fn() { raw_drops.val = raw_drops.val + 1 },
        group,
      )
      let result = invoke_registered_tool_middleware(
        "missing",
        "runtime-tool",
        registry_tool("runtime-metadata"),
        [],
        registry_raw_secret(702),
        Some(registry_counted_stream(stdin_drops)),
        @agentCommon.Principal::Anonymous,
        underlying,
      )
      assert_true(
        result is Err(@toolCommon.ToolError::InvalidToolName("missing")),
      )
      assert_eq(raw_drops.val, 0)
    })
    assert_eq(backend_calls.val, 0)
    assert_eq(raw_drops.val, 1)
    assert_eq(stdin_drops.val, 1)
    assert_resource_drop_counts(1, 0, 0)
  })
}

///|
#warnings("-unused_async")
test "guest boundary preserves protocol errors and transfers raw custom errors" {
  clear_tool_middlewares_for_tests()
  reset_resource_drop_counts()
  let raw_drops = Ref(0)
  register_tool_middleware(universal_middleware("protocol-error", []), async fn(
    _invocation,
    _underlying,
  ) {
    Err(ConstraintViolation("denied"))
  })
  register_tool_middleware(universal_middleware("custom-error", []), async fn(
    invocation,
    _underlying,
  ) {
    Err(Tool(invocation.input()))
  })
  run_component_async(async fn() {
    let custom_payload : Ref[@types.TypedSchemaValue?] = Ref(None)
    @async-core.with_task_group(async fn(group) {
      let protocol_underlying = new_scoped_underlying_tool(
        (_, _, _) => Ok({ result: None, stdout: None, }),
        fn() { raw_drops.val = raw_drops.val + 1 },
        group,
      )
      let protocol = invoke_registered_tool_middleware(
        "protocol-error",
        "runtime-tool",
        registry_tool("runtime-metadata"),
        [],
        @model.typed_schema_value_to_wit(
          @schema.try_into_typed_schema_value(()),
        ),
        None,
        @agentCommon.Principal::Anonymous,
        protocol_underlying,
      )
      assert_true(
        protocol is Err(@toolCommon.ToolError::ConstraintViolation("denied")),
      )
      let custom_underlying = new_scoped_underlying_tool(
        (_, _, _) => Ok({ result: None, stdout: None, }),
        fn() { raw_drops.val = raw_drops.val + 1 },
        group,
      )
      match
        invoke_registered_tool_middleware(
          "custom-error",
          "runtime-tool",
          registry_tool("runtime-metadata"),
          [],
          registry_raw_secret(704),
          None,
          @agentCommon.Principal::Anonymous,
          custom_underlying,
        ) {
        Err(@toolCommon.ToolError::CustomError(payload)) =>
          custom_payload.val = Some(payload)
        _ => fail("guest boundary changed raw custom error")
      }
      assert_eq(raw_drops.val, 0)
    })
    assert_eq(raw_drops.val, 2)
    assert_resource_drop_counts(0, 0, 0)
    match custom_payload.val {
      Some(payload) => drop_registry_raw_resources(payload)
      None => fail("guest boundary lost raw custom error payload")
    }
    assert_resource_drop_counts(1, 0, 0)
  })
}

///|
priv suberror RegistryInvokerFailure derive(Debug)

///|
#warnings("-unused_async")
test "guest boundary group teardown runs when a registered invoker raises" {
  clear_tool_middlewares_for_tests()
  reset_resource_drop_counts()
  let raw_drops = Ref(0)
  let stdin_drops = Ref(0)
  register_tool_middleware(universal_middleware("raises", []), async fn(
    _invocation,
    _underlying,
  ) {
    raise RegistryInvokerFailure
  })
  run_component_async(async fn() {
    try
      @async-core.with_task_group(async fn(group) {
        let underlying = new_scoped_underlying_tool(
          (_, _, _) => Ok({ result: None, stdout: None, }),
          fn() { raw_drops.val = raw_drops.val + 1 },
          group,
        )
        ignore(
          invoke_registered_tool_middleware(
            "raises",
            "runtime-tool",
            registry_tool("runtime-metadata"),
            [],
            registry_raw_secret(703),
            Some(registry_counted_stream(stdin_drops)),
            @agentCommon.Principal::Anonymous,
            underlying,
          ),
        )
      })
    catch {
      RegistryInvokerFailure => ()
      _ => fail("unexpected registered invoker failure")
    } noraise {
      _ => fail("raised registered invoker unexpectedly returned")
    }
    assert_eq(raw_drops.val, 1)
    assert_eq(stdin_drops.val, 1)
    assert_resource_drop_counts(1, 0, 0)
  })
}
