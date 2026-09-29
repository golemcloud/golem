///|
fn stdout_test_def(name : String) -> ToolDef {
  let command = fn(
    name : String,
    stdout : @toolCommon.StreamSpec?,
  ) -> CommandNodeDef {
    {
      name,
      aliases: [name + "-alias"],
      doc: { summary: name, description: "", examples: [], },
      globals: { options: [], flags: [], },
      subcommands: [],
      body: Some({
        positionals: { fixed: [], tail: None, },
        options: [],
        flags: [],
        constraints: [],
        stdin: None,
        stdout,
        stderr: None,
        result: None,
        errors: [],
        annotations: None,
      }),
    }
  }
  let stream = fn(required : Bool) -> @toolCommon.StreamSpec {
    {
      doc: { summary: "output", description: "", examples: [], },
      mime: [],
      required,
    }
  }
  let root = command(name, None)
  {
    version: "1",
    commands: [
      { ..root, subcommands: [1, 2], },
      command("required", Some(stream(true))),
      command("optional", Some(stream(false))),
    ],
  }
}

///|
test "stdout validation follows selected command metadata for definitions and wire registrations" {
  for wire in [false, true] {
    let name = if wire { "stdout-wire" } else { "stdout-def" }
    let def = stdout_test_def(name)
    if wire {
      register_wire_tool_invoker(def.to_tool(), (_, _, _, _, _, _) => {
        Ok(invocation_result_empty())
      })
    } else {
      register_tool_def_invoker(def, (_, _, _, _, _, _) => {
        Ok(invocation_result_empty())
      })
    }
    assert_true(validate_tool_outputs(name, [], false, false) is Ok(_))
    assert_true(
      validate_tool_outputs(name, [], true, false)
      is Err(@types.ToolError::InvalidInput(_)),
    )
    for path in [["required"], ["required-alias"]] {
      assert_true(
        validate_tool_outputs(name, path, false, false)
        is Err(@types.ToolError::InvalidInput(_)),
      )
      assert_true(validate_tool_outputs(name, path, true, false) is Ok(_))
    }
    for supplied in [false, true] {
      assert_true(
        validate_tool_outputs(name, ["optional"], supplied, false) is Ok(_),
      )
      assert_true(
        validate_tool_outputs(name, ["missing"], supplied, false)
        is Err(@types.ToolError::InvalidCommandPath(_)),
      )
    }
  }
  assert_true(
    validate_tool_outputs("missing-tool", [], true, false)
    is Err(@types.ToolError::InvalidToolName(_)),
  )
}

///|
test "grafted stdout follows mount aliases rather than parent metadata" {
  let parent = stdout_test_def("stdout-parent")
  let child = stdout_test_def("stdout-child")
  let grafted = parent.graft_subtree(
    child,
    "mounted",
    { options: [], flags: [], },
    Some("mounted"),
    None,
    Some(["mount-alias"]),
  )
  register_tool_def(grafted)
  assert_true(
    validate_tool_outputs("stdout-parent", [], true, false)
    is Err(@types.ToolError::InvalidInput(_)),
  )
  assert_true(
    validate_tool_outputs(
      "stdout-parent",
      ["mount-alias", "required-alias"],
      true,
      false,
    )
    is Ok(_),
  )
  assert_true(
    validate_tool_outputs(
      "stdout-parent",
      ["mounted", "required"],
      false,
      false,
    )
    is Err(@types.ToolError::InvalidInput(_)),
  )
}

///|
test "ordinary registry keeps private invokers hidden and releases rejected stdin" {
  run_provider_async(() => {
    let called = Ref(0)
    let private = stdout_test_def("private-invoker")
    register_subtool_def_invoker(private, (_, _, _, _, _, _) => {
      called.val += 1
      Ok(invocation_result_empty())
    })
    register_tool_def(stdout_test_def("metadata-only"))
    assert_true(get_registered_tool("private-invoker") is Err(_))
    for name in ["private-invoker", "metadata-only", "not-registered"] {
      let dropped = Ref(0)
      let stdin = @async-core.Stream::from_source(_ => None, () => (), () => {
        dropped.val += 1
      })
      let input = @model.typed_schema_value_to_wit({
        graph: {
          defs: [],
          root: @model.schema_type(@model.SchemaTypeBody::Record([])),
        },
        value: @model.SchemaValue::Record([]),
      })
      assert_true(
        invoke_registered_tool(
          name,
          [],
          input,
          Some(stdin),
          None,
          None,
          Principal::Anonymous,
        )
        is Err(@types.ToolError::InvalidToolName(_)),
      )
      assert_eq(dropped.val, 1)
    }
    assert_eq(called.val, 0)
    assert_true(
      invoke_registered_subtool(
        "private-invoker",
        [],
        [],
        None,
        None,
        None,
        Principal::Anonymous,
      )
      is Ok(_),
    )
    assert_eq(called.val, 1)
  })
}

///|
test "panic wire registration cannot replace a private invoker name" {
  let def = stdout_test_def("wire-private-collision")
  register_subtool_def_invoker(def, (_, _, _, _, _, _) => {
    Ok(invocation_result_empty())
  })
  register_wire_tool_invoker(def.to_tool(), (_, _, _, _, _, _) => {
    Ok(invocation_result_empty())
  })
}
