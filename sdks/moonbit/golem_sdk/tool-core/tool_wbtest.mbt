///|
fn test_resource_drops() -> @model.SchemaValueResourceDrops {
  {
    drop_secret: fn(_) {  },
    drop_quota_token: fn(_) {  },
    drop_stream: fn(_) {  },
    drop_native_stream: fn(_) {  },
  }
}

///|
fn register_subtool_def_for_test(tool : ToolDef) -> Unit raise ToolBuildError {
  let name = match tool.tool_name() {
    Ok(name) => name
    Err(error) => raise error
  }
  ensure_tool_name_available(name)
  state.defs[name] = tool
}

///|
fn decode_canonical_input_for_test(
  def : ToolDef,
  command_index : Int,
  input : @types.TypedSchemaValue,
  context : String,
) -> Array[CanonicalInputValue] raise @toolCommon.ToolError {
  decode_canonical_input_with_resource_drops(
    def,
    command_index,
    input,
    context,
    test_resource_drops(),
  )
}

///|
fn decode_invocation_input_for_test(
  input : @types.TypedSchemaValue,
  arity : Int,
  context : String,
) -> Array[@model.SchemaValue] raise @toolCommon.ToolError {
  decode_invocation_input_with_resource_drops(
    input,
    arity,
    context,
    test_resource_drops(),
  )
}

///|
fn[T : @schema.IntoSchema + @schema.FromSchema] decode_canonical_field_for_test(
  fields : Array[CanonicalInputValue],
  name : String,
  context : String,
) -> T raise @toolCommon.ToolError {
  decode_canonical_field_with_resource_drops(
    fields,
    name,
    context,
    test_resource_drops(),
  )
}

///|
fn[T : @schema.FromSchema] decode_param_for_test(
  fields : Array[@model.SchemaValue],
  index : Int,
  context : String,
) -> T raise @toolCommon.ToolError {
  decode_param_with_resource_drops(
    fields,
    index,
    context,
    test_resource_drops(),
  )
}

///|
test "rejected tool envelopes drop reached stream resources" {
  let dropped = [0]
  let input : @types.TypedSchemaValue = {
    graph: @model.schema_graph_to_wit({
      defs: [],
      root: @model.schema_type(@model.SchemaTypeBody::Record([])),
    }),
    value: {
      value_nodes: [
        @types.StreamValue(@types.SchemaValueStream::SchemaValueStream(91)),
      ],
      root: 0,
    },
  }
  let result = try? decode_invocation_input_with_resource_drops(
    input,
    0,
    "tool cleanup",
    {
      drop_secret: fn(_) {  },
      drop_quota_token: fn(_) {  },
      drop_stream: fn(_) { dropped[0] += 1 },
      drop_native_stream: fn(_) {  },
    },
  )
  guard result is Err(@toolCommon.ToolError::InvalidInput(_)) else {
    fail("expected the schema/value mismatch to be rejected")
  }
  assert_eq(dropped[0], 1)
}

///|
fn doc(summary : String) -> @toolCommon.Doc {
  { summary, description: "details for " + summary, examples: [] }
}

///|
fn string_graph() -> @model.SchemaGraph raise {
  @schema.schema_graph_of_tag((@schema.type_tag() : @schema.TypeTag[String]))
}

///|
fn sample_tool() -> @toolCommon.Tool raise {
  let string = string_graph()
  let def : ToolDef = {
    version: "1.0.0",
    commands: [
      {
        name: "grep",
        aliases: [],
        doc: doc("search files"),
        globals: {
          options: [],
          flags: [
            {
              long: "verbose",
              short: Some('v'),
              aliases: ["chatty"],
              doc: doc("show verbose output"),
              shape: @toolCommon.FlagShape::BoolFlag({
                default: false,
                negatable: false,
              }),
              env_var: None,
            },
          ],
        },
        subcommands: [1],
        body: None,
      },
      {
        name: "run",
        aliases: [],
        doc: doc("run grep"),
        globals: { options: [], flags: [] },
        subcommands: [],
        body: Some({
          positionals: {
            fixed: [
              {
                name: "pattern",
                doc: doc("pattern to search"),
                value_name: None,
                type_: string,
                default: None,
                required: true,
                accepts_stdio: false,
              },
            ],
            tail: None,
          },
          options: [
            {
              long: "config",
              short: None,
              aliases: [],
              doc: doc("config file"),
              value_name: None,
              shape: OptionShapeDef::Scalar(string),
              default: None,
              required: false,
              env_var: None,
            },
          ],
          flags: [],
          constraints: [],
          stdin: None,
          stdout: None,
          result: None,
          errors: [],
          annotations: None,
        }),
      },
    ],
  }
  def.to_tool()
}

///|
fn[T] unwrap_tool_result(result : Result[T, ToolBuildError]) -> T {
  match result {
    Ok(value) => value
    Err(_) => abort("unexpected tool result error")
  }
}

///|
test "subtree-only tool cannot be invoked as a top-level tool" {
  let def = canonical_test_def()
  register_subtool_def_for_test(def)
  assert_true(
    get_registered_tool("root")
    is Err(@toolCommon.ToolError::InvalidToolName("root")),
  )
}

