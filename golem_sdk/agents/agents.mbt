// TODO: should not be public

///|
let state : AgentState = AgentState::new()

///|
priv struct AgentState {
  registered_agents : @hashmap.HashMap[String, RegisteredAgent]
  mut agent_instance : AgentInstance?
}

///|
fn AgentState::new() -> AgentState {
  { registered_agents: @hashmap.new(), agent_instance: None }
}

///|
pub fn register_agent(agent : RegisteredAgent) -> Unit {
  state.registered_agents.set(agent.name, agent)
}

///|
pub fn get_registered_agent(name : String) -> RegisteredAgent? {
  state.registered_agents.get(name)
}

///|
pub fn get_all_registered_agents() -> Array[@common.AgentType] {
  state.registered_agents.to_array().map(kv => kv.1.agent_type)
}

///|
pub fn get_agent_instance() -> AgentInstance? {
  state.agent_instance
}

///|
pub fn set_agent_instance(instance : AgentInstance) -> Unit {
  state.agent_instance = Some(instance)
}

///|
pub(all) struct RegisteredAgent {
  name : String
  agent_type : @common.AgentType
  construct : (@common.DataValue) -> Result[&RawAgent, @common.AgentError]
}

///|
pub(all) struct AgentInstance {
  name : String
  agent_type : @common.AgentType
  interface : &RawAgent
}

///|
pub fn[T] panic_with_message(context : String, message : String) -> T {
  @logging.log(@logging.Level::ERROR, context, message)
  panic()
}

///|
pub(open) trait RawAgent {
  invoke(Self, method_name : String, input : @common.DataValue) -> Result[
    @common.DataValue,
    @common.AgentError,
  ]
}
