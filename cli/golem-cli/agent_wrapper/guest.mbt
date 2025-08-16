///|
impl Hash for Agent with hash_combine(self, hasher) {
  hasher.combine(self.rep())
}

///|
priv struct WrappedAgents {
  agents : Map[Agent, @guest.Agent]
  reverse_agents : Map[String, Agent]
  mut last_agent_id : Int
}

///|
fn Agent::unwrap(self : Agent) -> @guest.Agent {
  match wrapped_agents.agents.get(self) {
    Some(agent) => agent
    None => panic()
  }
}

///|
fn Agent::get_or_create(inner_agent : @guest.Agent) -> Agent {
  match wrapped_agents.reverse_agents.get(inner_agent.get_id()) {
    Some(agent) => agent
    None => {
      let agent = Agent::new(wrapped_agents.last_agent_id)
      wrapped_agents.last_agent_id += 1
      wrapped_agents.agents[agent] = inner_agent
      wrapped_agents.reverse_agents[inner_agent.get_id()] = agent
      agent
    }
  }
}

///|
let wrapped_agents : WrappedAgents = {
  agents: {},
  last_agent_id: 0,
  reverse_agents: {},
}

///| Destructor of the resource.
pub fn Agent::dtor(self : Agent) -> Unit {
  wrapped_agents.reverse_agents.remove(self.get_id())
  wrapped_agents.agents.remove(self)
}

///|
pub fn Agent::create(
  agent_type : String,
  input : @common.DataValue,
) -> Result[Agent, @common.AgentError] {
  @guest.Agent::create(agent_type, input).bind(inner_agent => {
    let agent = Agent::get_or_create(inner_agent)
    Ok(agent)
  })
}

///|
pub fn Agent::get_id(self : Agent) -> String {
  self.unwrap().get_id()
}

///|
pub fn Agent::invoke(
  self : Agent,
  method_name : String,
  input : @common.DataValue,
) -> Result[@common.DataValue, @common.AgentError] {
  self.unwrap().invoke(method_name, input)
}

///|
pub fn Agent::get_definition(self : Agent) -> @common.AgentType {
  self.unwrap().get_definition()
}

///|
pub fn get_agent(agent_type : String, agent_id : String) -> Agent {
  let inner_agent = @guest.get_agent(agent_type, agent_id)
  Agent::get_or_create(inner_agent)
}

///|
pub fn discover_agents() -> Array[Agent] {
  let agents = @guest.discover_agents()
  agents.map(Agent::get_or_create)
}

///|
pub fn discover_agent_types() -> Array[@common.AgentType] {
  @guest.discover_agent_types()
}

///|
pub fn invoke_agent(agent_type : String, agent_id : String, method_name : String, input : @common.DataValue) -> Result[@common.DataValue, @common.AgentError] {
  let inner_agent = @guest.get_agent(agent_type, agent_id)
  inner_agent.invoke(method_name, input)
}