///|
test "forwarding matches short forms in their separate namespace" {
  let flag = fn(long : String) -> @toolCommon.FlagSpec {
    {
      long,
      short: Some('v'),
      aliases: [],
      doc: doc(long),
      shape: @toolCommon.FlagShape::BoolFlag({
        default: false,
        negatable: false,
      }),
      env_var: None,
    }
  }
  let child : ToolDef = {
    version: "1",
    commands: [
      {
        name: "short-forward-child",
        aliases: [],
        doc: doc("short forward child"),
        globals: { options: [], flags: [flag("child-verbose")] },
        subcommands: [],
        body: Some({
          positionals: { fixed: [], tail: None },
          options: [],
          flags: [],
          constraints: [],
          stdin: None,
          stdout: None,
          result: None,
          errors: [],
          annotations: None,
        }),
      },
    ],
  }
  let parent : ToolDef = {
    version: "1",
    commands: [
      {
        name: "short-forward-parent",
        aliases: [],
        doc: doc("short forward parent"),
        globals: { options: [], flags: [flag("parent-verbose")] },
        subcommands: [],
        body: None,
      },
    ],
  }
  let parent_fields = parent.canonical_input_fields(0)
  inspect(parent_fields.map(field => field.name), content="[parent-verbose]")
  assert_eq(parent_fields[0].short, Some('v'))
  let model = parent.canonical_input_model(0)
  let input = build_canonical_input(model, [
    ("parent-verbose", @model.SchemaValue::Bool(true)),
  ])
  let wire = @model.typed_schema_value_to_wit(input)
  let decoded = decode_canonical_input_for_test(parent, 0, wire, "short parent")
  let forwarded = forward_canonical_input(decoded, child, 0)
  let child_input = decode_canonical_input_for_test(
    child, 0, forwarded, "short child",
  )
  let verbose : Bool = decode_canonical_field_for_test(
    child_input, "child-verbose", "short child",
  )
  assert_true(verbose)
}

///|
test "forwarding keeps authored aliases separate from short forms" {
  let string = string_graph()
  let option = fn(
    long : String,
    aliases : Array[String],
    short : Char?,
  ) -> OptionSpecDef {
    {
      long,
      short,
      aliases,
      doc: doc(long),
      value_name: None,
      shape: OptionShapeDef::Scalar(string),
      default: None,
      required: true,
      env_var: None,
    }
  }
  let def : ToolDef = {
    version: "1",
    commands: [
      {
        name: "root",
        aliases: [],
        doc: doc("root"),
        globals: { options: [option("format", ["f"], None)], flags: [] },
        subcommands: [1],
        body: None,
      },
      {
        name: "run",
        aliases: [],
        doc: doc("run"),
        globals: { options: [], flags: [] },
        subcommands: [],
        body: Some({
          positionals: { fixed: [], tail: None },
          options: [option("file", [], Some('f'))],
          flags: [],
          constraints: [],
          stdin: None,
          stdout: None,
          result: None,
          errors: [],
          annotations: None,
        }),
      },
    ],
  }
  let normalized = def.normalize_inherited_globals()
  let model = normalized.canonical_input_model(1)
  let input = build_canonical_input(model, [
    ("format", @model.SchemaValue::String("json")),
    ("file", @model.SchemaValue::String("input.txt")),
  ])
  let decoded = decode_canonical_input_for_test(
    normalized,
    1,
    @model.typed_schema_value_to_wit(input),
    "source",
  )
  let forwarded = forward_canonical_input(decoded, normalized, 1)
  let forwarded = decode_canonical_input_for_test(
    normalized, 1, forwarded, "forwarded",
  )
  let format : String = decode_canonical_field_for_test(
    forwarded, "format", "forwarded",
  )
  let file : String = decode_canonical_field_for_test(
    forwarded, "file", "forwarded",
  )
  assert_eq(format, "json")
  assert_eq(file, "input.txt")
}

///|
test "forwarding prefers an exact name over an unrelated short form" {
  let string = string_graph()
  let option = fn(long : String, short : Char?) -> OptionSpecDef {
    {
      long,
      short,
      aliases: [],
      doc: doc(long),
      value_name: None,
      shape: OptionShapeDef::Scalar(string),
      default: None,
      required: true,
      env_var: None,
    }
  }
  let source : ToolDef = {
    version: "1",
    commands: [
      {
        name: "source",
        aliases: [],
        doc: doc("source"),
        globals: { options: [option("format", None)], flags: [] },
        subcommands: [],
        body: Some({
          positionals: { fixed: [], tail: None },
          options: [option("file", Some('f'))],
          flags: [],
          constraints: [],
          stdin: None,
          stdout: None,
          result: None,
          errors: [],
          annotations: None,
        }),
      },
    ],
  }
  let target : ToolDef = {
    version: "1",
    commands: [
      {
        name: "target",
        aliases: [],
        doc: doc("target"),
        globals: { options: [option("format", Some('f'))], flags: [] },
        subcommands: [],
        body: None,
      },
    ],
  }
  let input = build_canonical_input(source.canonical_input_model(0), [
    ("format", @model.SchemaValue::String("json")),
    ("file", @model.SchemaValue::String("input.txt")),
  ])
  let decoded = decode_canonical_input_for_test(
    source,
    0,
    @model.typed_schema_value_to_wit(input),
    "source",
  )
  let forwarded = forward_canonical_input(decoded, target, 0)
  let forwarded = decode_canonical_input_for_test(
    target, 0, forwarded, "target",
  )
  let format : String = decode_canonical_field_for_test(
    forwarded, "format", "target",
  )
  assert_eq(format, "json")
}

