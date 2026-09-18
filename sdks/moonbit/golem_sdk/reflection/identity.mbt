///|
/// Constructs an agent ID from manually packed constructor values. This is a
/// schema-free operation: the runtime is authoritative about whether the
/// requested lifecycle operation is valid.
pub fn ParsedAgentId::create(
  type_name : String,
  constructor_value : @model.SchemaValue,
  phantom_id? : @types.Uuid? = None,
) -> ParsedAgentId raise ReflectError {
  let wire = @model.schema_value_to_wit(constructor_value) catch {
    error => raise Schema("failed to encode constructor value: \{repr(error)}")
  }
  match @agentHost.make_agent_id(type_name, wire, phantom_id) {
    Ok(value) => { value, }
    Err(error) => raise Identity(agent_error_message(error))
  }
}

///|
pub fn ParsedAgentId::parts(
  self : ParsedAgentId,
) -> ParsedAgentIdParts raise ReflectError {
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
