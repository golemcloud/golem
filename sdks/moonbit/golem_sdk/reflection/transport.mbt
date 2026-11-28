///|
priv struct Transport {
  raw : @agentHost.WasmRpc
  mut default_scope_card : @permissions.ScopeCard?
}

///|
fn Transport::new(raw : @agentHost.WasmRpc) -> Transport {
  { raw, default_scope_card: None, }
}

///|
fn encode_constructor(
  constructor_value : @model.SchemaValue,
) -> @types.SchemaValueTree raise ReflectError {
  @model.schema_value_to_wit(constructor_value) catch {
    error => raise Schema("failed to encode constructor value: \{repr(error)}")
  }
}

///|
/// Connects through the host `upsert`: the keyed agent of the constructor
/// value, created when it is missing, or a one-shot instance per invocation
/// for an ephemeral type.
fn Transport::upsert(
  type_name : String,
  constructor_value : @model.SchemaValue,
) -> Transport raise ReflectError {
  let input = encode_constructor(constructor_value)
  match @agentHost.WasmRpc::upsert(type_name, input) {
    Ok(raw) => Transport::new(raw)
    Err(error) => raise rpc_error(error)
  }
}

///|
/// Connects to the existing agent with the given ID; never creates one.
fn Transport::get_by_id(
  type_name : String,
  agent_id : @schema.AgentId,
) -> Transport raise ReflectError {
  match @agentHost.WasmRpc::get_by_id(type_name, agent_id.to_wit()) {
    Ok(raw) => Transport::new(raw)
    Err(error) => raise rpc_error(error)
  }
}

///|
/// Creates a new durable agent through the host `create`.
async fn Transport::create(
  type_name : String,
  constructor_value : @model.SchemaValue,
  keyed : Bool,
  agent_config : Array[@common.TypedAgentConfigValue],
  env : Array[(String, String)],
) -> Transport {
  let input = encode_constructor(constructor_value)
  match
    @agentHost.WasmRpc::create(type_name, input, { keyed, agent_config, env, }) {
    Ok(created) => Transport::new(created.client)
    Err(error) => raise rpc_error(error)
  }
}

///|
async fn Transport::agent_id(self : Transport) -> @schema.AgentId {
  match self.raw.agent_id() {
    Ok(id) => @schema.AgentId::from_wit(id)
    Err(error) => raise rpc_error(error)
  }
}

///|
fn invocation_metadata(
  metadata : @agentHost.InvocationMetadata,
) -> InvocationMetadata {
  {
    agent_id: @schema.AgentId::from_wit(metadata.agent_id),
    invocation_id: @rpc.InvocationId::from_host(metadata),
  }
}

///|
async fn Transport::invoke_value(
  self : Transport,
  method_name : String,
  input : @model.SchemaValue,
  scope_card : @permissions.ScopeCard?,
) -> InvocationOutput[@model.SchemaValue?] {
  // Check the scope card first: a dropped scope card aborts before the input
  // encoding moves any argument card. The encoding can suspend, so the raw
  // handle is resolved again after it.
  ignore(@scope_card.resolve(scope_card, self.default_scope_card))
  let wire = encode_awaited_input(input)
  let result = self.raw.invoke_and_await(
    method_name,
    wire,
    @scope_card.resolve(scope_card, self.default_scope_card),
  )
  match result {
    Ok(completed) =>
      {
        metadata: invocation_metadata(completed.metadata),
        value: decode_output_value(completed.result),
      }
    Err(error) => raise rpc_error(error)
  }
}

///|
/// Reads an invocation result into the schema model.
fn decode_output_value(
  output : @types.SchemaValueTree?,
) -> @model.SchemaValue? raise ReflectError {
  match output {
    Some(output) => {
      let decoded = @model_host.schema_value_from_wit(output) catch {
        error =>
          raise Schema("failed to decode invocation output: \{repr(error)}")
      }
      Some(decoded)
    }
    None => None
  }
}

///|
async fn encode_awaited_input(
  input : @model.SchemaValue,
) -> @types.SchemaValueTree {
  errdefer input.rollback_input_resources(@model_host.resource_drops())
  let wire = @model.schema_value_to_wit_async(input) catch {
    error => raise Schema("failed to encode invocation input: \{repr(error)}")
  }
  wire
}

///|
/// Triggers a method and returns the handle of the invocation, whose result
/// `decode` reads from the schema model.
async fn[T] Transport::trigger_value(
  self : Transport,
  method_name : String,
  input : @model.SchemaValue,
  decode : (@model.SchemaValue?) -> T raise,
) -> @rpc.Invocation[T] {
  let wire = @model.schema_value_to_wit(input) catch {
    error => raise Schema("failed to encode invocation input: \{repr(error)}")
  }
  match self.raw.trigger(method_name, wire) {
    Ok(raw) =>
      @rpc.Invocation::from_raw(raw, output => {
        decode(decode_output_value(output))
      })
    Err(error) => raise rpc_error(error)
  }
}

///|
/// Schedules a method and returns the handle of the invocation, whose result
/// `decode` reads from the schema model.
async fn[T] Transport::schedule_value(
  self : Transport,
  scheduled_at : @systemClock.Instant,
  method_name : String,
  input : @model.SchemaValue,
  decode : (@model.SchemaValue?) -> T raise,
) -> @rpc.Invocation[T] {
  let wire = @model.schema_value_to_wit(input) catch {
    error => raise Schema("failed to encode invocation input: \{repr(error)}")
  }
  match self.raw.schedule(scheduled_at, method_name, wire) {
    Ok(raw) =>
      @rpc.Invocation::from_raw(raw, output => {
        decode(decode_output_value(output))
      })
    Err(error) => raise rpc_error(error)
  }
}

///|
fn Transport::drop(self : Transport) -> Unit {
  self.raw.drop()
}
