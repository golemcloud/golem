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
    stderr: None,
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
    requires_filesystem: false,
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
    Err(@tool.ToolError::RemoteTool(@tool.RemoteToolError::InvalidInput(_))) =>
      ()
    _ => fail("expected local tool input rejection")
  }
}

///|
test "reflected trigger rejects each required output before RPC" {
  let base = sample_tool()
  let nodes = base.definition.commands.nodes
  let command = nodes[1]
  let body = command.body.unwrap()
  let required : @toolCommon.StreamSpec = {
    doc: { summary: "", description: "", examples: [], },
    mime: [],
    required: true,
  }
  let input = @model.SchemaValue::Record([
    @model.SchemaValue::String("hello"),
    @model.SchemaValue::Option(None),
    @model.SchemaValue::Option(None),
  ])
  for
    output_case in [
      (Some(required), None, "command requires caller-readable stdout"),
      (None, Some(required), "command requires caller-readable stderr"),
    ] {
    let (stdout, stderr, expected) = output_case
    let tool : ToolType = {
      ..base,
      definition: {
        ..base.definition,
        commands: {
          nodes: [
            nodes[0],
            { ..command, body: Some({ ..body, stdout, stderr, }), },
          ],
        },
      },
    }
    match tool.command(["run"]).trigger_value(input) {
      Err(
        @tool.ToolError::RemoteTool(
          @tool.RemoteToolError::InvalidInput(message)
        )
      ) => assert_eq(message, expected)
      _ => fail("expected required output to be rejected before RPC")
    }
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
    Err(
      @tool.ToolError::RemoteTool(@tool.RemoteToolError::InvalidInput(message))
    ) => assert_true(message.contains("namespace"))
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
test "tool snapshot accessors cannot mutate later command lookup" {
  let tool = sample_tool()
  let command = tool.command(["r"])
  let path = command.path()
  path[0] = "mutated"
  let arguments = command.arguments()
  let aliases = arguments[0].aliases()
  aliases.push("mutated")
  arguments[0] = arguments[1]
  let graph = command.input_schema().graph()
  guard graph.root.body is Record(fields) else {
    fail("expected reflected record input")
  }
  fields[0] = { ..fields[0], name: "mutated", }
  fields[1].metadata.aliases.push("mutated")

  let fresh = tool.command(["r"])
  assert_eq(fresh.path(), ["run"])
  assert_eq(fresh.arguments()[0].name(), "message")
  assert_eq(fresh.arguments()[0].aliases(), [])
  let fresh_graph = fresh.input_schema().graph()
  guard fresh_graph.root.body is Record(fresh_fields) else {
    fail("expected reflected record input")
  }
  assert_eq(fresh_fields[0].name, "message")
  assert_eq(fresh_fields[1].metadata.aliases, [])
  fresh
  .input_schema()
  .validate_value(
    @model.SchemaValue::Record([
      @model.SchemaValue::String("message"),
      @model.SchemaValue::Option(None),
      @model.SchemaValue::Option(None),
    ]),
  )
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
        Err(@tool.ToolError::Rpc(@tool.RpcError::Protocol(_))) => ()
        _ => fail("expected missing remote result")
      }
    }
  })
  assert_eq(reflection_resource_drop_count(4), 100)
}