///|
test "grafting rejects child short collision with strict ancestor" {
  let flag = fn(long : String) -> @toolCommon.FlagSpec {
    {
      long,
      short: Some('v'),
      aliases: [],
      doc: doc(long),
      shape: @toolCommon.FlagShape::BoolFlag({
        default: false,
        negatable: false,
      }),
      env_var: None,
    }
  }
  let child : ToolDef = {
    version: "1",
    commands: [
      {
        name: "child",
        aliases: [],
        doc: doc("child"),
        globals: { options: [], flags: [flag("child-verbose")] },
        subcommands: [],
        body: None,
      },
    ],
  }
  let parent : ToolDef = {
    version: "1",
    commands: [
      {
        name: "parent",
        aliases: [],
        doc: doc("parent"),
        globals: { options: [], flags: [flag("ancestor-verbose")] },
        subcommands: [],
        body: None,
      },
    ],
  }
  try
    parent.graft_subtree(
      child,
      "child",
      { options: [], flags: [] },
      None,
      None,
      None,
    )
  catch {
    InheritedGlobalConflict(_, _) => ()
    _ => fail("unexpected graft error")
  } noraise {
    _ => fail("expected strict-ancestor short-form conflict")
  }
}

///|
test "grafting rejects distinct parent and child globals sharing only a short form" {
  let string = string_graph()
  let option = fn(long : String) -> OptionSpecDef {
    {
      long,
      short: Some('f'),
      aliases: [],
      doc: doc(long),
      value_name: None,
      shape: OptionShapeDef::Scalar(string),
      default: None,
      required: true,
      env_var: None,
    }
  }
  let parent : ToolDef = {
    version: "1",
    commands: [
      {
        name: "parent",
        aliases: [],
        doc: doc("parent"),
        globals: { options: [], flags: [] },
        subcommands: [],
        body: None,
      },
    ],
  }
  let child : ToolDef = {
    version: "1",
    commands: [
      {
        name: "child",
        aliases: [],
        doc: doc("child"),
        globals: { options: [option("format")], flags: [] },
        subcommands: [],
        body: None,
      },
    ],
  }
  try
    parent.graft_subtree(
      child,
      "child",
      { options: [option("count")], flags: [] },
      None,
      None,
      None,
    )
  catch {
    InheritedGlobalConflict(_, _) => ()
    _ => fail("unexpected graft error")
  } noraise {
    _ => fail("expected distinct short-form collision to be rejected")
  }
}

///|
test "grafting rejects a child global matching two inherited identities" {
  let string = string_graph()
  let option = fn(long : String, aliases : Array[String]) -> OptionSpecDef {
    {
      long,
      short: None,
      aliases,
      doc: doc(long),
      value_name: None,
      shape: OptionShapeDef::Scalar(string),
      default: None,
      required: true,
      env_var: None,
    }
  }
  let parent : ToolDef = {
    version: "1",
    commands: [
      {
        name: "parent",
        aliases: [],
        doc: doc("parent"),
        globals: { options: [option("alpha", [])], flags: [] },
        subcommands: [],
        body: None,
      },
    ],
  }
  let child : ToolDef = {
    version: "1",
    commands: [
      {
        name: "child",
        aliases: [],
        doc: doc("child"),
        globals: { options: [option("alpha", ["beta"])], flags: [] },
        subcommands: [],
        body: None,
      },
    ],
  }
  try
    parent.graft_subtree(
      child,
      "child",
      { options: [option("beta", [])], flags: [] },
      None,
      None,
      None,
    )
  catch {
    InheritedGlobalConflict(_, _) => ()
    _ => fail("unexpected graft error")
  } noraise {
    _ => fail("expected ambiguous inherited identity conflict")
  }
}

///|
test "deprojected child global does not consume a descendant local input" {
  let string = string_graph()
  let option = fn(
    long : String,
    aliases : Array[String],
    short : Char?,
  ) -> OptionSpecDef {
    {
      long,
      short,
      aliases,
      doc: doc(long),
      value_name: None,
      shape: OptionShapeDef::Scalar(string),
      default: None,
      required: true,
      env_var: None,
    }
  }
  let child_raw : ToolDef = {
    version: "1",
    commands: [
      {
        name: "deproject-child",
        aliases: [],
        doc: doc("deproject child"),
        globals: { options: [option("child-global", [], Some('f'))], flags: [] },
        subcommands: [1],
        body: Some({
          positionals: { fixed: [], tail: None },
          options: [],
          flags: [],
          constraints: [],
          stdin: None,
          stdout: None,
          result: None,
          errors: [],
          annotations: None,
        }),
      },
      {
        name: "run",
        aliases: [],
        doc: doc("run"),
        globals: { options: [], flags: [] },
        subcommands: [],
        body: Some({
          positionals: { fixed: [], tail: None },
          options: [option("format", [], None)],
          flags: [],
          constraints: [],
          stdin: None,
          stdout: None,
          result: None,
          errors: [],
          annotations: None,
        }),
      },
    ],
  }
  let child = child_raw.normalize_inherited_globals()
  let parent_base : ToolDef = {
    version: "1",
    commands: [
      {
        name: "deproject-parent",
        aliases: [],
        doc: doc("deproject parent"),
        globals: { options: [], flags: [] },
        subcommands: [],
        body: None,
      },
    ],
  }
  let parent = parent_base
    .graft_subtree(
      child_raw,
      "deproject-child",
      { options: [option("count", ["child-global"], Some('f'))], flags: [] },
      None,
      None,
      None,
    )
    .normalize_inherited_globals()
  let parent_index = parent.command_index_by_path(["deproject-child", "run"])
  guard parent_index is Some(parent_index) else {
    fail("expected grafted descendant command")
  }
  inspect(
    parent.canonical_input_fields(parent_index).map(field => field.name),
    content="[count, format]",
  )
  let model = parent.canonical_input_model(parent_index)
  let input = build_canonical_input(model, [
    ("count", @model.SchemaValue::String("inherited")),
    ("format", @model.SchemaValue::String("local")),
  ])
  let wire = @model.typed_schema_value_to_wit(input)
  let fields = decode_canonical_input_for_test(
    parent, parent_index, wire, "deproject parent",
  )
  let child_index = child.command_index_by_path(["run"])
  guard child_index is Some(child_index) else {
    fail("expected child descendant command")
  }
  let forwarded = forward_canonical_input(fields, child, child_index)
  let child_input = decode_canonical_input_for_test(
    child, child_index, forwarded, "deproject child",
  )
  let format : String = decode_canonical_field_for_test(
    child_input, "format", "deproject child",
  )
  assert_eq(format, "local")
}

