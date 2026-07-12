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
  try decode_invocation_input(wire, 1, "tool command") catch {
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
    let fields = decode_invocation_input(wire, 1, "tool command")
    let _decoded : Int = decode_param(fields, 0, "tool command")
  } catch {
    @toolCommon.ToolError::InvalidInput(_) => ()
    _ => abort("expected invalid input")
  } noraise {
    _ => abort("expected invalid input")
  }
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
  try decode_invocation_input(wire, 1, "tool command") catch {
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
  let decoded = decode_canonical_input(def, 1, wire, "leaf")
  let force : Bool = decode_canonical_field(decoded, "f", "leaf")
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
  try decode_canonical_input(def, 1, wire, "leaf") catch {
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
  let decoded = decode_canonical_input(def, 0, wire, "echo")
  let value : String = decode_canonical_field(decoded, "value", "echo")
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
