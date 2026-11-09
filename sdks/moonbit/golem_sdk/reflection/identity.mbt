///|
/// Constructs an agent ID from manually packed constructor values. This is a
/// fully dynamic operation: the runtime is authoritative about whether the
/// requested lifecycle operation is valid.
pub fn ParsedAgentId::create(
  type_name : String,
  constructor_value : @model.SchemaValue,
  phantom_id~ : @schema.Uuid? = None,
) -> ParsedAgentId raise ReflectError {
  let prepared = @identity.Constructor::from_value(
    type_name, constructor_value, phantom_id,
  ) catch {
    error => raise Schema("failed to encode constructor value: \{repr(error)}")
  }
  let value = prepared.agent_id() catch {
    error => raise Identity(agent_error_message(error))
  }
  { value, }
}

///|
pub fn ParsedAgentId::parts(
  self : ParsedAgentId,
) -> ParsedAgentIdParts raise ReflectError {
  let parsed = @identity.parse(self.value) catch {
    error => raise Identity(agent_error_message(error))
  }
  let constructor_value = parsed.constructor_value() catch {
    error => raise Schema("failed to decode agent ID: \{repr(error)}")
  }
  {
    type_name: parsed.type_name,
    constructor_value,
    phantom_id: parsed.phantom_id,
  }
}