///|
test "reflected tool reports malformed remote result" {
  let command = sample_tool().command(["run"])
  match command.decode_result({ result: None, stdout: None, stderr: None, }) {
    Err(@tool.ToolError::Rpc(@tool.RpcError::Protocol(_))) => ()
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

///|
fn bash_tool(repeatables? : Bool = false) -> ToolType raise {
  let doc : @toolCommon.Doc = { summary: "", description: "", examples: [], }
  let encoder = @model.GraphEncoder::new([])
  let string = encoder.encode_type(@model.schema_type(String))
  let u32 = encoder.encode_type(@model.schema_type(U32(None)))
  let optional_string = encoder.encode_type(
    @model.schema_type(Option(@model.schema_type(String))),
  )
  let graph = encoder.finish()
  let option_spec = fn(
    long : String,
    shape : @toolCommon.OptionShape,
    default : @model.SchemaValue?,
  ) -> @toolCommon.OptionSpec raise {
    {
      long,
      short: None,
      aliases: [],
      doc,
      value_name: None,
      shape,
      default: match default {
        Some(value) => Some(@model.schema_value_to_wit(value))
        None => None
      },
      required: false,
      env_var: None,
    }
  }
  let options = [
    option_spec("cwd", Scalar(string), Some(@model.SchemaValue::String(""))),
    option_spec("timeout", Scalar(u32), Some(@model.SchemaValue::U32(600U))),
    option_spec("label", Scalar(string), None),
    option_spec(
      "shell",
      Scalar(optional_string),
      Some(@model.SchemaValue::Option(Some(@model.SchemaValue::String("bash")))),
    ),
  ]
  if repeatables {
    options.push(
      option_spec(
        "env",
        RepeatableList({ repetition: Repeated, item_type: string, }),
        Some(@model.SchemaValue::List([@model.SchemaValue::String("A=1")])),
      ),
    )
  }
  let body : @toolCommon.CommandBody = {
    positionals: {
      fixed: [
        {
          name: "script",
          doc,
          value_name: None,
          type_: string,
          default: None,
          required: true,
          accepts_stdio: false,
        },
      ],
      tail: if repeatables {
        Some({
          name: "args",
          doc,
          value_name: None,
          item_type: string,
          min: 0U,
          max: None,
          separator: None,
          verbatim: false,
          accepts_stdio: false,
        })
      } else {
        None
      },
    },
    options,
    flags: [
      {
        long: "login",
        short: None,
        aliases: [],
        doc,
        shape: BoolFlag({ default: false, negatable: false, }),
        env_var: None,
      },
      {
        long: "verbose",
        short: None,
        aliases: [],
        doc,
        shape: CountFlag(None),
        env_var: None,
      },
    ],
    constraints: [],
    stdin: None,
    stdout: None,
    stderr: None,
    result: None,
    errors: [],
    annotations: None,
  }
  let definition : @toolCommon.Tool = {
    version: "1",
    requires_filesystem: false,
    commands: {
      nodes: [
        {
          name: "bash",
          aliases: [],
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
    lookup_name: "bash",
    definition,
    implemented_by: { uuid: { high_bits: 0UL, low_bits: 0UL, }, },
    decoded: @model.schema_graph_roots_from_wit(graph),
  }
}

///|
fn bash_input(
  script? : String = "echo ok",
  cwd? : String = "",
  timeout? : UInt = 600U,
  label? : String? = None,
  shell? : String? = Some("bash"),
  login? : Bool = false,
  verbose? : UInt = 0U,
) -> @model.SchemaValue {
  Record([
    String(script),
    String(cwd),
    U32(timeout),
    Option(label.map(value => @model.SchemaValue::String(value))),
    Option(shell.map(value => @model.SchemaValue::String(value))),
    Bool(login),
    U32(verbose),
  ])
}

///|
fn expect_packed_json(
  command : ToolCommand,
  input : Json,
) -> @model.SchemaValue raise {
  match command.pack_json(input) {
    Ok(value) => value
    Err(
      @tool.ToolError::RemoteTool(@tool.RemoteToolError::InvalidInput(message))
    ) => fail("unexpected tool input rejection: " + message)
    Err(_) => fail("unexpected tool input failure")
  }
}

///|
test "reflected tool JSON fills omitted declared defaults" {
  let command = bash_tool().command([])
  assert_eq(command.arguments.map(argument => argument.name), [
    "script", "cwd", "timeout", "label", "shell", "login", "verbose",
  ])
  assert_eq(expect_packed_json(command, { "script": "echo ok" }), bash_input())
  assert_eq(
    command.input_schema.pack_json({ "script": "echo ok" }),
    bash_input(),
  )
}

///|
test "reflected tool JSON keeps explicit values over declared defaults" {
  let command = bash_tool().command([])
  assert_eq(
    expect_packed_json(command, {
      "script": "ls",
      "cwd": "/tmp",
      "timeout": 30,
      "label": "listing",
      "shell": null,
      "login": true,
      "verbose": 2,
    }),
    bash_input(
      script="ls",
      cwd="/tmp",
      timeout=30U,
      label=Some("listing"),
      shell=None,
      login=true,
      verbose=2U,
    ),
  )
  assert_eq(
    expect_packed_json(command, { "script": "sleep 1", "timeout": 120000 }),
    bash_input(script="sleep 1", timeout=120000U),
  )
}

///|
test "reflected tool JSON names a missing required argument and its path" {
  let command = bash_tool().command([])
  match command.pack_json({ "timeout": 30 }) {
    Err(
      @tool.ToolError::RemoteTool(@tool.RemoteToolError::InvalidInput(message))
    ) => assert_true(message.contains("missing field 'script'"))
    _ => fail("expected the missing script to be rejected")
  }
  try command.input_schema.pack_json({ "timeout": 30 }) catch {
    Json(message, path~) => {
      assert_eq(message, "missing field 'script'")
      assert_eq(path, ["script"])
    }
    error => fail("unexpected error: \{repr(error)}")
  } noraise {
    _ => fail("expected the missing script to be rejected")
  }
  try
    command.input_schema.pack_json({ "script": "ls", "timeout": "30" })
  catch {
    Json(_, path~) => assert_eq(path, ["timeout"])
    error => fail("unexpected error: \{repr(error)}")
  } noraise {
    _ => fail("expected a string timeout to be rejected")
  }
  try
    command.input_schema.pack_json({ "script": "ls", "timeout": null })
  catch {
    Json(_, path~) => assert_eq(path, ["timeout"])
    error => fail("unexpected error: \{repr(error)}")
  } noraise {
    _ => fail("expected a null timeout to be rejected")
  }
  try command.input_schema.pack_json({ "script": "ls", "unknown": 1 }) catch {
    Json(message, path~) => {
      assert_eq(message, "unknown field 'unknown'")
      assert_eq(path, ["unknown"])
    }
    error => fail("unexpected error: \{repr(error)}")
  } noraise {
    _ => fail("expected an unknown argument to be rejected")
  }
}

///|
test "reflected tool JSON schema marks declared defaults" {
  let command = bash_tool().command([])
  let rendered = command.input_schema.to_json_schema(include_draft_marker=false)
  let plain = SchemaRef::new(command.wire_input).to_json_schema(
    include_draft_marker=false,
  )
  guard rendered is Object(fields) && plain is Object(plain_fields) else {
    fail("expected object schemas")
  }
  assert_eq(fields.get("required"), Some(Json::array([Json::string("script")])))
  guard fields.get("properties") is Some(Object(properties)) &&
    plain_fields.get("properties") is Some(Object(plain_properties)) else {
    fail("expected schema properties")
  }
  let defaults : Map[String, Json] = Map([])
  for name, property in properties {
    guard property is Object(property_fields) else {
      fail("expected an object property schema")
    }
    let without_default = property_fields.copy()
    match without_default.get("default") {
      Some(value) => defaults[name] = value
      None => ()
    }
    without_default.remove("default")
    assert_eq(Some(Json::object(without_default)), plain_properties.get(name))
  }
  json_inspect(defaults, content={
    "cwd": "",
    "timeout": 600,
    "shell": "bash",
    "login": false,
    "verbose": 0,
  })
  fields.remove("required")
  fields["properties"] = Json::object(plain_properties)
  plain_fields.remove("required")
  assert_eq(Json::object(fields), Json::object(plain_fields))
}

///|
test "reflected tool defaults leave the canonical wire input unchanged" {
  let command = bash_tool().command([])
  assert_eq(command.input_schema.graph, command.wire_input)
  assert_eq(
    command.input_schema.graph(),
    SchemaRef::new(command.wire_input).graph(),
  )
  guard command.wire_input.root.body is Record(fields) else {
    fail("expected a record input")
  }
  assert_eq(fields.map(field => field.name), [
    "script", "cwd", "timeout", "label", "shell", "login", "verbose",
  ])
  assert_eq(fields.map(field => field.body), [
    @model.schema_type(String),
    @model.schema_type(String),
    @model.schema_type(U32(None)),
    @model.schema_type(Option(@model.schema_type(String))),
    @model.schema_type(Option(@model.schema_type(String))),
    @model.schema_type(Bool),
    @model.schema_type(U32(None)),
  ])
  match command.checked_input(bash_input()) {
    Ok(typed) => assert_eq(typed.graph, command.wire_input)
    Err(_) => fail("expected the packed wire record to validate")
  }
  try
    command.input_schema.validate_value(
      @model.SchemaValue::Record([@model.SchemaValue::String("echo ok")]),
    )
  catch {
    Schema(_) => ()
    error => fail("unexpected error: \{repr(error)}")
  } noraise {
    _ => fail("expected the value surface to require every wire field")
  }
}

///|
test "reflected tool JSON does not fill repeatable options or tails" {
  let command = bash_tool(repeatables=true).command([])
  try command.input_schema.pack_json({ "script": "ls", "env": [] }) catch {
    Json(message, path~) => {
      assert_eq(message, "missing field 'args'")
      assert_eq(path, ["args"])
    }
    error => fail("unexpected error: \{repr(error)}")
  } noraise {
    _ => fail("expected an omitted tail to be rejected")
  }
  try command.input_schema.pack_json({ "script": "ls", "args": [] }) catch {
    Json(message, path~) => {
      assert_eq(message, "missing field 'env'")
      assert_eq(path, ["env"])
    }
    error => fail("unexpected error: \{repr(error)}")
  } noraise {
    _ => fail("expected an omitted repeatable option to be rejected")
  }
  assert_eq(
    expect_packed_json(command, { "script": "ls", "args": [], "env": [] }),
    @model.SchemaValue::Record([
      String("ls"),
      List([]),
      String(""),
      U32(600U),
      Option(None),
      Option(Some(String("bash"))),
      List([]),
      Bool(false),
      U32(0U),
    ]),
  )
  guard command.input_schema.to_json_schema() is Object(fields) &&
    fields.get("properties") is Some(Object(properties)) &&
    properties.get("env") is Some(Object(env)) else {
    fail("expected an object schema")
  }
  assert_eq(
    fields.get("required"),
    Some(
      Json::array([
        Json::string("script"),
        Json::string("args"),
        Json::string("env"),
      ]),
    ),
  )
  assert_false(env.contains("default"))
}
