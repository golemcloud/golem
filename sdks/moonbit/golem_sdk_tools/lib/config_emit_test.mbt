///|
test "generate config field impls targets the new schema-model API" {
  let config_types : Array[ConfigTypeInfo] = [
    {
      name: "DatabaseConfig",
      package: "",
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
      #|"///|\nimpl @config.ConfigField for DatabaseConfig with collect_entries(path) {\n  let entries :Array[@agents.AgentConfigDef] = []\n  let fp0 = path.copy()\n  fp0.push(\"host\")\n  entries.append(\n    @config.collect_entries_of(\n      (@schema.TypeTag::{  } : @schema.TypeTag[String]),\n      fp0,\n    ),\n  )\n  let fp1 = path.copy()\n  fp1.push(\"port\")\n  entries.append(\n    @config.collect_entries_of(\n      (@schema.TypeTag::{  } : @schema.TypeTag[UInt]),\n      fp1,\n    ),\n  )\n  let fp2 = path.copy()\n  fp2.push(\"timeout\")\n  entries.append(\n    @config.collect_entries_of(\n      (@schema.TypeTag::{  } : @schema.TypeTag[UInt64]),\n      fp2,\n    ),\n  )\n  entries\n}\n\n///|\nimpl @config.ConfigField for DatabaseConfig with load(path) {\n  let fp0 = path.copy()\n  fp0.push(\"host\")\n  let host :String = \n    @config.load_field((@schema.TypeTag::{  } : @schema.TypeTag[String]), fp0)\n  let fp1 = path.copy()\n  fp1.push(\"port\")\n  let port :UInt = \n    @config.load_field((@schema.TypeTag::{  } : @schema.TypeTag[UInt]), fp1)\n  let fp2 = path.copy()\n  fp2.push(\"timeout\")\n  let timeout :UInt64 = \n    @config.load_field((@schema.TypeTag::{  } : @schema.TypeTag[UInt64]), fp2)\n  { host, port, timeout }\n}\n\n"
    ),
  )
}
