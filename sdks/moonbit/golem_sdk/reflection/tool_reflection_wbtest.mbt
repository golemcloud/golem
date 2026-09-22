///|
fn sample_tool() -> ToolType raise {
  let doc : @toolCommon.Doc = { summary: "", description: "", examples: [], }
  let graph = @model.schema_graph_to_wit({
    defs: [],
    root: @model.schema_type(String),
  })
  let body : @toolCommon.CommandBody = {
    positionals: {
      fixed: [
        {
          name: "message",
          doc,
          value_name: None,
          type_: graph.root,
          default: None,
          required: true,
          accepts_stdio: false,
        },
        {
          name: "note",
          doc,
          value_name: None,
          type_: graph.root,
          default: None,
          required: false,
          accepts_stdio: false,
        },
      ],
      tail: None,
    },
    options: [
      {
        long: "label",
        short: None,
        aliases: [],
        doc,
        value_name: None,
        shape: @toolCommon.OptionShape::Scalar(graph.root),
        default: None,
        required: false,
        env_var: None,
      },
    ],
    flags: [],
    constraints: [],
    stdin: None,
    stdout: None,
    result: Some({
      type_: graph.root,
      doc,
      formatters: [],
      default_formatter: "",
    }),
    errors: [],
    annotations: None,
  }
  let definition : @toolCommon.Tool = {
    version: "1",
    commands: {
      nodes: [
        {
          name: "sample",
          aliases: [],
          doc,
          globals: { options: [], flags: [], },
          subcommands: [1],
          body: None,
        },
        {
          name: "run",
          aliases: ["r"],
          doc,
          globals: { options: [], flags: [], },
          subcommands: [],
          body: Some(body),
        },
      ],
    },
    schema: graph,
  }
  {
    lookup_name: "sample",
    definition,
    implemented_by: { uuid: { high_bits: 0UL, low_bits: 0UL, }, },
    decoded: @model.schema_graph_roots_from_wit(graph),
  }
}

///|
test "reflected tool resolves aliases and rejects input before RPC" {
  let command = sample_tool().command(["r"])
  assert_eq(command.path, ["run"])
  assert_eq(command.arguments[0].name, "message")
  let invalid = @model.SchemaValue::Record([@model.SchemaValue::S32(1)])
  match command.start_value(invalid) {
    Err(@tool.ToolError::InvalidInput(_)) => ()
    _ => fail("expected local tool input rejection")
  }
}

///|
test "reflected input definitions follow canonical argument order" {
  let root = @model.schema_type(
    Record([
      @model.field("a", @model.schema_type(Ref("a"))),
      @model.field("b", @model.schema_type(Ref("b"))),
    ]),
  )
  let wire = @model.schema_graph_to_wit({
    defs: [
      { id: "b", name: None, body: @model.schema_type(String), },
      { id: "a", name: None, body: @model.schema_type(String), },
    ],
    root,
  })
  guard wire.type_nodes[wire.root].body is @types.RecordType(fields) else {
    fail("expected record root")
  }
  let base = sample_tool()
  let nodes = base.definition.commands.nodes
  let command = nodes[1]
  let body = command.body.unwrap()
  let original = body.positionals.fixed[0]
  let tool : ToolType = {
    ..base,
    decoded: @model.schema_graph_roots_from_wit(wire),
    definition: {
      ..base.definition,
      schema: wire,
      commands: {
        nodes: [
          nodes[0],
          {
            ..command,
            body: Some({
              ..body,
              positionals: {
                fixed: [
                  { ..original, name: "a", type_: fields[0].body, },
                  { ..original, name: "b", type_: fields[1].body, },
                ],
                tail: None,
              },
              options: [],
              result: None,
            }),
          },
        ],
      },
    },
  }
  let selected = tool.command(["run"])
  assert_eq(selected.input_schema.graph.defs.map(def => def.id), ["a", "b"])
}

