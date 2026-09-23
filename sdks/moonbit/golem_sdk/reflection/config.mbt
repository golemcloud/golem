///|
fn AgentType::config_declaration(
  self : AgentType,
  path : Array[String],
) -> ReflectedConfigDeclaration raise ReflectError {
  let declaration = self.config.iter().find_first(entry => entry.path == path)
  guard declaration is Some(declaration) else {
    raise Schema("unknown config path '\{path.join(".")}'")
  }
  guard declaration.source == @common.AgentConfigSource::LOCAL else {
    raise Schema(
      "secret config field '\{path.join(".")}' cannot be overridden over RPC",
    )
  }
  declaration
}

///|
pub fn AgentType::pack_config_json(
  self : AgentType,
  overrides : Array[ReflectedConfigJsonValue],
) -> Array[@common.TypedAgentConfigValue] raise ReflectError {
  let result : Array[@common.TypedAgentConfigValue] = []
  for entry in overrides {
    let declaration = self.config_declaration(entry.path)
    let value = declaration.schema.pack_json(entry.value)
    let wire = @model.typed_schema_value_to_wit({
      graph: declaration.schema.graph,
      value,
    }) catch {
      error => raise Schema("failed to encode config value: \{repr(error)}")
    }
    result.push({ path: entry.path, value: wire, })
  }
  result
}

///|
pub fn AgentType::validate_config_values(
  self : AgentType,
  overrides : Array[ReflectedConfigValue],
) -> Array[@common.TypedAgentConfigValue] raise ReflectError {
  let result : Array[@common.TypedAgentConfigValue] = []
  for entry in overrides {
    let declaration = self.config_declaration(entry.path)
    declaration.schema.validate_value(entry.value)
    let wire = @model.typed_schema_value_to_wit({
      graph: declaration.schema.graph,
      value: entry.value,
    }) catch {
      error => raise Schema("failed to encode config value: \{repr(error)}")
    }
    result.push({ path: entry.path, value: wire, })
  }
  result
}
