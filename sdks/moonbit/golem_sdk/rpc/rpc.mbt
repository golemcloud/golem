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
}

///|
fn make_client(
  agent_type_name : String,
  constructor_input : @types.SchemaValueTree,
  phantom_id : @types.Uuid?,
  agent_config : Array[@common.TypedAgentConfigValue],
) -> AgentClient raise @common.AgentError {
  let rpc = @agentHost.WasmRpc::wasm_rpc(
    agent_type_name, constructor_input, phantom_id, agent_config,
  )
  { agent_type_name, constructor_input, phantom_id, rpc }
}

///|
pub fn AgentClient::get(
  agent_type_name : String,
  constructor_input : @types.SchemaValueTree,
) -> AgentClient raise @common.AgentError {
  make_client(agent_type_name, constructor_input, None, [])
}

///|
pub fn AgentClient::get_with_config(
  agent_type_name : String,
  constructor_input : @types.SchemaValueTree,
  agent_config : Array[@common.TypedAgentConfigValue],
) -> AgentClient raise @common.AgentError {
  make_client(agent_type_name, constructor_input, None, agent_config)
}

///|
pub fn AgentClient::new_phantom(
  agent_type_name : String,
  constructor_input : @types.SchemaValueTree,
) -> AgentClient raise @common.AgentError {
  let phantom_id = @apiHost.generate_idempotency_key()
  AgentClient::get_phantom(agent_type_name, constructor_input, phantom_id)
}

///|
pub fn AgentClient::new_phantom_with_config(
  agent_type_name : String,
  constructor_input : @types.SchemaValueTree,
  agent_config : Array[@common.TypedAgentConfigValue],
) -> AgentClient raise @common.AgentError {
  let phantom_id = @apiHost.generate_idempotency_key()
  AgentClient::get_phantom_with_config(
    agent_type_name, constructor_input, phantom_id, agent_config,
  )
}

///|
pub fn AgentClient::get_phantom(
  agent_type_name : String,
  constructor_input : @types.SchemaValueTree,
  phantom_id : @types.Uuid,
) -> AgentClient raise @common.AgentError {
  make_client(agent_type_name, constructor_input, Some(phantom_id), [])
}

///|
pub fn AgentClient::get_phantom_with_config(
  agent_type_name : String,
  constructor_input : @types.SchemaValueTree,
  phantom_id : @types.Uuid,
  agent_config : Array[@common.TypedAgentConfigValue],
) -> AgentClient raise @common.AgentError {
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
/// Invoke a method and await its result. Returns `None` when the method's
/// output schema is `unit`, and `Some(tree)` for a single returned value.
pub fn AgentClient::invoke_and_await(
  self : AgentClient,
  function_name : String,
  input : @types.SchemaValueTree,
) -> @types.SchemaValueTree? {
  match self.rpc.invoke_and_await(function_name, input) {
    Ok(result) => result
    Err(e) => panic_on_rpc_error(e)
  }
}

///|
pub fn AgentClient::invoke(
  self : AgentClient,
  function_name : String,
  input : @types.SchemaValueTree,
) -> Unit {
  match self.rpc.invoke(function_name, input) {
    Ok(_) => ()
    Err(e) => {
      let _ : Unit = panic_on_rpc_error(e)
    }
  }
}

///|
pub fn AgentClient::schedule_invocation(
  self : AgentClient,
  scheduled_at : @wallClock.Datetime,
  function_name : String,
  input : @types.SchemaValueTree,
) -> Unit {
  self.rpc.schedule_invocation(scheduled_at, function_name, input)
}

///|
pub fn AgentClient::schedule_cancelable_invocation(
  self : AgentClient,
  scheduled_at : @wallClock.Datetime,
  function_name : String,
  input : @types.SchemaValueTree,
) -> @agentHost.CancellationToken {
  self.rpc.schedule_cancelable_invocation(scheduled_at, function_name, input)
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
      abort("RPC remote agent error: \{e}")
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
  { path, value: wire }
}