///|
test "bug finder forwarding prefers surviving descendant over ancestor alias" {
  let string = string_graph()
  let option = fn(long : String, aliases : Array[String]) -> OptionSpecDef {
    {
      long,
      short: None,
      aliases,
      doc: doc(long),
      value_name: None,
      shape: OptionShapeDef::Scalar(string),
      default: None,
      required: true,
      env_var: None,
    }
  }
  let child_raw : ToolDef = {
    version: "1",
    commands: [
      {
        name: "ranked-forward-child",
        aliases: [],
        doc: doc("ranked forward child"),
        globals: { options: [option("format", ["leaf"])], flags: [] },
        subcommands: [1],
        body: None,
      },
      {
        name: "run",
        aliases: [],
        doc: doc("run"),
        globals: { options: [], flags: [] },
        subcommands: [],
        body: Some({
          positionals: { fixed: [], tail: None },
          options: [option("leaf", [])],
          flags: [],
          constraints: [],
          stdin: None,
          stdout: None,
          result: None,
          errors: [],
          annotations: None,
        }),
      },
    ],
  }
  let child = child_raw.normalize_inherited_globals()
  inspect(
    child.canonical_input_fields(1).map(field => field.name),
    content="[format]",
  )
  let parent_base : ToolDef = {
    version: "1",
    commands: [
      {
        name: "ranked-forward-parent",
        aliases: [],
        doc: doc("ranked forward parent"),
        globals: { options: [], flags: [] },
        subcommands: [],
        body: None,
      },
    ],
  }
  let parent = parent_base
    .graft_subtree(
      child_raw,
      "ranked-forward-child",
      { options: [option("parent-value", ["format"])], flags: [] },
      None,
      None,
      None,
    )
    .normalize_inherited_globals()
  let parent_index = parent.command_index_by_path([
    "ranked-forward-child", "run",
  ])
  guard parent_index is Some(parent_index) else {
    fail("expected grafted descendant command")
  }
  inspect(
    parent.canonical_input_fields(parent_index).map(field => field.name),
    content="[parent-value, leaf]",
  )
  let model = parent.canonical_input_model(parent_index)
  let input = build_canonical_input(model, [
    ("parent-value", @model.SchemaValue::String("ancestor")),
    ("leaf", @model.SchemaValue::String("local")),
  ])
  let fields = decode_canonical_input_for_test(
    parent,
    parent_index,
    @model.typed_schema_value_to_wit(input),
    "ranked parent",
  )
  let child_index = child.command_index_by_path(["run"])
  guard child_index is Some(child_index) else {
    fail("expected child descendant command")
  }
  let forwarded = forward_canonical_input(fields, child, child_index)
  let child_input = decode_canonical_input_for_test(
    child, child_index, forwarded, "ranked child",
  )
  let leaf : String = decode_canonical_field_for_test(
    child_input, "format", "ranked child",
  )
  assert_eq(leaf, "local")
}

///|
test "reverse-alias deprojection retains the surviving canonical value" {
  let string = string_graph()
  let option = fn(long : String, aliases : Array[String]) -> OptionSpecDef {
    {
      long,
      short: None,
      aliases,
      doc: doc(long),
      value_name: None,
      shape: OptionShapeDef::Scalar(string),
      default: None,
      required: true,
      env_var: None,
    }
  }
  let raw : ToolDef = {
    version: "1",
    commands: [
      {
        name: "reverse-alias",
        aliases: [],
        doc: doc("reverse alias"),
        globals: { options: [option("format", [])], flags: [] },
        subcommands: [1],
        body: None,
      },
      {
        name: "run",
        aliases: [],
        doc: doc("run"),
        globals: { options: [option("count", ["format"])], flags: [] },
        subcommands: [],
        body: Some({
          positionals: { fixed: [], tail: None },
          options: [],
          flags: [],
          constraints: [],
          stdin: None,
          stdout: None,
          result: None,
          errors: [],
          annotations: None,
        }),
      },
    ],
  }
  let def = raw.normalize_inherited_globals()
  inspect(
    def.canonical_input_fields(1).map(field => field.name),
    content="[format]",
  )
  let input = build_canonical_input(def.canonical_input_model(1), [
    ("format", @model.SchemaValue::String("7")),
  ])
  let decoded = decode_canonical_input_for_test(
    def,
    1,
    @model.typed_schema_value_to_wit(input),
    "reverse alias",
  )
  let format : String = decode_canonical_field_for_test(
    decoded, "format", "reverse alias",
  ) catch {
    _ => fail("surviving canonical field was not decoded")
  }
  inspect(format, content="7")
}

