// TODO: should not be public

///|
/// Global singleton holding all registered agents and the current agent instance.
let state : AgentState = AgentState::new()

///|
/// Internal mutable state for the agent registry.
priv struct AgentState {
  registered_agents : @hashmap.HashMap[String, RegisteredAgent]
  mut agent_instance : AgentInstance?
  mut constructing_config : Array[() -> Unit raise @common.AgentError]?
}

///|
/// Creates a new empty agent state.
fn AgentState::new() -> AgentState {
  {
    registered_agents: @hashmap.HashMap([]),
    agent_instance: None,
    constructing_config: None,
  }
}

///|
/// Retains constructor-loaded config handles for refresh before method dispatch.
/// Method-local loads are not retained by the agent instance.
pub fn register_config_refresh(
  refresh : () -> Unit raise @common.AgentError,
) -> Unit {
  if state.constructing_config is Some(refreshers) {
    refreshers.push(refresh)
  }
}

///|
priv struct ConfigRefreshingAgent {
  raw : &RawAgent
  refreshers : Array[() -> Unit raise @common.AgentError]
}

///|
#warnings("-result_error_return")
impl RawAgent for ConfigRefreshingAgent with fn invoke(
  self,
  method_name,
  input,
  context,
) {
  // Ordinary recorded reads preserve each historical invocation's config during
  // replay; new invocations see the host's capture from the selected revision.
  for refresh in self.refreshers {
    refresh() catch {
      error => {
        discard_wire_value(input)
        return Err(error)
      }
    }
  }
  self.raw.invoke(method_name, input, context)
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
/// Async custom multipart save; restoration is a separate static factory.
pub(open) trait MultipartSnapshottable {
  async fn save_snapshot_parts(Self) -> Result[MultipartSnapshot, String]
}

///|
pub(all) enum SnapshotData {
  Json(Bytes)
  Binary(Bytes)
  Multipart(MultipartSnapshot)
} derive(Debug, Eq)

///|
pub extend SnapshotData with Debug::{to_repr}

///|
pub extend SnapshotData with Eq::{equal, not_equal}

///|
/// Indicates the serialization format used for snapshots.
pub(all) enum SnapshotFormat {
  Json
  Binary
} derive(Debug, Eq)

///|
pub extend SnapshotFormat with Debug::{to_repr}

///|
pub extend SnapshotFormat with Eq::{equal, not_equal}

///|
/// Result of constructing an agent, containing the raw dispatch interface
/// and optionally the snapshot interface.
pub(all) struct ConstructedAgent {
  raw : &RawAgent
  snapshottable : &Snapshottable?
  snapshot_format : SnapshotFormat?
  multipart_snapshottable : &MultipartSnapshottable?
}

///|
pub using @caller {
  type Caller,
  type InvocationContext,
  type Principal,
  type PrincipalKind,
  type PrincipalRef,
  type TokenProvenance,
  type UserPrincipal,
  type AccountPrincipal,
  type AgentPrincipal,
  type ToolPrincipal,
  type AccountAuthentication,
}

///|
/// Information available while constructing a fresh agent from a snapshot.
/// `identity_fields` holds the constructor parameters that the host passes
/// to `load`, in the form that `initialize` received them.
pub(all) struct SnapshotRestoreContext {
  agent_id : @schema.AgentId
  agent_type : String
  identity_fields : WireInput
}

///|
/// Decodes the constructor parameter at `index`. A restoration that does not
/// decode a constructor parameter holding a resource fails.
pub fn[T : @schema.FromWire] SnapshotRestoreContext::identity(
  self : SnapshotRestoreContext,
  index : Int32,
) -> T raise @common.AgentError {
  decode_wire_param(self.identity_fields, index, "\{self.agent_type}::new")
}

///|
/// Decodes the constructor parameters that `load` receives against the
/// constructor schema of the registered agent type, as `initialize` decodes
/// its input. The capabilities in `input` are dropped on failure.
pub fn decode_snapshot_constructor(
  agent_type : String,
  input : @types.SchemaValueTree,
) -> WireInput raise @common.AgentError {
  guard get_registered_agent(agent_type) is Some(registered_agent) else {
    @model.drop_wit_capabilities(input)
    raise @common.AgentError::InvalidType("Agent type not found: \{agent_type}")
  }
  let @common.InputSchema::Parameters(fields) = registered_agent.agent_type.constructor_.input_schema
  let arity = fields.filter(field => field.source is UserSupplied).length()
  decode_wire_input(input, arity, "\{agent_type}::new")
}

///|
/// A registered agent type with distinct initialization and restoration factories.
pub(all) struct RegisteredAgent {
  name : String
  agent_type : @common.AgentType
  construct : async (@types.SchemaValueTree, InvocationContext) -> Result[
    ConstructedAgent,
    @common.AgentError,
  ]
  load_snapshot : (async (SnapshotData, SnapshotRestoreContext) -> Result[
    ConstructedAgent,
    String,
  ])?
}

///|
/// A live agent instance with its name, type definition, dispatch interface,
/// and snapshot support.
pub(all) struct AgentInstance {
  name : String
  agent_type : @common.AgentType
  interface : &RawAgent
  snapshottable : &Snapshottable?
  snapshot_format : SnapshotFormat?
  multipart_snapshottable : &MultipartSnapshottable?
}

///|
/// Installs a complete agent returned by an initialization or restoration factory.
fn install_constructed_agent(
  registered_agent : RegisteredAgent,
  constructed : ConstructedAgent,
) -> Unit {
  let interface : &RawAgent = match state.constructing_config {
    Some(refreshers) if !refreshers.is_empty() =>
      (ConfigRefreshingAgent::{ raw: constructed.raw, refreshers, } : &RawAgent)
    _ => constructed.raw
  }
  set_agent_instance({
    name: registered_agent.name,
    agent_type: registered_agent.agent_type,
    interface,
    snapshottable: constructed.snapshottable,
    snapshot_format: constructed.snapshot_format,
    multipart_snapshottable: constructed.multipart_snapshottable,
  })
}

///|
/// Commits a restoration result atomically to the agent singleton.
fn finish_restoration(
  registered_agent : RegisteredAgent,
  restored : Result[ConstructedAgent, String],
) -> Result[Unit, String] {
  match restored {
    Err(e) => Err(e)
    Ok(constructed) => {
      install_constructed_agent(registered_agent, constructed)
      Ok(())
    }
  }
}

///|
/// Constructs an agent and sets it as the current instance.
#warnings("-result_error_return")
pub async fn construct_and_set_instance(
  agent_type : String,
  input : @types.SchemaValueTree,
  context : InvocationContext,
) -> Result[Unit, @common.AgentError] {
  match get_registered_agent(agent_type) {
    None =>
      panic_with_message(
        "construct_and_set_instance",
        "Agent type not found: \{agent_type}",
      )
    Some(registered_agent) => {
      state.constructing_config = Some([])
      defer {
        state.constructing_config = None
      }
      let constructed = (registered_agent.construct)(input, context)
      match constructed {
        Ok(constructed) => {
          install_constructed_agent(registered_agent, constructed)
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
  snapshot : SnapshotData,
  context : SnapshotRestoreContext,
) -> Result[Unit, String] {
  defer context.identity_fields.abort()
  match get_registered_agent(agent_type) {
    None => Err("Agent type not found: \{agent_type}")
    Some(registered_agent) =>
      match registered_agent.load_snapshot {
        None => Err("Agent does not support snapshots")
        Some(load_snapshot) => {
          state.constructing_config = Some([])
          defer {
            state.constructing_config = None
          }
          let restored = match load_snapshot(snapshot, context) {
            Ok(restored) => restored
            Err(error) => return Err(error)
          }
          context.identity_fields.finish() catch {
            error =>
              return Err("Invalid constructor parameters: \{repr(error)}")
          }
          finish_restoration(registered_agent, Ok(restored))
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
    context : InvocationContext,
  ) -> Result[@types.SchemaValueTree?, @common.AgentError]
}
