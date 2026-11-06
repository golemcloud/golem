///|
/// Prepared constructor identity shared by the typed and reflected clients.
pub struct Constructor {
  type_name : String
  input : @types.SchemaValueTree
  phantom_id : @types.Uuid?
}

///|
pub fn Constructor::from_wire(
  type_name : String,
  input : @types.SchemaValueTree,
  phantom_id : @schema.Uuid?,
) -> Constructor {
  { type_name, input, phantom_id: phantom_id.map(@schema.Uuid::to_wit), }
}

///|
pub fn Constructor::from_value(
  type_name : String,
  value : @model.SchemaValue,
  phantom_id : @schema.Uuid?,
) -> Constructor raise @model.SchemaModelError {
  Constructor::from_wire(
    type_name,
    @model.schema_value_to_wit(value),
    phantom_id,
  )
}

///|
pub fn Constructor::agent_id(
  self : Constructor,
) -> String raise @common.AgentError {
  match @agentHost.make_agent_id(self.type_name, self.input, self.phantom_id) {
    Ok(id) => id
    Err(error) => raise error
  }
}

///|
/// Uses the trapping host constructor required by typed RPC factories.
pub fn Constructor::connect(
  self : Constructor,
  config : Array[@common.TypedAgentConfigValue],
) -> @agentHost.WasmRpc {
  @agentHost.WasmRpc::wasm_rpc(
    self.type_name,
    self.input,
    self.phantom_id,
    config,
  )
}

///|
/// Uses the fallible host constructor required by reflection factories.
pub fn Constructor::try_connect(
  self : Constructor,
  config : Array[@common.TypedAgentConfigValue],
) -> Result[@agentHost.WasmRpc, @agentHost.RpcError] {
  @agentHost.WasmRpc::create(
    self.type_name,
    self.input,
    self.phantom_id,
    config,
  )
}

///|
pub fn new_phantom_id() -> @schema.Uuid {
  @schema.Uuid::from_wit(@apiHost.generate_idempotency_key())
}

///|
pub struct Parsed {
  type_name : String
  input : @types.TypedSchemaValue
  phantom_id : @schema.Uuid?
}

///|
pub fn parse(value : String) -> Parsed raise @common.AgentError {
  match @agentHost.parse_agent_id(value) {
    Ok((type_name, input, phantom_id)) =>
      { type_name, input, phantom_id: phantom_id.map(@schema.Uuid::from_wit), }
    Err(error) => raise error
  }
}

///|
pub fn Parsed::constructor_value(
  self : Parsed,
) -> @model.SchemaValue raise @model.SchemaModelError {
  @model_host.typed_schema_value_from_wit(self.input).value
}
