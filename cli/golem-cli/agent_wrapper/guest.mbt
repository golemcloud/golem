

///|
pub fn initialize(
  agent_type : String,
  input : @common.DataValue,
  auth : Option[@common.AuthContext],
) -> Result[Unit, @common.AgentError] {
  @guest.initialize(agent_type, input, auth)
}

///|
pub fn invoke(
  method_name : String,
  input : @common.DataValue,
  auth : Option[@common.AuthContext],
) -> Result[@common.DataValue, @common.AgentError] {
  @guest.invoke(method_name, input, auth)
}

///|
pub fn get_definition() -> @common.AgentType {
  @guest.get_definition()
}

///|
pub fn discover_agent_types() -> Result[Array[@common.AgentType], @common.AgentError] {
  @guest.discover_agent_types()
}
