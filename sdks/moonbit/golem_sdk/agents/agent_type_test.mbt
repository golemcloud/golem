// Phase 3a tests for agent-type assembly (`AgentTypeDef::to_agent_type`):
//   * all constructor / method / config schema graphs are merged into one
//     shared, deduplicated def pool,
//   * each schema root is installed as a `type-node-index` into that pool
//     (`NamedField.schema`, `OutputSchema::Single`, config `value_type`),
//   * auto-injected parameters encode as an empty-record placeholder and carry
//     the `AutoInjected` field source,
//   * `schema.root` is a structural placeholder, not a semantic root.

///|
/// A small nominal record graph: `test.Foo { a : s32 }`.
fn foo_graph() -> @model.SchemaGraph raise {
  let b = @model.SchemaBuilder::new()
  let root = b.register(
    "test.Foo",
    () => {
      @model.schema_type(
        @model.SchemaTypeBody::Record([
          @model.field("a", @model.schema_type(@model.SchemaTypeBody::S32)),
        ]),
      )
    },
    Some("Foo"),
  )
  b.build_graph(root)
}

///|
/// A primitive s32 graph (no named defs).
fn s32_graph() -> @model.SchemaGraph {
  { defs: [], root: @model.schema_type(@model.SchemaTypeBody::S32) }
}

///|
/// Assemble an agent type that reuses `test.Foo` in the constructor, a method
/// input, and a config declaration, plus an auto-injected principal parameter
/// and an s32 method output.
fn sample_agent_type() -> @common.AgentType raise {
  let def : AgentTypeDef = {
    type_name: "Sample",
    description: "sample agent",
    source_language: "moonbit",
    constructor_: {
      name: None,
      description: "ctor",
      prompt_hint: None,
      input: [
        { name: "foo", source: UserSupplied(foo_graph()) },
        { name: "principal", source: AutoInjected(PRINCIPAL) },
      ],
    },
    methods: [
      {
        name: "m",
        description: "method m",
        http_endpoint: [],
        prompt_hint: None,
        input: [{ name: "foo", source: UserSupplied(foo_graph()) }],
        output: Some(s32_graph()),
        read_only: None,
      },
    ],
    dependencies: [],
    mode: DURABLE,
    http_mount: None,
    snapshotting: Disabled,
    config: [{ source: LOCAL, path: ["a", "b"], value_type: foo_graph() }],
  }
  def.to_agent_type()
}

///|
test "merged def pool deduplicates shared types by id" {
  let at = sample_agent_type()
  // `test.Foo` appears in the constructor, the method input, and the config,
  // but is merged into a single def.
  inspect(at.schema.defs.length(), content="1")
  inspect(at.schema.defs[0].id, content="test.Foo")
}

///|
test "constructor input installs type-node-indices and field sources" {
  let at = sample_agent_type()
  let @common.InputSchema::Parameters(fields) = at.constructor_.input_schema
  inspect(fields.length(), content="2")
  inspect(fields[0].name, content="foo")
  inspect(fields[0].source, content="UserSupplied")
  inspect(fields[1].name, content="principal")
  inspect(fields[1].source, content="AutoInjected(PRINCIPAL)")
  // Distinct roots get distinct indices into the shared pool.
  inspect(fields[0].schema != fields[1].schema, content="true")
}

///|
test "all installed indices point into the single shared pool" {
  let at = sample_agent_type()
  let n = at.schema.type_nodes.length()
  let in_range = fn(i : Int) -> Bool { i >= 0 && i < n }
  let @common.InputSchema::Parameters(fields) = at.constructor_.input_schema
  // Every constructor field index resolves within the shared pool.
  for f in fields {
    assert_true(in_range(f.schema))
  }
  // The auto-injected parameter encodes an empty-record placeholder node.
  inspect(at.schema.type_nodes[fields[1].schema].body, content="RecordType([])")
  // The graph's own root is a structural empty-record sentinel (not a semantic root).
  assert_true(in_range(at.schema.root))
  inspect(at.schema.type_nodes[at.schema.root].body, content="RecordType([])")
  // Method output and config indices also resolve within the same pool.
  guard at.methods[0].output_schema is @common.OutputSchema::Single(out_idx)
  assert_true(in_range(out_idx))
  assert_true(in_range(at.config[0].value_type))
}

///|
test "method output and config encode into the shared pool" {
  let at = sample_agent_type()
  let method = at.methods[0]
  inspect(
    method.output_schema is @common.OutputSchema::Single(_),
    content="true",
  )
  let @common.InputSchema::Parameters(method_fields) = method.input_schema
  inspect(method_fields.length(), content="1")
  inspect(at.config.length(), content="1")
  inspect(at.config[0].path, content="[a, b]")
}

///|
test "unit method output encodes as OutputSchema::Unit" {
  let def : AgentTypeDef = {
    type_name: "Unit",
    description: "",
    source_language: "moonbit",
    constructor_: { name: None, description: "", prompt_hint: None, input: [] },
    methods: [
      {
        name: "noop",
        description: "",
        http_endpoint: [],
        prompt_hint: None,
        input: [],
        output: None,
        read_only: None,
      },
    ],
    dependencies: [],
    mode: DURABLE,
    http_mount: None,
    snapshotting: Disabled,
    config: [],
  }
  let at = def.to_agent_type()
  inspect(at.methods[0].output_schema, content="Unit")
}

///|
test "conflicting same-id definitions are rejected" {
  // Two graphs share id `test.Foo` but with structurally different bodies.
  let other : @model.SchemaGraph = {
    defs: [
      {
        id: "test.Foo",
        name: Some("Foo"),
        body: @model.schema_type(
          @model.SchemaTypeBody::Record([
            @model.field("a", @model.schema_type(@model.SchemaTypeBody::String)),
          ]),
        ),
      },
    ],
    root: @model.schema_type(@model.SchemaTypeBody::Ref("test.Foo")),
  }
  let def : AgentTypeDef = {
    type_name: "Conflict",
    description: "",
    source_language: "moonbit",
    constructor_: {
      name: None,
      description: "",
      prompt_hint: None,
      input: [
        { name: "a", source: UserSupplied(foo_graph()) },
        { name: "b", source: UserSupplied(other) },
      ],
    },
    methods: [],
    dependencies: [],
    mode: DURABLE,
    http_mount: None,
    snapshotting: Disabled,
    config: [],
  }
  let raised = try {
    let _ = def.to_agent_type()
    false
  } catch {
    _ => true
  }
  inspect(raised, content="true")
}
