///|
pub(all) struct AgentClient {
  agent_id : @types.AgentId
  rpc : @types.WasmRpc
}

///|
pub fn AgentClient::get(
  agent_type_name : String,
  constructor_data_value : @common.DataValue,
) -> AgentClient raise @common.AgentError {
  let registered = @agentHost.get_agent_type(agent_type_name)
  guard registered is Some(reg) else {
    raise @common.AgentError::InvalidType(
      "Agent type '\{agent_type_name}' not found",
    )
  }
  let component_id = reg.implemented_by
  let agent_id_str = match
    @agentHost.make_agent_id(agent_type_name, constructor_data_value, None) {
    Ok(id) => id
    Err(e) => raise e
  }
  let agent_id : @types.AgentId = { component_id, agent_id: agent_id_str }
  let rpc = @types.WasmRpc::wasm_rpc(agent_id)
  { agent_id, rpc }
}

///|
pub fn AgentClient::new_phantom(
  agent_type_name : String,
  constructor_data_value : @common.DataValue,
  phantom_id : @types.Uuid,
) -> AgentClient raise @common.AgentError {
  let registered = @agentHost.get_agent_type(agent_type_name)
  guard registered is Some(reg) else {
    raise @common.AgentError::InvalidType(
      "Agent type '\{agent_type_name}' not found",
    )
  }
  let component_id = reg.implemented_by
  let agent_id_str = match
    @agentHost.make_agent_id(
      agent_type_name,
      constructor_data_value,
      Some(phantom_id),
    ) {
    Ok(id) => id
    Err(e) => raise e
  }
  let agent_id : @types.AgentId = { component_id, agent_id: agent_id_str }
  let rpc = @types.WasmRpc::wasm_rpc(agent_id)
  { agent_id, rpc }
}

///|
pub fn AgentClient::get_agent_id(self : AgentClient) -> String {
  self.agent_id.agent_id
}

///|
pub fn AgentClient::phantom_id(
  self : AgentClient,
) -> @types.Uuid? raise @common.AgentError {
  let result = match @agentHost.parse_agent_id(self.agent_id.agent_id) {
    Ok(parsed) => parsed
    Err(e) => raise e
  }
  result.2
}

///|
pub fn AgentClient::invoke_and_await(
  self : AgentClient,
  function_name : String,
  params : Array[@types.WitValue],
) -> @types.WitValue {
  match self.rpc.invoke_and_await(function_name, params) {
    Ok(result) => result
    Err(e) => panic_on_rpc_error(e)
  }
}

///|
pub fn AgentClient::invoke(
  self : AgentClient,
  function_name : String,
  params : Array[@types.WitValue],
) -> Unit {
  match self.rpc.invoke(function_name, params) {
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
  params : Array[@types.WitValue],
) -> Unit {
  self.rpc.schedule_invocation(scheduled_at, function_name, params)
}

///|
pub fn AgentClient::drop(self : AgentClient) -> Unit {
  self.rpc.drop()
}

///|
fn[T] panic_on_rpc_error(e : @types.RpcError) -> T {
  match e {
    @types.RpcError::ProtocolError(msg) => abort("RPC protocol error: " + msg)
    @types.RpcError::Denied(msg) => abort("RPC denied: " + msg)
    @types.RpcError::NotFound(msg) => abort("RPC not found: " + msg)
    @types.RpcError::RemoteInternalError(msg) =>
      abort("RPC remote internal error: " + msg)
  }
}
