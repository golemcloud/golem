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
  {
    lookup_name: "sample",
    definition: {
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
    },
    implemented_by: { uuid: { high_bits: 0UL, low_bits: 0UL, }, },
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
test "reflected tool reports malformed remote result" {
  let command = sample_tool().command(["run"])
  match command.decode_result({ result: None, stdout: None, }) {
    Err(@tool.ToolError::MalformedRemoteOutput(_)) => ()
    _ => fail("expected missing tool result to be malformed")
  }
}