///|
test "panic top-level registration rejects an existing private subtool name" {
  let def : ToolDef = {
    version: "1",
    commands: [
      {
        name: "registry-collision",
        aliases: [],
        doc: doc("registry collision"),
        globals: { options: [], flags: [] },
        subcommands: [],
        body: None,
      },
    ],
  }
  register_subtool_def_for_test(def)
  register_tool_def(def)
}

///|
test "help renders command names and inherited globals" {
  let tool = sample_tool()
  let root = unwrap_tool_result(render_help(tool, []))
  assert_true(root.contains("Usage: grep"))
  assert_true(root.contains("Globals:"))
  assert_true(root.contains("--verbose"))
  assert_true(root.contains("Subcommands:"))
  assert_true(root.contains("run"))
  let run = unwrap_tool_result(render_help(tool, ["run"]))
  assert_true(run.contains("Usage: run"))
  assert_true(run.contains("--config"))
  assert_true(run.contains("pattern"))
}

///|
test "argument help searches globals and body arguments" {
  let tool = sample_tool()
  let verbose = unwrap_tool_result(
    render_argument_help(tool, ["run"], "chatty"),
  )
  assert_true(verbose.contains("--verbose (flag, global)"))
  let pattern = unwrap_tool_result(
    render_argument_help(tool, ["run"], "pattern"),
  )
  assert_true(pattern.contains("pattern (positional, required)"))
}

///|
test "decode invocation input rejects graph root that is not a record" {
  let graph = string_graph()
  let typed = encode_invocation_input(graph, [@model.SchemaValue::String("x")])
  let wire = @model.typed_schema_value_to_wit(typed)
  try decode_invocation_input_for_test(wire, 1, "tool command") catch {
    @toolCommon.ToolError::InvalidInput(_) => ()
    _ => abort("expected invalid input")
  } noraise {
    _ => abort("expected invalid input")
  }
}

///|
test "decode invocation input rejects value that does not match typed schema field" {
  let graph : @model.SchemaGraph = {
    defs: [],
    root: @model.schema_type(
      @model.SchemaTypeBody::Record([
        @model.field("arg", @model.schema_type(@model.SchemaTypeBody::String)),
      ]),
    ),
  }
  let typed : @model.TypedSchemaValue = {
    graph,
    value: @model.SchemaValue::Record([@model.SchemaValue::S32(7)]),
  }
  let wire = @model.typed_schema_value_to_wit(typed)
  try {
    let fields = decode_invocation_input_for_test(wire, 1, "tool command")
    let _decoded : Int = decode_param_for_test(fields, 0, "tool command")
  } catch {
    @toolCommon.ToolError::InvalidInput(_) => ()
    _ => abort("expected invalid input")
  } noraise {
    _ => abort("expected invalid input")
  }
}

///|
test "decode invocation input accepts a stream value" {
  let graph : @model.SchemaGraph = {
    defs: [],
    root: @model.schema_type(
      @model.SchemaTypeBody::Record([
        @model.field(
          "stream",
          @model.schema_type(@model.SchemaTypeBody::Stream(None)),
        ),
      ]),
    ),
  }
  let handle = @model.GuestSchemaValueStreamHandle::from_wrapped(
    @types.SchemaValueStream::SchemaValueStream(92),
  )
  let typed : @model.TypedSchemaValue = {
    graph,
    value: @model.SchemaValue::Record([@model.SchemaValue::Stream(handle)]),
  }
  let decoded = decode_invocation_input_for_test(
    @model.typed_schema_value_to_wit(typed),
    1,
    "stream tool command",
  )
  guard decoded[0] is @model.SchemaValue::Stream(decoded_handle) else {
    fail("expected a decoded stream value")
  }
  assert_true(decoded_handle.is_present())
  @model.SchemaValue::Record(decoded).release_resources(test_resource_drops())
}

///|
test "decode invocation input rejects union value whose discriminator does not match branch" {
  let tagged_record = @model.schema_type(
    @model.SchemaTypeBody::Record([
      @model.field("kind", @model.schema_type(@model.SchemaTypeBody::String)),
    ]),
  )
  let graph : @model.SchemaGraph = {
    defs: [],
    root: @model.schema_type(
      @model.SchemaTypeBody::Record([
        @model.field(
          "arg",
          @model.schema_type(
            @model.SchemaTypeBody::Union([
              {
                tag: "a",
                body: tagged_record,
                discriminator: @types.DiscriminatorRule::FieldEquals({
                  field_name: "kind",
                  literal: Some("a"),
                }),
                metadata: @model.empty_metadata(),
              },
            ]),
          ),
        ),
      ]),
    ),
  }
  let typed : @model.TypedSchemaValue = {
    graph,
    value: @model.SchemaValue::Record([
      @model.SchemaValue::Union(
        "a",
        @model.SchemaValue::Record([@model.SchemaValue::String("b")]),
      ),
    ]),
  }
  let wire = @model.typed_schema_value_to_wit(typed)
  try decode_invocation_input_for_test(wire, 1, "tool command") catch {
    @toolCommon.ToolError::InvalidInput(_) => ()
    _ => abort("expected invalid input")
  } noraise {
    _ => abort("expected invalid input")
  }
}

