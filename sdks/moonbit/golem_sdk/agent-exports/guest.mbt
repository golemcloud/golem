///|
/// Initializes the agent of a given type with the given constructor parameters.
/// If called a second time, it fails.
pub async fn initialize(
  agent_type : String,
  input : @types.SchemaValueTree,
  _background_group : @async-core.TaskGroup[Unit],
) -> Result[Unit, @common.AgentError] {
  let context = @agents.InvocationContext::current()
  defer context.end()
  if @agents.get_agent_instance() is Some(_) {
    @agents.panic_with_message(
      "initialize", "Agent instance is already initialized",
    )
  } else {
    @agents.construct_and_set_instance(agent_type, input, context)
  }
}

///|
/// Invokes an agent. If create was not called before, it fails
pub async fn invoke(
  method_name : String,
  input : @types.SchemaValueTree,
  _background_group : @async-core.TaskGroup[Unit],
) -> Result[@types.SchemaValueTree?, @common.AgentError] {
  let context = @agents.InvocationContext::current()
  defer context.end()
  let result = match @agents.get_agent_instance() {
    None =>
      @agents.panic_with_message("invoke", "Agent instance is not initialized")
    Some(instance) => instance.interface.invoke(method_name, input, context)
  }
  // A result that holds a stream keeps the caller for its producer.
  context.end(
    returned_stream=result is Ok(Some(tree)) && @caller.holds_stream(tree),
  )
  result
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
