///|
pub using @raw {
  type UpdateMode,
  type AgentStatus,
  type AgentMetadata,
  type RevertAgentTarget,
  type FilterComparator,
  type StringFilterComparator,
  type AgentNameFilter,
  type AgentStatusFilter,
  type AgentVersionFilter,
  type AgentCreatedAtFilter,
  type AgentEnvFilter,
  type AgentConfigVarsFilter,
  type AgentPropertyFilter,
  type AgentAllFilter,
  type AgentAnyFilter,
  type ForkResult,
  type ForkDetails,
  type EnvironmentId,
}

///|
/// Get the component-id for a given component reference.
/// Returns None when no component with the specified reference exists.
///
/// Golem OSS: "{component_name}"
/// Golem Cloud: "{component_name}", "{project_name}/{component_name}",
///   or "{account_id}/{project_name}/{component_name}"
pub fn resolve_component_id(
  component_reference : String,
) -> @types.ComponentId? {
  @raw.resolve_component_id(component_reference)
}

///|
/// Get the agent-id for a given component and agent name.
/// Returns None when no component for the specified reference exists.
pub fn resolve_agent_id(
  component_reference : String,
  agent_name : String,
) -> @types.AgentId? {
  @raw.resolve_agent_id(component_reference, agent_name)
}

///|
/// Get the agent-id for a given component and agent name.
/// Returns None when no component for the specified component-reference or
/// no agent with the specified agent-name exists.
pub fn resolve_agent_id_strict(
  component_reference : String,
  agent_name : String,
) -> @types.AgentId? {
  @raw.resolve_agent_id_strict(component_reference, agent_name)
}

///|
/// Get the current agent's metadata.
pub fn get_self_metadata() -> AgentMetadata {
  @raw.get_self_metadata()
}

///|
/// Get agent metadata for a specific agent.
pub fn get_agent_metadata(agent_id : @types.AgentId) -> AgentMetadata? {
  @raw.get_agent_metadata(agent_id)
}

///|
/// Initiates an update attempt for the given agent. Returns immediately
/// once the request has been processed, not waiting for the agent to
/// get updated.
pub fn update_agent(
  agent_id : @types.AgentId,
  target_revision : UInt64,
  mode : UpdateMode,
) -> Unit {
  @raw.update_agent(agent_id, target_revision, mode)
}

///|
/// Fork an agent to another agent at a given oplog index.
pub fn fork_agent(
  source_agent_id : @types.AgentId,
  target_agent_id : @types.AgentId,
  oplog_idx_cut_off : UInt64,
) -> Unit {
  @raw.fork_agent(source_agent_id, target_agent_id, oplog_idx_cut_off)
}

///|
/// Revert an agent to a previous state.
pub fn revert_agent(
  agent_id : @types.AgentId,
  revert_target : RevertAgentTarget,
) -> Unit {
  @raw.revert_agent(agent_id, revert_target)
}

///|
/// Forks the current agent at the current execution point. The new agent
/// gets the same base agent ID but with a new unique phantom ID.
pub fn fork() -> ForkResult {
  @raw.fork()
}

///|
/// A pager for iterating over batches of agent metadata.
/// Wraps the raw WIT resource handle, ensuring proper lifecycle management.
pub(all) struct AgentsPager {
  priv inner : @raw.GetAgents
}

///|
/// Creates an agent enumeration request. Enumerates all agents of all types
/// defined in the given component, filtered by the optional conditions.
/// If `precise` is true, the server will calculate the most recent state
/// of all returned agents.
pub fn get_agents(
  component_id : @types.ComponentId,
  filter : AgentAnyFilter?,
  precise : Bool,
) -> AgentsPager {
  { inner: @raw.GetAgents::get_agents(component_id, filter, precise) }
}

///|
/// Retrieves the next batch of agent metadata.
/// Returns None when there are no more results.
pub fn AgentsPager::next_page(self : AgentsPager) -> Array[AgentMetadata]? {
  self.inner.get_next()
}

///|
/// Collects all remaining pages of agent metadata into a single array.
pub fn AgentsPager::collect_all(self : AgentsPager) -> Array[AgentMetadata] {
  let result : Array[AgentMetadata] = []
  while true {
    match self.inner.get_next() {
      Some(page) => result.append(page)
      None => break
    }
  }
  result
}

///|
/// Drops the underlying resource handle.
pub fn AgentsPager::drop(self : AgentsPager) -> Unit {
  self.inner.drop()
}
