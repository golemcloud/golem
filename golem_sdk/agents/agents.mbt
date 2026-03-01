// TODO: should not be public

///|
/// Global singleton holding all registered agents and the current agent instance.
let state : AgentState = AgentState::new()

///|
/// Internal mutable state for the agent registry.
priv struct AgentState {
  registered_agents : @hashmap.HashMap[String, RegisteredAgent]
  mut agent_instance : AgentInstance?
}

///|
/// Creates a new empty agent state.
fn AgentState::new() -> AgentState {
  { registered_agents: @hashmap.new(), agent_instance: None }
}

///|
/// Registers an agent type in the global registry. Called during `init` by generated code.
pub fn register_agent(agent : RegisteredAgent) -> Unit {
  state.registered_agents.set(agent.name, agent)
}

///|
/// Looks up a registered agent by its type name.
pub fn get_registered_agent(name : String) -> RegisteredAgent? {
  state.registered_agents.get(name)
}

///|
/// Returns the `AgentType` definitions for all registered agents.
pub fn get_all_registered_agents() -> Array[@common.AgentType] {
  state.registered_agents.to_array().map(kv => kv.1.agent_type)
}

///|
/// Returns the currently initialized agent instance, if any.
pub fn get_agent_instance() -> AgentInstance? {
  state.agent_instance
}

///|
/// Sets the current agent instance after initialization.
pub fn set_agent_instance(instance : AgentInstance) -> Unit {
  state.agent_instance = Some(instance)
}

///|
/// A registered agent type with its name, type definition, and constructor function.
pub(all) struct RegisteredAgent {
  name : String
  agent_type : @common.AgentType
  construct : (@common.DataValue) -> Result[&RawAgent, @common.AgentError]
}

///|
/// A live agent instance with its name, type definition, and dispatch interface.
pub(all) struct AgentInstance {
  name : String
  agent_type : @common.AgentType
  interface : &RawAgent
}

///|
/// Logs an error message and panics. Used for unrecoverable SDK errors.
pub fn[T] panic_with_message(context : String, message : String) -> T {
  @logging.log(@logging.Level::ERROR, context, message)
  panic()
}

///|
/// Low-level trait that every agent must implement. Generated code provides
/// `invoke` dispatch by method name.
pub(open) trait RawAgent {
  invoke(Self, method_name : String, input : @common.DataValue) -> Result[
    @common.DataValue,
    @common.AgentError,
  ]
}
