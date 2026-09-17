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
      ],
      tail: None,
    },
    options: [],
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
test "reflected tool reports malformed remote result" {
  let command = sample_tool().command(["run"])
  match command.decode_result({ result: None, stdout: None, }) {
    Err(@tool.ToolError::MalformedRemoteOutput(_)) => ()
    _ => fail("expected missing tool result to be malformed")
  }
}
