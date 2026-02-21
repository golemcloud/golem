

///|
pub fn initialize(
  agent_type : String,
  input : @common.DataValue,
  principal : @common.Principal,
) -> Result[Unit, @common.AgentError] {
  @guest.initialize(agent_type, input, principal)
}

///|
pub fn invoke(
  method_name : String,
  input : @common.DataValue,
  principal : @common.Principal,
) -> Result[@common.DataValue, @common.AgentError] {
  @guest.invoke(method_name, input, principal)
}

///|
pub fn get_definition() -> @common.AgentType {
  @guest.get_definition()
}

///|
pub fn discover_agent_types() -> Result[Array[@common.AgentType], @common.AgentError] {
  @guest.discover_agent_types()
}
