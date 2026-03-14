///|
/// Initializes the agent of a given type with the given constructor parameters.
/// If called a second time, it fails.
pub fn initialize(
  agent_type : String,
  input : @types.DataValue,
  principal : @common.Principal,
) -> Result[Unit, @common.AgentError] {
  if @agents.get_agent_instance() is Some(_) {
    @agents.panic_with_message(
      "initialize", "Agent instance is already initialized",
    )
  } else {
    @agents.construct_and_set_instance(agent_type, input, principal)
  }
}

///|
/// Invokes an agent. If create was not called before, it fails
pub fn invoke(
  method_name : String,
  input : @types.DataValue,
  _principal : @common.Principal,
) -> Result[@types.DataValue, @common.AgentError] {
  match @agents.get_agent_instance() {
    None =>
      @agents.panic_with_message(
        "get_definition", "Agent instance is not initialized",
      )
    Some(instance) => instance.interface.invoke(method_name, input)
  }
}

///|
/// Gets the agent type. If create was not called before, it fails
pub fn get_definition() -> @common.AgentType {
  match @agents.get_agent_instance() {
    None =>
      @agents.panic_with_message(
        "get_definition", "Agent instance is not initialized",
      )
    Some(instance) => instance.agent_type
  }
}

///|
/// Gets the agent types defined by this component
pub fn discover_agent_types() -> Result[
  Array[@common.AgentType],
  @common.AgentError,
] {
  Ok(@agents.get_all_registered_agents())
}
