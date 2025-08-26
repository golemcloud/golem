

///|
pub fn initialize(
  agent_type : String,
  input : @common.DataValue,
) -> Result[Unit, @common.AgentError] {
  @guest.initialize(agent_type, input)
}

///|
pub fn invoke(
  method_name : String,
  input : @common.DataValue,
) -> Result[@common.DataValue, @common.AgentError] {
  @guest.invoke(method_name, input)
}

///|
pub fn get_definition() -> @common.AgentType {
  @guest.get_definition()
}

///|
pub fn discover_agent_types() -> Array[@common.AgentType] {
  @guest.discover_agent_types()
}
