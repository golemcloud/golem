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
  { registered_agents: @hashmap.HashMap([]), agent_instance: None, }
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
/// Optional trait that agents can implement to support snapshot-based state persistence.
/// Snapshot restoration is a separate static factory registered for the agent type.
pub(open) trait Snapshottable {
  fn save_snapshot(Self) -> Bytes
}

///|
/// Indicates the serialization format used for snapshots.
pub(all) enum SnapshotFormat {
  Json
  Binary
} derive(Debug, Eq)

///|
/// Result of constructing an agent, containing the raw dispatch interface
/// and optionally the snapshot interface.
pub(all) struct ConstructedAgent {
  raw : &RawAgent
  snapshottable : &Snapshottable?
  snapshot_format : SnapshotFormat?
}

///|
/// Information available while constructing a fresh agent from a snapshot.
pub(all) struct SnapshotRestoreContext {
  agent_id : String
  agent_type : String
  identity_fields : Array[@model.SchemaValue]
  phantom_id : @types.Uuid?
  principal : @common.Principal
}

///|
/// Decodes an identity field from the ambient agent ID.
pub fn[T : @schema.FromSchema] SnapshotRestoreContext::identity(
  self : SnapshotRestoreContext,
  index : Int,
) -> T raise @common.AgentError {
  decode_param(self.identity_fields, index, "snapshot restoration")
}

///|
/// A registered agent type with distinct initialization and restoration factories.
pub(all) struct RegisteredAgent {
  name : String
  agent_type : @common.AgentType
  construct : async (@types.SchemaValueTree, @common.Principal) -> Result[
    ConstructedAgent,
    @common.AgentError,
  ]
  load_snapshot : (async (Bytes, SnapshotRestoreContext) -> Result[
    ConstructedAgent,
    String,
  ])?
}

///|
/// A live agent instance with its name, type definition, dispatch interface,
/// snapshot support, and the principal that initialized it.
pub(all) struct AgentInstance {
  name : String
  agent_type : @common.AgentType
  principal : @common.Principal
  interface : &RawAgent
  snapshottable : &Snapshottable?
  snapshot_format : SnapshotFormat?
}

///|
/// Installs a complete agent returned by an initialization or restoration factory.
fn install_constructed_agent(
  registered_agent : RegisteredAgent,
  principal : @common.Principal,
  constructed : ConstructedAgent,
) -> Unit {
  set_agent_instance({
    name: registered_agent.name,
    agent_type: registered_agent.agent_type,
    principal,
    interface: constructed.raw,
    snapshottable: constructed.snapshottable,
    snapshot_format: constructed.snapshot_format,
  })
}

///|
/// Commits a restoration result atomically to the agent singleton.
fn finish_restoration(
  registered_agent : RegisteredAgent,
  principal : @common.Principal,
  restored : Result[ConstructedAgent, String],
) -> Result[Unit, String] {
  match restored {
    Err(e) => Err(e)
    Ok(constructed) => {
      install_constructed_agent(registered_agent, principal, constructed)
      Ok(())
    }
  }
}

///|
/// Constructs an agent and sets it as the current instance.
pub async fn construct_and_set_instance(
  agent_type : String,
  input : @types.SchemaValueTree,
  principal : @common.Principal,
) -> Result[Unit, @common.AgentError] {
  match get_registered_agent(agent_type) {
    None =>
      panic_with_message(
        "construct_and_set_instance",
        "Agent type not found: \{agent_type}",
      )
    Some(registered_agent) => {
      let constructed = (registered_agent.construct)(input, principal)
      match constructed {
        Ok(constructed) => {
          install_constructed_agent(registered_agent, principal, constructed)
          Ok(())
        }
        Err(e) => Err(e)
      }
    }
  }
}

///|
/// Restores an agent and sets it as the current instance only after the
/// restoration factory has returned a complete agent successfully.
pub async fn restore_and_set_instance(
  agent_type : String,
  snapshot : Bytes,
  context : SnapshotRestoreContext,
) -> Result[Unit, String] {
  match get_registered_agent(agent_type) {
    None => Err("Agent type not found: \{agent_type}")
    Some(registered_agent) =>
      match registered_agent.load_snapshot {
        None => Err("Agent does not support snapshots")
        Some(load_snapshot) => {
          let principal = context.principal
          finish_restoration(
            registered_agent,
            principal,
            load_snapshot(snapshot, context),
          )
        }
      }
  }
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
  async fn invoke(
    Self,
    method_name : String,
    input : @types.SchemaValueTree,
    principal : @common.Principal,
  ) -> Result[@types.SchemaValueTree?, @common.AgentError]
}
