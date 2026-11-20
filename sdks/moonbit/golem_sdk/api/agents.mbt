///|
pub using @raw {
  type UpdateMode,
  type AgentStatus,
  type AgentMetadata,
  type RevertAgentTarget,
  type FilterComparator,
  type StringFilterComparator,
  type AgentKeyFilter,
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
/// Get the current agent's metadata.
/// Requires self-view in the effective permission card, within the initialCard
/// upper bound. Do not call during pure initialization. Stored environment and
/// configuration fields are returned in full, without per-key WASI filtering.
/// Snapshot restoration instead reads GOLEM_AGENT_ID and GOLEM_AGENT_TYPE
/// through WASI and requires separate environment-read permission.
pub fn get_self_metadata() -> AgentMetadata {
  @raw.get_self_metadata().unwrap()
}

///|
/// Get agent metadata for a specific agent.
pub fn get_agent_metadata(agent_id : @schema.AgentId) -> AgentMetadata? {
  @raw.get_agent_metadata(agent_id.to_wit())
}

///|
/// Initiates an update attempt for the given agent. Returns immediately
/// once the request has been processed, not waiting for the agent to
/// get updated.
pub fn update_agent(
  agent_id : @schema.AgentId,
  target_revision : UInt64,
  mode : UpdateMode,
) -> Unit {
  @raw.update_agent(agent_id.to_wit(), target_revision, mode).unwrap()
}

///|
/// Forks an agent at a given oplog index into a new agent of the same type,
/// and returns the ID that Golem issued for the new agent. The new agent has
/// no key.
pub fn fork_agent(
  source_agent_id : @schema.AgentId,
  oplog_idx_cut_off : UInt64,
) -> @schema.AgentId {
  @schema.AgentId::from_wit(
    @raw.fork_agent(source_agent_id.to_wit(), oplog_idx_cut_off).unwrap(),
  )
}

///|
/// Revert an agent to a previous state.
pub fn revert_agent(
  agent_id : @schema.AgentId,
  revert_target : RevertAgentTarget,
) -> Unit {
  @raw.revert_agent(agent_id.to_wit(), revert_target).unwrap()
}

///|
/// Forks the current agent at the current execution point. The new agent has
/// the same type, a new ID that Golem issues and no key. Both sides receive
/// the ID of the forked agent in the `ForkResult`.
pub fn fork() -> ForkResult {
  @raw.fork().unwrap()
}

///|
/// A pager for iterating over batches of agent metadata.
/// Wraps the raw WIT resource handle, ensuring proper lifecycle management.
pub(all) struct AgentsPager {
  priv inner : @raw.GetAgents
}

///|
/// Creates an agent enumeration request. Enumerates the agents of the given
/// agent type in the current environment, filtered by the optional
/// conditions. If `precise` is true, the server will calculate the most recent
/// state of all returned agents.
pub fn get_agents(
  agent_type_name : String,
  filter : AgentAnyFilter?,
  precise : Bool,
) -> AgentsPager {
  { inner: @raw.GetAgents::get_agents(agent_type_name, filter, precise), }
}

///|
/// Retrieves the next batch of agent metadata.
/// Returns None when there are no more results.
pub fn AgentsPager::next_page(self : AgentsPager) -> Array[AgentMetadata]? {
  self.inner.get_next().unwrap()
}

///|
/// Collects all remaining pages of agent metadata into a single array.
pub fn AgentsPager::collect_all(self : AgentsPager) -> Array[AgentMetadata] {
  let result : Array[AgentMetadata] = []
  while true {
    match self.inner.get_next().unwrap() {
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
