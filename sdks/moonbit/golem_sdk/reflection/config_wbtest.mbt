///|
fn configured_reflected_type() -> AgentType {
  let string_schema = SchemaRef::new({
    defs: [],
    root: @model.schema_type(String),
  })
  {
    name: "ConfiguredAgent",
    description: "",
    source_language: "moonbit",
    mode: Durable,
    constructor_input: string_schema,
    methods: [],
    config: [
      {
        path: ["greeting"],
        source: @common.AgentConfigSource::LOCAL,
        schema: string_schema,
        default_value: None,
      },
      {
        path: ["apiKey"],
        source: @common.AgentConfigSource::SECRET,
        schema: string_schema,
        default_value: None,
      },
    ],
  }
}

///|
test "reflected config rejects unknown, secret, and invalid values before RPC" {
  let agent_type = configured_reflected_type()
  let accepted = agent_type.pack_config_json([
    { path: ["greeting"], value: Json::string("hello"), },
  ])
  assert_eq(accepted.length(), 1)
  try
    agent_type.pack_config_json([
      { path: ["unknown"], value: Json::string("x"), },
    ])
  catch {
    Schema(_) => ()
    error => fail("unexpected error: \{repr(error)}")
  } noraise {
    _ => fail("expected unknown config path to be rejected")
  }
  try
    agent_type.pack_config_json([
      { path: ["apiKey"], value: Json::string("x"), },
    ])
  catch {
    Schema(_) => ()
    error => fail("unexpected error: \{repr(error)}")
  } noraise {
    _ => fail("expected secret config override to be rejected")
  }
  try
    agent_type.validate_config_values([
      { path: ["greeting"], value: @model.SchemaValue::U32(42U), },
    ])
  catch {
    _ => ()
  } noraise {
    _ => fail("expected invalid schema-native config value to be rejected")
  }
}

///|
test "native config accepts large integers without a JSON round trip" {
  let agent_type = configured_reflected_type()
  agent_type.config.push({
    path: ["counter"],
    source: @common.AgentConfigSource::LOCAL,
    schema: SchemaRef::new({ defs: [], root: @model.schema_type(U64(None)), }),
    default_value: None,
  })
  let native = agent_type.validate_config_values([
    {
      path: ["counter"],
      value: @model.SchemaValue::U64(18446744073709551615UL),
    },
  ])
  assert_eq(native.length(), 1)
  try
    agent_type.pack_config_json([
      { path: ["counter"], value: Json::number(9007199254740992.0), },
    ])
  catch {
    Json(_, ..) => ()
    error => fail("unexpected error: \{repr(error)}")
  } noraise {
    _ => fail("expected JSON unsafe integer to be rejected")
  }
}

///|
test "agent snapshot accessors return defensive nested copies" {
  let agent_type = configured_reflected_type()
  let declarations = agent_type.config()
  let path = declarations[0].path()
  path[0] = "mutated"
  declarations[0] = declarations[1]
  let graph = agent_type.constructor_input().graph()
  graph.root.metadata.aliases.push("mutated")

  assert_eq(agent_type.config()[0].path(), ["greeting"])
  assert_eq(agent_type.constructor_input().root().metadata.aliases, [])
}

///|
test "reflected config preserves absent, null, and canonical JSON defaults" {
  let graph = @model.schema_graph_to_wit({
    defs: [],
    root: @model.schema_type(Option(@model.schema_type(String))),
  })
  let defaults : Array[String?] = [None, Some("null"), Some("\"árvíz\\n\"")]
  let config : Array[@common.AgentConfigDeclaration] = []
  for i, default_value in defaults {
    config.push({
      source: @common.AgentConfigSource::LOCAL,
      path: ["nested", "field\{i}"],
      value_type: graph.root,
      default_value,
    })
  }
  let reflected = decode_agent_type({
    agent_type: {
      type_name: "Defaults",
      kind: @common.AgentTypeKind::REGULAR,
      description: "",
      source_language: "moonbit",
      schema: graph,
      constructor_: {
        name: None,
        description: "",
        prompt_hint: None,
        input_schema: @common.InputSchema::Parameters([]),
      },
      methods: [],
      dependencies: [],
      mode: @common.AgentMode::DURABLE,
      http_mount: None,
      snapshotting: @common.Snapshotting::Disabled,
      config,
    },
    implemented_by: { uuid: { high_bits: 0UL, low_bits: 1UL, }, },
  })
  let declarations = reflected.config()
  for i, expected in defaults {
    assert_eq(declarations[i].default_value(), expected)
    assert_eq(declarations[i].path(), ["nested", "field\{i}"])
    assert_eq(declarations[i].source(), @common.AgentConfigSource::LOCAL)
    assert_eq(
      declarations[i].schema().graph(),
      @model.schema_graph_from_wit(graph),
    )
  }
}
