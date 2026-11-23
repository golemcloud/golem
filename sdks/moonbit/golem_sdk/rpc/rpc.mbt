///|
/// Typed RPC client for a remote agent. All payloads are carried as
/// `@types.SchemaValueTree` (the flat schema value carrier): the constructor
/// input and each method input/output are value trees whose root encodes the
/// parameter list (a record, one field per declared parameter). Generated
/// client code builds these trees from `@schema` and decodes results back.
pub(all) struct AgentClient {
  agent_type_name : String
  rpc : @agentHost.WasmRpc
  priv mut default_scope_card : @permissions.ScopeCard?
}

///|
pub(all) struct InvocationMetadata {
  agent_id : @schema.AgentId
  invocation_id : InvocationId
}

///|
fn invocation_metadata(
  metadata : @agentHost.InvocationMetadata,
) -> InvocationMetadata {
  {
    agent_id: @schema.AgentId::from_wit(metadata.agent_id),
    invocation_id: InvocationId::from_host(metadata),
  }
}

///|
pub(all) struct InvocationResult[T] {
  metadata : InvocationMetadata
  value : T
}

///|
pub(all) struct ScheduledInvocationReceipt {
  metadata : InvocationMetadata
}

///|
pub(all) struct CancelableScheduledInvocationReceipt {
  metadata : InvocationMetadata
  cancellation_token : @agentHost.CancellationToken
}

///|
/// Converts a typed configuration override carrier into the leaf values sent
/// when creating an agent. The agent code generator implements this trait for
/// generated `*Override` types.
pub(open) trait AgentConfigOverrides {
  fn typed_values(Self) -> Array[@common.TypedAgentConfigValue] raise @common.AgentError
}

///|
fn make_client(
  agent_type_name : String,
  rpc : @agentHost.WasmRpc,
) -> AgentClient {
  { agent_type_name, rpc, default_scope_card: None, }
}

///|
/// Binds the current durable incarnation without creating it or waiting for initialization.
///
/// Use this to schedule work on self from inside the constructor. `upsert` reuses
/// an initialized agent without rerunning its constructor, but waits for an unfinished
/// constructor. Calling `upsert` on self from that constructor would therefore deadlock.
///
/// Scheduled work runs after construction finishes. This does not make awaited
/// self-calls reentrant or bypass method permissions. A different client type raises
/// an error. Replay preserves the recorded incarnation.
pub fn AgentClient::bind_self(agent_type_name : String) -> AgentClient raise {
  let (id, rpc) = match @agentHost.WasmRpc::bind_self() {
    Ok(bound) => bound
    Err(error) => raise error
  }
  errdefer rpc.drop()
  let bound_type = match @agentHost.get_agent_type_by_agent_id(id) {
    Some(registered) => registered.agent_type.type_name
    None =>
      raise @common.AgentError::InvalidAgentId(
        "self binding returned an agent ID of no deployed agent type: \{@schema.AgentId::from_wit(id)}",
      )
  }
  if bound_type != agent_type_name {
    raise @common.AgentError::InvalidType(
      "self binding expects \{agent_type_name}, identity is \{bound_type}",
    )
  }
  make_client(agent_type_name, rpc)
}

///|
/// Connects to the agent of the given type that the constructor parameters
/// key. For a durable type the host creates the agent when it is missing and
/// reuses it otherwise; for an ephemeral type every invocation runs on a new
/// one-shot instance.
pub fn AgentClient::upsert(
  agent_type_name : String,
  constructor_input : @types.SchemaValueTree,
) -> AgentClient raise @agentHost.RpcError {
  match @agentHost.WasmRpc::upsert(agent_type_name, constructor_input) {
    Ok(rpc) => make_client(agent_type_name, rpc)
    Err(error) => raise error
  }
}

///|
/// Connects to the existing agent with the given ID. Never creates an agent:
/// an unknown ID, or the ID of an agent of another type, raises
/// `RpcError::NotFound`.
pub fn AgentClient::get_by_id(
  agent_type_name : String,
  agent_id : @schema.AgentId,
) -> AgentClient raise @agentHost.RpcError {
  match @agentHost.WasmRpc::get_by_id(agent_type_name, agent_id.to_wit()) {
    Ok(rpc) => make_client(agent_type_name, rpc)
    Err(error) => raise error
  }
}

///|
/// Creates a new durable agent and returns after its initialization.
///
/// With `keyed`, the new agent is the keyed agent of the constructor
/// parameters, and the creation raises `RpcError::Conflict` when that agent
/// already exists. Otherwise the new agent has no key and a new ID.
/// `config` overrides values of the agent type's local configuration, and
/// `env` adds environment variables to the ones the manifest defines; the
/// caller's own environment is never copied.
pub async fn AgentClient::create(
  agent_type_name : String,
  constructor_input : @types.SchemaValueTree,
  keyed~ : Bool = false,
  config~ : Array[@common.TypedAgentConfigValue] = [],
  env~ : Array[(String, String)] = [],
) -> AgentClient {
  let options : @agentHost.CreateOptions = { keyed, agent_config: config, env, }
  match
    @agentHost.WasmRpc::create(agent_type_name, constructor_input, options) {
    Ok(created) => make_client(agent_type_name, created.client)
    Err(error) => raise error
  }
}

///|
/// The ID of the agent this client connects to. On a client of `upsert`, the
/// host creates the agent when it is missing, without invoking a method. A
/// client of an ephemeral agent type has no agent ID and raises an error.
pub async fn AgentClient::agent_id(self : AgentClient) -> @schema.AgentId {
  match self.rpc.agent_id() {
    Ok(id) => @schema.AgentId::from_wit(id)
    Err(error) => raise error
  }
}

