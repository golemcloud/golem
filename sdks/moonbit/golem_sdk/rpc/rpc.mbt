///|
/// Typed RPC client for a remote agent. All payloads are carried as
/// `@types.SchemaValueTree` (the flat schema value carrier): the constructor
/// input and each method input/output are value trees whose root encodes the
/// parameter list (a record, one field per declared parameter). Generated client
/// code (Phase 4) builds these trees from `@schema` and decodes results back.
pub(all) struct AgentClient {
  agent_type_name : String
  constructor_input : @types.SchemaValueTree
  phantom_id : @types.Uuid?
  rpc : @agentHost.WasmRpc
  priv mut default_scope_card : @permissions.ScopeCard?
}

///|
pub(all) struct InvocationMetadata {
  agent_id : String
  idempotency_key : String
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
/// when constructing an agent client. The agent code generator implements this
/// trait for generated `*Override` types.
pub(open) trait AgentConfigOverrides {
  fn typed_values(Self) -> Array[@common.TypedAgentConfigValue] raise @common.AgentError
}

///|
fn make_client(
  agent_type_name : String,
  constructor_input : @types.SchemaValueTree,
  phantom_id : @types.Uuid?,
  agent_config : Array[@common.TypedAgentConfigValue],
) -> AgentClient {
  let rpc = @agentHost.WasmRpc::wasm_rpc(
    agent_type_name, constructor_input, phantom_id, agent_config,
  )
  {
    agent_type_name,
    constructor_input,
    phantom_id,
    rpc,
    default_scope_card: None,
  }
}

///|
pub fn AgentClient::get(
  agent_type_name : String,
  constructor_input : @types.SchemaValueTree,
) -> AgentClient {
  make_client(agent_type_name, constructor_input, None, [])
}

///|
pub fn AgentClient::get_with_config(
  agent_type_name : String,
  constructor_input : @types.SchemaValueTree,
  agent_config : Array[@common.TypedAgentConfigValue],
) -> AgentClient {
  make_client(agent_type_name, constructor_input, None, agent_config)
}

///|
pub fn AgentClient::new_phantom(
  agent_type_name : String,
  constructor_input : @types.SchemaValueTree,
) -> AgentClient {
  let phantom_id = @apiHost.generate_idempotency_key()
  AgentClient::get_phantom(agent_type_name, constructor_input, phantom_id)
}

///|
pub fn AgentClient::new_phantom_with_config(
  agent_type_name : String,
  constructor_input : @types.SchemaValueTree,
  agent_config : Array[@common.TypedAgentConfigValue],
) -> AgentClient {
  let phantom_id = @apiHost.generate_idempotency_key()
  AgentClient::get_phantom_with_config(
    agent_type_name, constructor_input, phantom_id, agent_config,
  )
}

///|
/// Creates a logical ephemeral proxy. The host assigns a fresh final phantom
/// identity independently for every invocation.
pub fn AgentClient::ephemeral(
  agent_type_name : String,
  constructor_input : @types.SchemaValueTree,
) -> AgentClient {
  make_client(agent_type_name, constructor_input, None, [])
}

///|
pub fn AgentClient::ephemeral_with_config(
  agent_type_name : String,
  constructor_input : @types.SchemaValueTree,
  agent_config : Array[@common.TypedAgentConfigValue],
) -> AgentClient {
  make_client(agent_type_name, constructor_input, None, agent_config)
}

///|
pub fn AgentClient::get_phantom(
  agent_type_name : String,
  constructor_input : @types.SchemaValueTree,
  phantom_id : @types.Uuid,
) -> AgentClient {
  make_client(agent_type_name, constructor_input, Some(phantom_id), [])
}

///|
pub fn AgentClient::get_phantom_with_config(
  agent_type_name : String,
  constructor_input : @types.SchemaValueTree,
  phantom_id : @types.Uuid,
  agent_config : Array[@common.TypedAgentConfigValue],
) -> AgentClient {
  make_client(
    agent_type_name,
    constructor_input,
    Some(phantom_id),
    agent_config,
  )
}

///|
pub fn AgentClient::get_agent_id(
  self : AgentClient,
) -> String raise @common.AgentError {
  match
    @agentHost.make_agent_id(
      self.agent_type_name,
      self.constructor_input,
      self.phantom_id,
    ) {
    Ok(id) => id
    Err(e) => raise e
  }
}

///|
pub fn AgentClient::phantom_id(self : AgentClient) -> @types.Uuid? {
  self.phantom_id
}

///|
/// Sets the scope card that awaited invocations of this client attach when the
/// call does not pass its own `scope_card`. `None` removes the default.
///
/// The client borrows the scope card: the card is kept, and every awaited
/// invocation lends it to the callee for the duration of that call.
/// Invocations that only trigger or schedule work never attach a scope card.
/// A scope card is valid only during the invocation that derived it. In a later
/// invocation the host denies the call and discards its whole input tree,
/// argument cards included, and the awaited invocation aborts with
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
  match result {
    Ok(value) => {
      let metadata : InvocationMetadata = {
        agent_id: invocation.metadata.agent_id,
        idempotency_key: invocation.metadata.idempotency_key,
      }
      { metadata, value, }
    }
    Err(e) => panic_on_rpc_error(e)
  }
}

///|
pub fn AgentClient::invoke(
  self : AgentClient,
  function_name : String,
  input : @types.SchemaValueTree,
) -> InvocationMetadata {
  match self.rpc.invoke(function_name, input, None) {
    Ok(value) =>
      { agent_id: value.agent_id, idempotency_key: value.idempotency_key, }
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
    Ok(value) =>
      {
        metadata: {
          agent_id: value.metadata.agent_id,
          idempotency_key: value.metadata.idempotency_key,
        },
      }
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
        metadata: {
          agent_id: value.metadata.agent_id,
          idempotency_key: value.metadata.idempotency_key,
        },
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
