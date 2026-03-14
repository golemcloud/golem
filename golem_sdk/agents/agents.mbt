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
/// Optional trait that agents can implement to support snapshot-based state persistence.
/// When implemented, the SDK will use these methods to save/load agent state during updates.
pub(open) trait Snapshottable {
  save_snapshot(Self) -> Bytes
  load_snapshot(Self, Bytes) -> Result[Unit, String]
}

///|
/// Indicates the serialization format used for snapshots.
pub(all) enum SnapshotFormat {
  Json
  Binary
} derive(Show, Eq)

///|
/// Result of constructing an agent, containing the raw dispatch interface
/// and optionally the snapshot interface.
pub(all) struct ConstructedAgent {
  raw : &RawAgent
  snapshottable : &Snapshottable?
  snapshot_format : SnapshotFormat?
}

///|
/// A registered agent type with its name, type definition, and constructor function.
pub(all) struct RegisteredAgent {
  name : String
  agent_type : @common.AgentType
  construct : (@rpcTypes.DataValue) -> Result[
    ConstructedAgent,
    @common.AgentError,
  ]
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
/// Constructs an agent and sets it as the current instance.
/// Shared logic used by both `initialize` and snapshot-based `load`.
pub fn construct_and_set_instance(
  agent_type : String,
  input : @rpcTypes.DataValue,
  principal : @common.Principal,
) -> Result[Unit, @common.AgentError] {
  match get_registered_agent(agent_type) {
    None =>
      panic_with_message(
        "construct_and_set_instance",
        "Agent type not found: \{agent_type}",
      )
    Some(registered_agent) => {
      let constructed = (registered_agent.construct)(input)
      match constructed {
        Ok(constructed) => {
          set_agent_instance({
            name: registered_agent.name,
            agent_type: registered_agent.agent_type,
            principal,
            interface: constructed.raw,
            snapshottable: constructed.snapshottable,
            snapshot_format: constructed.snapshot_format,
          })
          Ok(())
        }
        Err(e) => Err(e)
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
  invoke(Self, method_name : String, input : @rpcTypes.DataValue) -> Result[
    @rpcTypes.DataValue,
    @common.AgentError,
  ]
}
