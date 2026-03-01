///|
/// Initializes the agent of a given type with the given constructor parameters.
/// If called a second time, it fails.
pub fn initialize(
  agent_type : String,
  input : @common.DataValue,
) -> Result[Unit, @common.AgentError] {
  if not(@agents.get_agent_instance().is_empty()) {
    @agents.panic_with_message(
      "initialize", "Agent instance is already initialized",
    )
  } else {
    match @agents.get_registered_agent(agent_type) {
      None =>
        @agents.panic_with_message(
          "initialize",
          "Agent type not found: \{agent_type}",
        )
      Some(registered_agent) => {
        let constructed = (registered_agent.construct)(input)
        match constructed {
          Ok(constructed) => {
            @agents.set_agent_instance({
              name: registered_agent.name,
              agent_type: registered_agent.agent_type,
              interface: constructed.raw,
              snapshottable: constructed.snapshottable,
            })
            Ok(())
          }
          Err(e) => Err(e)
        }
      }
    }
  }
}

///|
/// Invokes an agent. If create was not called before, it fails
pub fn invoke(
  method_name : String,
  input : @common.DataValue,
) -> Result[@common.DataValue, @common.AgentError] {
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