///|
test "normalization rejects a local field overlapping two inherited globals" {
  let string = string_graph()
  let option = fn(long : String, aliases : Array[String]) -> OptionSpecDef {
    {
      long,
      short: None,
      aliases,
      doc: doc(long),
      value_name: None,
      shape: OptionShapeDef::Scalar(string),
      default: None,
      required: true,
      env_var: None,
    }
  }
  let def : ToolDef = {
    version: "1",
    commands: [
      {
        name: "root",
        aliases: [],
        doc: doc("root"),
        globals: { options: [option("alpha", [])], flags: [] },
        subcommands: [1],
        body: None,
      },
      {
        name: "child",
        aliases: [],
        doc: doc("child"),
        globals: { options: [option("beta", [])], flags: [] },
        subcommands: [2],
        body: None,
      },
      {
        name: "leaf",
        aliases: [],
        doc: doc("leaf"),
        globals: { options: [], flags: [] },
        subcommands: [],
        body: Some({
          positionals: { fixed: [], tail: None },
          options: [option("alpha", ["beta"])],
          flags: [],
          constraints: [],
          stdin: None,
          stdout: None,
          result: None,
          errors: [],
          annotations: None,
        }),
      },
    ],
  }
  try def.normalize_inherited_globals() catch {
    InheritedGlobalConflict(_, _) => ()
    _ => abort("unexpected normalization error")
  } noraise {
    _ => abort("expected inherited global conflict")
  }
}

///|
test "normalization rejects distinct options sharing an inherited short form" {
  let string = string_graph()
  let option = fn(long : String) -> OptionSpecDef {
    {
      long,
      short: Some('f'),
      aliases: [],
      doc: doc(long),
      value_name: None,
      shape: OptionShapeDef::Scalar(string),
      default: None,
      required: true,
      env_var: None,
    }
  }
  let def : ToolDef = {
    version: "1",
    commands: [
      {
        name: "root",
        aliases: [],
        doc: doc("root"),
        globals: { options: [option("format")], flags: [] },
        subcommands: [1],
        body: None,
      },
      {
        name: "child",
        aliases: [],
        doc: doc("child"),
        globals: { options: [], flags: [] },
        subcommands: [],
        body: Some({
          positionals: { fixed: [], tail: None },
          options: [option("file")],
          flags: [],
          constraints: [],
          stdin: None,
          stdout: None,
          result: None,
          errors: [],
          annotations: None,
        }),
      },
    ],
  }
  try def.normalize_inherited_globals() catch {
    _ => ()
  } noraise {
    _ => fail("expected inherited short-form conflict")
  }
}

///|
test "normalization resolves value-is literals against inherited globals" {
  let string = string_graph()
  let format : OptionSpecDef = {
    long: "format",
    short: None,
    aliases: ["f"],
    doc: doc("format"),
    value_name: None,
    shape: OptionShapeDef::Scalar(string),
    default: None,
    required: false,
    env_var: None,
  }
  let def : ToolDef = {
    version: "1",
    commands: [
      {
        name: "root",
        aliases: [],
        doc: doc("root"),
        globals: { options: [format], flags: [] },
        subcommands: [1],
        body: None,
      },
      {
        name: "render",
        aliases: [],
        doc: doc("render"),
        globals: { options: [], flags: [] },
        subcommands: [],
        body: Some({
          positionals: { fixed: [], tail: None },
          options: [],
          flags: [],
          constraints: [
            ConstraintDef::RequiresAll([
              RefDef::ValueIs({
                name: "f",
                value: ValueIsLiteralDef::Deferred(ToolLiteral::Scalar("json")),
              }),
            ]),
          ],
          stdin: None,
          stdout: None,
          result: None,
          errors: [],
          annotations: None,
        }),
      },
    ],
  }
  let normalized = def.normalize_inherited_globals()
  guard normalized.commands[1].body is Some(body) else {
    fail("expected command body")
  }
  guard body.constraints is [ConstraintDef::RequiresAll([ref_])] else {
    fail("expected requires-all constraint")
  }
  guard ref_
    is RefDef::ValueIs(
      {
        name: "format",
        value: ValueIsLiteralDef::Resolved(@model.SchemaValue::String("json")),
      }
    ) else {
    fail("expected resolved value-is constraint")
  }
  let _wire = normalized.to_tool()
}