///|
test "namespace-only commands expose metadata and reject invocation locally" {
  let metadata = sample_tool().command([])
  assert_true(metadata.body is None)
  assert_eq(metadata.subcommands, ["run"])
  match metadata.start_value(@model.SchemaValue::Record([])) {
    Err(@tool.ToolError::InvalidInput(message)) =>
      assert_true(message.contains("namespace"))
    _ => fail("expected namespace invocation to fail locally")
  }
}

///|
test "caller-owned tool definitions bind named and partial clients without discovery" {
  let named = ToolClientDefinition::named("stable-tool", fn(name) { name })
  let partial = ToolClientDefinition::unnamed(fn(name) { name })
  assert_eq(named.client(), "stable-tool")
  assert_eq(named.client_for("ignored"), "stable-tool")
  try partial.client() catch {
    Discovery(_) => ()
    _ => fail("expected a missing target discovery error")
  } noraise {
    _ => fail("nameless definition unexpectedly created a client")
  }
  assert_eq(partial.client_for("deployed-tool"), "deployed-tool")
  try partial.client_for(" ") catch {
    Discovery(_) => ()
    _ => fail("expected a blank target discovery error")
  } noraise {
    _ => fail("blank tool name unexpectedly created a client")
  }
}

///|
test "nested namespace lookup retains canonical child names" {
  let tool = sample_tool()
  let nodes = tool.definition.commands.nodes
  let nested = ToolType::{
    ..tool,
    definition: {
      ..tool.definition,
      commands: {
        nodes: [
          { ..nodes[0], subcommands: [1], },
          { ..nodes[0], name: "group", aliases: ["g"], subcommands: [2], },
          nodes[1],
        ],
      },
    },
  }
  let metadata = nested.command(["g"])
  let command = nested.command(["g", "r"])
  assert_eq(metadata.path, ["group"])
  assert_eq(metadata.subcommands, ["run"])
  assert_eq(command.path, ["group", "run"])
}

///|
test "reflected input graph excludes definitions owned only by output" {
  let input_def : @model.SchemaTypeDef = {
    id: "Input",
    name: Some("Input"),
    body: @model.schema_type(String),
  }
  let output_def : @model.SchemaTypeDef = {
    id: "Output",
    name: Some("Output"),
    body: @model.schema_type(
      Record([@model.field("value", @model.schema_type(String))]),
    ),
  }
  let encoder = @model.GraphEncoder::new([input_def, output_def])
  let input_root = encoder.encode_type(@model.schema_type(Ref("Input")))
  let output_root = encoder.encode_type(@model.schema_type(Ref("Output")))
  let graph = encoder.finish()
  let tool = sample_tool()
  let nodes = tool.definition.commands.nodes
  let body = nodes[1].body.unwrap()
  let command = ToolType::{
    ..tool,
    decoded: @model.schema_graph_roots_from_wit({ ..graph, root: input_root, }),
    definition: {
      ..tool.definition,
      schema: { ..graph, root: input_root, },
      commands: {
        nodes: [
          nodes[0],
          {
            ..nodes[1],
            body: Some({
              ..body,
              positionals: {
                ..body.positionals,
                fixed: [{ ..body.positionals.fixed[0], type_: input_root, }],
              },
              options: [],
              result: Some({ ..body.result.unwrap(), type_: output_root, }),
            }),
          },
        ],
      },
    },
  }.command(["run"])
  assert_eq(command.wire_input.defs.map(def => def.id), ["Input"])
}

///|
test "reflected optional tool inputs share the canonical wire schema" {
  let command = sample_tool().command(["run"])
  assert_eq(command.input_schema.graph, command.wire_input)
  let omitted = @model.SchemaValue::Record([
    @model.SchemaValue::String("hello"),
    @model.SchemaValue::Option(None),
    @model.SchemaValue::Option(None),
  ])
  let supplied = @model.SchemaValue::Record([
    @model.SchemaValue::String("hello"),
    @model.SchemaValue::Option(Some(@model.SchemaValue::String("note"))),
    @model.SchemaValue::Option(Some(@model.SchemaValue::String("label"))),
  ])
  match command.checked_input(omitted) {
    Ok(_) => ()
    Err(_) => fail("expected omitted optional inputs to validate")
  }
  match command.checked_input(supplied) {
    Ok(_) => ()
    Err(_) => fail("expected supplied optional inputs to validate")
  }
}