///|
/// Sets the scope card that awaited invocations of this client attach when the
/// call does not pass its own `scope_card`. `None` removes the default.
///
/// The client borrows the scope card: the card is kept, and every awaited
/// invocation lends it to the callee for the duration of that call.
/// Invocations that only trigger or schedule work never attach a scope card.
/// A scope card is valid only during the invocation that derived or received
/// it. In a later invocation the host denies the call and discards its whole
/// input tree, argument cards included, and the awaited invocation aborts with
/// `RPC denied`. An awaited invocation also aborts if the default scope card
/// was dropped in the meantime.
pub fn AgentClient::set_default_scope_card(
  self : AgentClient,
  card : @permissions.ScopeCard?,
) -> Unit {
  self.default_scope_card = card
}

///|
/// Aborts if the scope card that an awaited invocation with `scope_card` would
/// attach was dropped. Generated clients call this before they encode the call
/// input, so that a dropped scope card never moves an argument card.
pub fn AgentClient::check_scope_card(
  self : AgentClient,
  scope_card? : @permissions.ScopeCard,
) -> Unit {
  ignore(@scope_card.resolve(scope_card, self.default_scope_card))
}

///|
/// Invoke a method and await its result and final invocation metadata. The
/// result is `None` when the output schema is `unit`, and `Some(tree)` for a
/// single returned value.
///
/// `scope_card` lends an invocation-local scope card (see
/// `@permissions.derive_scope`) to the callee for this call; it overrides the
/// client default. The card is borrowed, not consumed. Aborts if the scope card
/// was dropped; call `check_scope_card` before encoding `input` to abort
/// before any argument card moves.
pub async fn AgentClient::invoke_and_await(
  self : AgentClient,
  function_name : String,
  input : @types.SchemaValueTree,
  scope_card? : @permissions.ScopeCard,
) -> InvocationResult[@types.SchemaValueTree?] {
  let raw_scope_card = @scope_card.resolve(scope_card, self.default_scope_card)
  let invocation = self.rpc.async_invoke_and_await(
    function_name, input, raw_scope_card,
  )
  defer invocation.future.drop()
  let result = {
    errdefer invocation.future.cancel()
    invocation.future.get()
  }
  match (result, invocation.metadata) {
    (Ok(value), Some(metadata)) =>
      { metadata: invocation_metadata(metadata), value, }
    (Ok(_), None) =>
      abort(
        "RPC protocol error: completed invocation has no invocation metadata",
      )
    (Err(e), _) => panic_on_rpc_error(e)
  }
}

///|
pub fn AgentClient::invoke(
  self : AgentClient,
  function_name : String,
  input : @types.SchemaValueTree,
) -> InvocationMetadata {
  match self.rpc.invoke(function_name, input, None) {
    Ok(value) => invocation_metadata(value)
    Err(e) => panic_on_rpc_error(e)
  }
}

///|
pub fn AgentClient::schedule_invocation(
  self : AgentClient,
  scheduled_at : @systemClock.Instant,
  function_name : String,
  input : @types.SchemaValueTree,
) -> ScheduledInvocationReceipt {
  match self.rpc.schedule_invocation(scheduled_at, function_name, input, None) {
    Ok(value) => { metadata: invocation_metadata(value.metadata), }
    Err(e) => panic_on_rpc_error(e)
  }
}

///|
pub fn AgentClient::schedule_cancelable_invocation(
  self : AgentClient,
  scheduled_at : @systemClock.Instant,
  function_name : String,
  input : @types.SchemaValueTree,
) -> CancelableScheduledInvocationReceipt {
  match
    self.rpc.schedule_cancelable_invocation(
      scheduled_at,
      function_name,
      input,
      None,
    ) {
    Ok(value) =>
      {
        metadata: invocation_metadata(value.metadata),
        cancellation_token: value.cancellation_token,
      }
    Err(e) => panic_on_rpc_error(e)
  }
}

///|
pub fn AgentClient::drop(self : AgentClient) -> Unit {
  self.rpc.drop()
}

///|
fn[T] panic_on_rpc_error(e : @agentHost.RpcError) -> T {
  match e {
    @agentHost.RpcError::ProtocolError(msg) =>
      abort("RPC protocol error: " + msg)
    @agentHost.RpcError::Denied(msg) => abort("RPC denied: " + msg)
    @agentHost.RpcError::NotFound(msg) => abort("RPC not found: " + msg)
    @agentHost.RpcError::RemoteInternalError(msg) =>
      abort("RPC remote internal error: " + msg)
    @agentHost.RpcError::RemoteAgentError(e) =>
      abort("RPC remote agent error: \{Repr(e)}")
    @agentHost.RpcError::Conflict(msg) => abort("RPC conflict: " + msg)
  }
}

///|
/// Build a `@common.TypedAgentConfigValue` from a path and a typed value whose
/// type implements `@schema.IntoSchema`. The value is encoded as a
/// self-contained `@types.TypedSchemaValue` (schema graph + value tree).
pub fn[T : @schema.IntoSchema] typed_config_value(
  path : Array[String],
  v : T,
) -> @common.TypedAgentConfigValue raise @common.AgentError {
  let wire = @model.typed_schema_value_to_wit(
    @schema.try_into_typed_schema_value(v),
  ) catch {
    e => raise @common.AgentError::InvalidInput(e.to_string())
  }
  { path, value: wire, }
}