///|
fn canonical_test_def() -> ToolDef raise {
  let string = string_graph()
  let map = @schema.schema_graph_of_tag(
    (@schema.type_tag() : @schema.TypeTag[Map[String, UInt]]),
  )
  let scalar = fn(long, aliases) -> OptionSpecDef {
    {
      long,
      short: None,
      aliases,
      doc: doc(long),
      value_name: None,
      shape: OptionShapeDef::Scalar(string),
      default: None,
      required: false,
      env_var: None,
    }
  }
  {
    version: "1",
    commands: [
      {
        name: "root",
        aliases: [],
        doc: doc("root"),
        globals: {
          options: [scalar("global", []), scalar("shadow", [])],
          flags: [
            {
              long: "count",
              short: None,
              aliases: ["c"],
              doc: doc("count"),
              shape: @toolCommon.FlagShape::CountFlag(None),
              env_var: None,
            },
          ],
        },
        subcommands: [1],
        body: None,
      },
      {
        name: "leaf",
        aliases: ["l"],
        doc: doc("leaf"),
        globals: { options: [], flags: [] },
        subcommands: [],
        body: Some({
          positionals: {
            fixed: [
              {
                name: "arg",
                doc: doc("arg"),
                value_name: None,
                type_: string,
                default: None,
                required: true,
                accepts_stdio: false,
              },
            ],
            tail: Some({
              name: "rest",
              doc: doc("rest"),
              value_name: None,
              item_type: string,
              min: 0,
              max: None,
              separator: None,
              verbatim: false,
              accepts_stdio: false,
            }),
          },
          options: [
            {
              long: "items",
              short: None,
              aliases: ["shadow"],
              doc: doc("items"),
              value_name: None,
              shape: OptionShapeDef::RepeatableList({
                repetition: @toolCommon.Repetition::Repeated,
                item_type: string,
              }),
              default: None,
              required: false,
              env_var: None,
            },
            {
              long: "values",
              short: None,
              aliases: [],
              doc: doc("values"),
              value_name: None,
              shape: OptionShapeDef::RepeatableMap({
                repetition: @toolCommon.Repetition::Repeated,
                map_type: map,
                duplicate_key_policy: @toolCommon.DuplicateKeyPolicy::LAST_WINS,
              }),
              default: None,
              required: false,
              env_var: None,
            },
          ],
          flags: [
            {
              long: "force",
              short: None,
              aliases: ["f"],
              doc: doc("force"),
              shape: @toolCommon.FlagShape::BoolFlag({
                default: false,
                negatable: false,
              }),
              env_var: None,
            },
          ],
          constraints: [],
          stdin: None,
          stdout: None,
          result: None,
          errors: [],
          annotations: None,
        }),
      },
    ],
  }
}

///|
test "canonical command lookup, ordering, shadowing, and collected shapes" {
  let def = canonical_test_def()
  inspect(def.command_index_by_path(["l"]), content="Some(1)")
  let fields = def.canonical_input_fields(1)
  inspect(
    fields.map(field => field.name),
    content="[global, count, arg, rest, items, values, force]",
  )
  assert_true(fields[0].type_.root.body is @model.SchemaTypeBody::String)
  assert_true(fields[1].type_.root.body is @model.SchemaTypeBody::U32(_))
  assert_true(fields[3].type_.root.body is @model.SchemaTypeBody::List(_))
  assert_true(fields[4].type_.root.body is @model.SchemaTypeBody::List(_))
  assert_true(fields[5].type_.root.body is @model.SchemaTypeBody::Map(_, _))
  assert_true(fields[6].type_.root.body is @model.SchemaTypeBody::Bool)
}

///|
test "canonical model builds and decodes the expected named record" {
  let def = canonical_test_def()
  let model = def.canonical_input_model(1)
  guard model.record_graph.root.body
    is @model.SchemaTypeBody::Record(record_fields) else {
    fail("expected canonical record")
  }
  inspect(
    record_fields.map(field => field.name),
    content="[global, count, arg, rest, items, values, force]",
  )
  let input = build_canonical_input(model, [
    ("force", @model.SchemaValue::Bool(true)),
    ("values", @model.SchemaValue::Map([])),
    ("items", @model.SchemaValue::List([])),
    ("rest", @model.SchemaValue::List([])),
    ("arg", @model.SchemaValue::String("arg")),
    ("count", @model.SchemaValue::U32(1)),
    ("global", @model.SchemaValue::String("old")),
    ("global", @model.SchemaValue::String("new")),
  ])
  guard input.value is @model.SchemaValue::Record(values) else {
    fail("expected record value")
  }
  inspect(values[0], content="String(new)")
  let wire = @model.typed_schema_value_to_wit(input)
  let decoded = decode_canonical_input_for_test(def, 1, wire, "leaf")
  let force : Bool = decode_canonical_field_for_test(decoded, "f", "leaf")
  assert_true(force)
}

///|
test "canonical input rejects missing fields and mismatching wire schema" {
  let def = canonical_test_def()
  let model = def.canonical_input_model(1)
  try build_canonical_input(model, []) catch {
    MissingCanonicalInputField("global") => ()
    _ => fail("expected missing canonical field")
  } noraise {
    _ => fail("expected missing canonical field")
  }
  let mismatching : @model.TypedSchemaValue = {
    graph: string_graph(),
    value: @model.SchemaValue::String("not a record"),
  }
  let wire = @model.typed_schema_value_to_wit(mismatching)
  try decode_canonical_input_for_test(def, 1, wire, "leaf") catch {
    @toolCommon.ToolError::InvalidInput(_) => ()
    _ => fail("expected invalid canonical input")
  } noraise {
    _ => fail("expected invalid canonical input")
  }
}

///|
fn literal_graph(body : @model.SchemaTypeBody) -> @model.SchemaGraph {
  { defs: [], root: @model.schema_type(body) }
}

