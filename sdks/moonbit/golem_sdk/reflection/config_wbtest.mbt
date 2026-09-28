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
    implemented_by: { uuid: { high_bits: 0UL, low_bits: 1UL, }, },
    constructor_input: string_schema,
    methods: [],
    config: [
      {
        path: ["greeting"],
        source: @common.AgentConfigSource::LOCAL,
        schema: string_schema,
      },
      {
        path: ["apiKey"],
        source: @common.AgentConfigSource::SECRET,
        schema: string_schema,
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
    Json(_) => ()
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
