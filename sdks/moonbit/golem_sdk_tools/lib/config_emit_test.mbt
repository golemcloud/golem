///|
test "generate config field impls targets the new schema-model API" {
  let config_types : Array[ConfigTypeInfo] = [
    {
      name: "DatabaseConfig",
      package_path: "",
      field_metadata: {},
      fields: [
        ("host", Simple("String")),
        ("port", Simple("UInt")),
        ("timeout", Simple("UInt64")),
      ],
    },
  ]
  let impls = generate_config_field_impls(config_types)
  let output = @formatter.impls_to_string(impls)
  debug_inspect(
    output,
    content=(
      #|"///|\n///\nimpl @config.ConfigField for DatabaseConfig with fn collect_entries(path) {\n  let entries :Array[@agents.AgentConfigDef] = []\n  let fp0 = path.copy()\n  fp0.push(\"host\")\n  entries.append(\n    @config.local_entry((@schema.TypeTag::{  } : @schema.TypeTag[String]), fp0),\n  )\n  let fp1 = path.copy()\n  fp1.push(\"port\")\n  entries.append(\n    @config.local_entry((@schema.TypeTag::{  } : @schema.TypeTag[UInt]), fp1),\n  )\n  let fp2 = path.copy()\n  fp2.push(\"timeout\")\n  entries.append(\n    @config.local_entry((@schema.TypeTag::{  } : @schema.TypeTag[UInt64]), fp2),\n  )\n  entries\n}\n\n///|\n///\nimpl @config.WireConfigField for DatabaseConfig with fn append_wire_entries(\n  encoder,\n  path,\n) {\n  let entries :Array[@common.AgentConfigDeclaration] = []\n  let fp0 = path.copy()\n  fp0.push(\"host\")\n  entries.append(\n    @config.wire_local_entry(\n      encoder,\n      (@schema.TypeTag::{  } : @schema.TypeTag[String]),\n      fp0,\n    ),\n  )\n  let fp1 = path.copy()\n  fp1.push(\"port\")\n  entries.append(\n    @config.wire_local_entry(\n      encoder,\n      (@schema.TypeTag::{  } : @schema.TypeTag[UInt]),\n      fp1,\n    ),\n  )\n  let fp2 = path.copy()\n  fp2.push(\"timeout\")\n  entries.append(\n    @config.wire_local_entry(\n      encoder,\n      (@schema.TypeTag::{  } : @schema.TypeTag[UInt64]),\n      fp2,\n    ),\n  )\n  entries\n}\n\n///|\n///\nimpl @config.ConfigField for DatabaseConfig with fn load(path) {\n  let fp0 = path.copy()\n  fp0.push(\"host\")\n  let host :String = @config.load_config_value(fp0)\n  let fp1 = path.copy()\n  fp1.push(\"port\")\n  let port :UInt = @config.load_config_value(fp1)\n  let fp2 = path.copy()\n  fp2.push(\"timeout\")\n  let timeout :UInt64 = @config.load_config_value(fp2)\n  { host, port, timeout }\n}\n\n"
    ),
  )
}

///|
test "config defaults are portable metadata and names agree across access paths" {
  let source =
    #|#derive.config
    #|struct App {
    #|  #derive.config(default="0")
    #|  max_chars : UInt
    #|  #derive.config(name="Settings/Exact", default="{\"label\":\"code-default\",\"count\":31}")
    #|  settings : Settings
    #|  #derive.config(default="null")
    #|  cleared : String?
    #|  omitted : String?
    #|  #derive.config(default="[null,7,\"retained\"]")
    #|  slots : (Int?, Int?, String)
    #|  #derive.config(default="18446744073709551615")
    #|  limit : UInt64
    #|}
  let configs = @lib.parse_config_types([("config.mbt", source)]) catch {
    @lib.AgentParseError(message) => fail(message)
  }
  @lib.validate_config_types([], configs)
  assert_eq(configs[0].field_metadata["max_chars"], ("maxChars", Some("0")))
  assert_eq(configs[0].field_metadata["cleared"].1, Some("null"))
  assert_eq(configs[0].field_metadata["omitted"].1, None)
  assert_eq(configs[0].field_metadata["limit"].1, Some("18446744073709551615"))
  let output = @formatter.impls_to_string(
    @lib.generate_config_field_impls(configs),
  )
  assert_true(output.contains("default_value=\"null\""))
  assert_true(output.contains("@config.wire_local_entry"))
  assert_true(output.contains("TypeTag[Settings]"))
  assert_true(output.contains("fp0.push(\"maxChars\")"))
  assert_true(output.contains("fp1.push(\"Settings/Exact\")"))
  let load = output.split("with fn load(path)").to_array()[1].to_string()
  assert_true(load.contains("max_chars :UInt = @config.load_config_value(fp0)"))
  assert_false(load.contains("default_value"))
  assert_false(load.contains("code-default"))
  let overrides = @formatter.impls_to_string(
    @lib.generate_config_override_walkers(configs),
  )
  assert_true(overrides.contains("p.push(\"maxChars\")"))
  assert_true(overrides.contains("p.push(\"Settings/Exact\")"))
}

///|
test "config rejects nonportable defaults and invalid canonical names" {
  for
    field in [
      "#derive.config(default=make_default)\n value : String", "#derive.config(default=42)\n value : Int",
      "#derive.config(default=\"not JSON\")\n value : String", "#derive.config(default=\"null\")\n value : @config.Secret[String]",
      "#derive.config(name=\"$config\")\n value : String", "#derive.config(name=\"\")\n value : String",
      "max_chars : UInt\n maxChars : UInt", "#derive.config(default=\"1\", default=\"2\")\n value : Int",
    ] {
    let source = "#derive.config\nstruct Bad {\n" + field + "\n}"
    try @lib.parse_config_types([("bad.mbt", source)]) catch {
      @lib.AgentParseError(_) => ()
    } noraise {
      _ => fail("expected config declaration to be rejected: " + field)
    }
  }
  let groups = @lib.parse_config_types([
    (
      "groups.mbt",
      (
        #|#derive.config
        #|struct Nested { value : Int }
        #|#derive.config
        #|struct Bad {
        #|  #derive.config(default="{\"value\":3}")
        #|  nested : Nested
        #|}
      ),
    ),
  ])
  try @lib.validate_config_types([], groups) catch {
    @lib.AgentParseError(message) =>
      assert_true(message.contains("put defaults on its leaves"))
  } noraise {
    _ => fail("expected group default to be rejected")
  }
  let values = @lib.parse_value_types([
    ("record.mbt", "#derive.golem_schema\nstruct Data { max_chars : UInt }"),
  ])
  let ordinary = @formatter.impls_to_string(
    @lib.generate_value_type_impls(values),
  )
  assert_true(ordinary.contains("\"max_chars\""))
  assert_false(ordinary.contains("\"maxChars\""))
}