///|
extern "wasm" fn set_reflection_tool_host_mode(mode : Int) = "golem:test" "set-tool-host-mode"

///|
extern "wasm" fn reset_reflection_resource_drop_counts() = "golem:test" "reset-resource-drop-counts"

///|
extern "wasm" fn reflection_resource_drop_count(kind : Int) -> Int = "golem:test" "resource-drop-count"

///|
fn run_reflection_tool_async(body : async () -> Unit) -> Unit raise {
  let mut callback = @async-core.with_waitableset(async fn() {
    body()
    @async-core.task_returned()
  })
  let mut steps = 0
  while callback != 0 {
    steps += 1
    if steps > 1000 {
      fail("reflected tool async test did not complete")
    }
    if (callback & 0xf) != 1 {
      fail("reflected tool test unexpectedly waited on a host event")
    }
    callback = @async-core.cb(0, 0, 0)
  }
}

///|
test "repeated reflected convenience calls release result futures on every exit" {
  let command = sample_tool().command(["run"])
  let input = @model.SchemaValue::Record([
    @model.SchemaValue::String("hello"),
    @model.SchemaValue::Option(None),
    @model.SchemaValue::Option(None),
  ])
  reset_reflection_resource_drop_counts()
  set_reflection_tool_host_mode(1)
  defer set_reflection_tool_host_mode(0)
  run_reflection_tool_async(async fn() {
    for _ in 0..<50 {
      match command.invoke_value(input) {
        Err(@tool.ToolError::Rpc(@tool.RpcError::Cancelled)) => ()
        _ => fail("expected cancelled tool result")
      }
    }
  })
  assert_eq(reflection_resource_drop_count(4), 50)
  set_reflection_tool_host_mode(2)
  run_reflection_tool_async(async fn() {
    for _ in 0..<50 {
      match command.invoke_value(input) {
        Err(@tool.ToolError::MalformedRemoteOutput(_)) => ()
        _ => fail("expected missing remote result")
      }
    }
  })
  assert_eq(reflection_resource_drop_count(4), 100)
}

///|
test "reflected tool reports malformed remote result" {
  let command = sample_tool().command(["run"])
  match command.decode_result({ result: None, stdout: None, }) {
    Err(@tool.ToolError::MalformedRemoteOutput(_)) => ()
    _ => fail("expected missing tool result to be malformed")
  }
}

///|
test "tool constraint references use flag defaults and nested values" {
  let tool = sample_tool()
  let command = tool.command(["run"])
  let flag : @toolCommon.FlagSpec = {
    long: "enabled",
    short: None,
    aliases: [],
    doc: { summary: "", description: "", examples: [], },
    shape: BoolFlag({ default: true, negatable: true, }),
    env_var: None,
  }
  command.arguments.push(tool_flag_argument(tool, flag))
  assert_eq(command.arguments[3].default, Some(@model.SchemaValue::Bool(true)))
  let values = [
    @model.SchemaValue::String("message"),
    @model.SchemaValue::Option(None),
    @model.SchemaValue::Option(Some(@model.SchemaValue::String("fast"))),
    @model.SchemaValue::Bool(false),
  ]
  assert_true(command.constraint_ref_matches(Present("enabled"), values))
  assert_true(
    command.constraint_ref_matches(
      ValueIs({
        name: "label",
        value: @model.schema_value_to_wit(@model.SchemaValue::String("fast")),
      }),
      values,
    ),
  )
  values[3] = @model.SchemaValue::Bool(true)
  assert_false(command.constraint_ref_matches(Present("enabled"), values))
}
