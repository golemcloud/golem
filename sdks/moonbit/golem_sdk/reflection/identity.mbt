///|
/// Constructs an agent ID from manually packed constructor values. This is a
/// schema-free operation: the runtime is authoritative about whether the
/// requested lifecycle operation is valid.
pub fn AgentId::create(
  component_id : @types.ComponentId,
  type_name : String,
  constructor_value : @model.SchemaValue,
  phantom_id? : @types.Uuid? = None,
) -> AgentId raise ReflectError {
  let wire = @model.schema_value_to_wit(constructor_value) catch {
    error => raise Schema("failed to encode constructor value: \{repr(error)}")
  }
  match @agentHost.make_agent_id(type_name, wire, phantom_id) {
    Ok(value) => { component_id, value, }
    Err(error) => raise Identity(agent_error_message(error))
  }
}

///|
pub fn AgentId::parts(self : AgentId) -> AgentIdParts raise ReflectError {
  match @agentHost.parse_agent_id(self.value) {
    Ok((type_name, constructor_input, phantom_id)) => {
      let typed = @model_host.typed_schema_value_from_wit(constructor_input) catch {
        error => raise Schema("failed to decode agent ID: \{repr(error)}")
      }
      { type_name, constructor_value: typed.value, phantom_id, }
    }
    Err(error) => raise Identity(agent_error_message(error))
  }
}

///|
pub fn AgentId::to_wit(self : AgentId) -> @types.AgentId {
  { component_id: self.component_id, agent_id: self.value, }
}

///|
pub fn agent_id_from_wit(agent_id : @types.AgentId) -> AgentId {
  { component_id: agent_id.component_id, value: agent_id.agent_id, }
}
