///|
pub(all) struct AgentClient {
  agent_type_name : String
  constructor_data_value : @types.DataValue
  phantom_id : @types.Uuid?
  rpc : @agentHost.WasmRpc
}

///|
fn make_client(
  agent_type_name : String,
  constructor_data_value : @types.DataValue,
  phantom_id : @types.Uuid?,
  agent_config : Array[@common.TypedAgentConfigValue],
) -> AgentClient raise @common.AgentError {
  let rpc = @agentHost.WasmRpc::wasm_rpc(
    agent_type_name, constructor_data_value, phantom_id, agent_config,
  )
  { agent_type_name, constructor_data_value, phantom_id, rpc }
}

///|
pub fn AgentClient::get(
  agent_type_name : String,
  constructor_data_value : @types.DataValue,
) -> AgentClient raise @common.AgentError {
  make_client(agent_type_name, constructor_data_value, None, [])
}

///|
pub fn AgentClient::get_with_config(
  agent_type_name : String,
  constructor_data_value : @types.DataValue,
  agent_config : Array[@common.TypedAgentConfigValue],
) -> AgentClient raise @common.AgentError {
  make_client(agent_type_name, constructor_data_value, None, agent_config)
}

///|
pub fn AgentClient::new_phantom(
  agent_type_name : String,
  constructor_data_value : @types.DataValue,
) -> AgentClient raise @common.AgentError {
  let phantom_id = @apiHost.generate_idempotency_key()
  AgentClient::get_phantom(agent_type_name, constructor_data_value, phantom_id)
}

///|
pub fn AgentClient::new_phantom_with_config(
  agent_type_name : String,
  constructor_data_value : @types.DataValue,
  agent_config : Array[@common.TypedAgentConfigValue],
) -> AgentClient raise @common.AgentError {
  let phantom_id = @apiHost.generate_idempotency_key()
  AgentClient::get_phantom_with_config(
    agent_type_name, constructor_data_value, phantom_id, agent_config,
  )
}

///|
pub fn AgentClient::get_phantom(
  agent_type_name : String,
  constructor_data_value : @types.DataValue,
  phantom_id : @types.Uuid,
) -> AgentClient raise @common.AgentError {
  make_client(agent_type_name, constructor_data_value, Some(phantom_id), [])
}

///|
pub fn AgentClient::get_phantom_with_config(
  agent_type_name : String,
  constructor_data_value : @types.DataValue,
  phantom_id : @types.Uuid,
  agent_config : Array[@common.TypedAgentConfigValue],
) -> AgentClient raise @common.AgentError {
  make_client(
    agent_type_name,
    constructor_data_value,
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
      self.constructor_data_value,
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
pub fn AgentClient::invoke_and_await(
  self : AgentClient,
  function_name : String,
  input : @types.DataValue,
) -> @types.DataValue {
  match self.rpc.invoke_and_await(function_name, input) {
    Ok(result) => result
    Err(e) => panic_on_rpc_error(e)
  }
}

///|
pub fn AgentClient::invoke(
  self : AgentClient,
  function_name : String,
  input : @types.DataValue,
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
  input : @types.DataValue,
) -> Unit {
  self.rpc.schedule_invocation(scheduled_at, function_name, input)
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
      abort("RPC remote agent error: " + e.to_string())
  }
}