///|
test "tool literals use the target schema" {
  inspect(
    literal_to_schema_value(
      literal_graph(@model.SchemaTypeBody::Enum(["always", "auto"])),
      ToolLiteral::Scalar("auto"),
    ),
    content="Enum(1)",
  )
  inspect(
    literal_to_schema_value(
      literal_graph(@model.SchemaTypeBody::String),
      ToolLiteral::Scalar("42"),
    ),
    content="String(42)",
  )
  inspect(
    literal_to_schema_value(
      literal_graph(@model.SchemaTypeBody::U64(None)),
      ToolLiteral::Scalar("18446744073709551615"),
    ),
    content="U64(18446744073709551615)",
  )
  inspect(
    literal_to_schema_value(
      literal_graph(
        @model.SchemaTypeBody::List(
          @model.schema_type(@model.SchemaTypeBody::S32(None)),
        ),
      ),
      ToolLiteral::List(["1", "-2"]),
    ),
    content="List([S32(1), S32(-2)])",
  )
  inspect(
    literal_to_schema_value(
      literal_graph(
        @model.SchemaTypeBody::Map(
          @model.schema_type(@model.SchemaTypeBody::String),
          @model.schema_type(@model.SchemaTypeBody::Bool),
        ),
      ),
      ToolLiteral::Map([("enabled", "true")]),
    ),
    content="Map([{key: String(enabled), value: Bool(true)}])",
  )
}

///|
test "invalid literals and value-is element fallback" {
  try
    literal_to_schema_value(
      literal_graph(@model.SchemaTypeBody::U8(None)),
      ToolLiteral::Scalar("256"),
    )
  catch {
    InvalidLiteral("u8 literal '256' is out of range") => ()
    _ => fail("unexpected error")
  } noraise {
    _ => fail("expected invalid literal")
  }
  inspect(
    value_is_literal_to_schema_value(
      literal_graph(
        @model.SchemaTypeBody::Option(
          @model.schema_type(
            @model.SchemaTypeBody::List(
              @model.schema_type(@model.SchemaTypeBody::String),
            ),
          ),
        ),
      ),
      ToolLiteral::Scalar("needle"),
    ),
    content="String(needle)",
  )
}

///|
test "text path and url refinements overlay restrictions" {
  let text = refine_text(
    literal_graph(@model.SchemaTypeBody::String),
    Some("^[a-z]+$"),
    Some(2),
    Some(8),
  )
  inspect(
    text.root.body,
    content=(
      #|Text({languages: None, min_length: Some(2), max_length: Some(8), regex: Some(^[a-z]+$)})
    ),
  )
  let path = refine_path(
    literal_graph(
      @model.SchemaTypeBody::Path({
        direction: @types.PathDirection::INPUT,
        kind: @types.PathKind::ANY,
        allowed_mime_types: None,
        allowed_extensions: Some(["txt"]),
      }),
    ),
    Some(@types.PathDirection::OUTPUT),
    Some(@types.PathKind::FILE),
    Some(["text/plain"]),
  )
  inspect(
    path.root.body,
    content=(
      #|Path({direction: OUTPUT, kind: FILE, allowed_mime_types: Some([text/plain]), allowed_extensions: Some([txt])})
    ),
  )
  let url = refine_url(
    literal_graph(
      @model.SchemaTypeBody::Url({
        allowed_schemes: None,
        allowed_hosts: Some(["example.com"]),
      }),
    ),
    Some(["https"]),
  )
  inspect(
    url.root.body,
    content=(
      #|Url({allowed_schemes: Some([https]), allowed_hosts: Some([example.com])})
    ),
  )
}

///|
test "text-refined string canonical input round-trips through typed values" {
  let refined = refine_text(string_graph(), Some("^[a-z]+$"), None, None)
  let def : ToolDef = {
    version: "1",
    commands: [
      {
        name: "echo",
        aliases: [],
        doc: doc("echo"),
        globals: { options: [], flags: [] },
        subcommands: [],
        body: Some({
          positionals: {
            fixed: [
              {
                name: "value",
                doc: doc("value"),
                value_name: None,
                type_: refined,
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
          result: None,
          errors: [],
          annotations: None,
        }),
      },
    ],
  }
  let model = def.canonical_input_model(0)
  let input = build_canonical_input(model, [
    ("value", @schema.to_value_as("aaa")),
  ])
  let wire = @model.typed_schema_value_to_wit(input)
  let decoded = decode_canonical_input_for_test(def, 0, wire, "echo")
  let value : String = decode_canonical_field_for_test(decoded, "value", "echo")
  inspect(value, content="aaa")
}

///|
test "numeric refinements infer kind and reject invalid ranges" {
  let numeric = refine_numeric(
    literal_graph(@model.SchemaTypeBody::U64(None)),
    Some("1"),
    Some("18446744073709551615"),
    Some("bytes"),
  )
  inspect(
    numeric.root.body,
    content=(
      #|U64(Some({min: Some(Unsigned(1)), max: Some(Unsigned(18446744073709551615)), unit: Some(bytes)}))
    ),
  )
  try
    refine_numeric(
      literal_graph(@model.SchemaTypeBody::S32(None)),
      Some("10"),
      Some("2"),
      None,
    )
  catch {
    InvalidNumericBound("numeric minimum exceeds maximum") => ()
    _ => fail("unexpected error")
  } noraise {
    _ => fail("expected invalid numeric range")
  }
  try
    refine_text(literal_graph(@model.SchemaTypeBody::Bool), None, None, None)
  catch {
    RefinementTypeMismatch("text", "bool") => ()
    _ => fail("unexpected error")
  } noraise {
    _ => fail("expected refinement mismatch")
  }
}
