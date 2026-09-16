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
