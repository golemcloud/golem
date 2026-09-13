///|
/// Typed client whose `IntoSchema` and `FromSchema` implementations are
/// authored by the caller. The environment-unique type name is the invocation
/// address; no reflected schema is consulted.
pub(all) struct CallerCodecClient {
  priv transport : Transport
  reusable_identity : ParsedAgentId?
}

///|
fn[C : @schema.IntoSchema] caller_codec_complete(
  type_name : String,
  constructor_input : C,
  phantom_id : @types.Uuid?,
  agent_config : Array[@common.TypedAgentConfigValue],
  reusable : Bool,
) -> CallerCodecClient raise ReflectError {
  let constructor_value = @schema.to_value_as(constructor_input)
  let reusable_identity = if reusable {
    Some(ParsedAgentId::create(type_name, constructor_value, phantom_id~))
  } else {
    None
  }
  {
    transport: Transport::create_with_config(
      type_name, constructor_value, phantom_id, agent_config,
    ),
    reusable_identity,
  }
}

///|
/// Constructs an ordinary durable client from its complete local contract.
pub fn[C : @schema.IntoSchema] CallerCodecClient::get(
  type_name : String,
  constructor_input : C,
) -> CallerCodecClient raise ReflectError {
  caller_codec_complete(type_name, constructor_input, None, [], true)
}

///|
pub fn[C : @schema.IntoSchema, Config : @rpc.AgentConfigOverrides] CallerCodecClient::get_with_config(
  type_name : String,
  constructor_input : C,
  config : Config,
) -> CallerCodecClient raise ReflectError {
  let values = config.typed_values() catch {
    error =>
      raise Schema(
        "failed to encode config overrides: " + agent_error_message(error),
      )
  }
  caller_codec_complete(type_name, constructor_input, None, values, true)
}

///|
/// Constructs a durable phantom client with a known phantom identity.
pub fn[C : @schema.IntoSchema] CallerCodecClient::get_phantom(
  type_name : String,
  constructor_input : C,
  phantom_id : @types.Uuid,
) -> CallerCodecClient raise ReflectError {
  caller_codec_complete(
    type_name,
    constructor_input,
    Some(phantom_id),
    [],
    true,
  )
}

///|
pub fn[C : @schema.IntoSchema, Config : @rpc.AgentConfigOverrides] CallerCodecClient::get_phantom_with_config(
  type_name : String,
  constructor_input : C,
  phantom_id : @types.Uuid,
  config : Config,
) -> CallerCodecClient raise ReflectError {
  let values = config.typed_values() catch {
    error =>
      raise Schema(
        "failed to encode config overrides: " + agent_error_message(error),
      )
  }
  caller_codec_complete(
    type_name,
    constructor_input,
    Some(phantom_id),
    values,
    true,
  )
}

///|
/// Constructs a durable phantom client with a fresh phantom identity.
pub fn[C : @schema.IntoSchema] CallerCodecClient::new_phantom(
  type_name : String,
  constructor_input : C,
) -> CallerCodecClient raise ReflectError {
  let phantom_id = @apiHost.generate_idempotency_key()
  CallerCodecClient::get_phantom(type_name, constructor_input, phantom_id)
}

///|
pub fn[C : @schema.IntoSchema, Config : @rpc.AgentConfigOverrides] CallerCodecClient::new_phantom_with_config(
  type_name : String,
  constructor_input : C,
  config : Config,
) -> CallerCodecClient raise ReflectError {
  let phantom_id = @apiHost.generate_idempotency_key()
  CallerCodecClient::get_phantom_with_config(
    type_name, constructor_input, phantom_id, config,
  )
}

///|
/// Constructs the reusable identity for a complete durable contract.
pub fn[C : @schema.IntoSchema] CallerCodecClient::agent_id(
  type_name : String,
  constructor_input : C,
  phantom_id? : @types.Uuid? = None,
) -> ParsedAgentId raise ReflectError {
  ParsedAgentId::create(
    type_name,
    @schema.to_value_as(constructor_input),
    phantom_id~,
  )
}

///|
/// Binds a complete durable contract, rejecting an identity whose exact agent
/// name or constructor shape differs from the local contract.
pub fn[C : @schema.FromSchema] CallerCodecClient::bind_complete(
  type_name : String,
  agent_id : ParsedAgentId,
  constructor_type : @schema.TypeTag[C],
) -> CallerCodecClient raise ReflectError {
  let parts = agent_id.parts()
  validate_complete_id(type_name, parts, constructor_type)
  {
    transport: Transport::create(
      parts.type_name,
      parts.constructor_value,
      parts.phantom_id,
    ),
    reusable_identity: Some(agent_id),
  }
}

///|
fn[C : @schema.FromSchema] validate_complete_id(
  type_name : String,
  parts : ParsedAgentIdParts,
  _constructor_type : @schema.TypeTag[C],
) -> Unit raise ReflectError {
  guard parts.type_name == type_name else {
    raise Identity(
      "agent ID names '" + parts.type_name + "', expected '" + type_name + "'",
    )
  }
  let _ : C = @schema.from_value_as(parts.constructor_value) catch {
    error =>
      raise Identity(
        "agent ID constructor does not match the complete contract: \{repr(error)}",
      )
  }
}

///|
/// Constructs a logical ephemeral client. Its final identity is available only
/// in invocation metadata.
pub fn[C : @schema.IntoSchema] CallerCodecClient::ephemeral(
  type_name : String,
  constructor_input : C,
) -> CallerCodecClient raise ReflectError {
  caller_codec_complete(type_name, constructor_input, None, [], false)
}

///|
pub fn[C : @schema.IntoSchema, Config : @rpc.AgentConfigOverrides] CallerCodecClient::ephemeral_with_config(
  type_name : String,
  constructor_input : C,
  config : Config,
) -> CallerCodecClient raise ReflectError {
  let values = config.typed_values() catch {
    error =>
      raise Schema(
        "failed to encode config overrides: " + agent_error_message(error),
      )
  }
  caller_codec_complete(type_name, constructor_input, None, values, false)
}

///|
/// Constructs an ephemeral client with a known phantom identity.
pub fn[C : @schema.IntoSchema] CallerCodecClient::ephemeral_phantom(
  type_name : String,
  constructor_input : C,
  phantom_id : @types.Uuid,
) -> CallerCodecClient raise ReflectError {
  caller_codec_complete(
    type_name,
    constructor_input,
    Some(phantom_id),
    [],
    true,
  )
}

///|
pub fn[C : @schema.IntoSchema, Config : @rpc.AgentConfigOverrides] CallerCodecClient::ephemeral_phantom_with_config(
  type_name : String,
  constructor_input : C,
  phantom_id : @types.Uuid,
  config : Config,
) -> CallerCodecClient raise ReflectError {
  let values = config.typed_values() catch {
    error =>
      raise Schema(
        "failed to encode config overrides: " + agent_error_message(error),
      )
  }
  caller_codec_complete(
    type_name,
    constructor_input,
    Some(phantom_id),
    values,
    true,
  )
}

///|
pub fn CallerCodecClient::bind(
  agent_id : ParsedAgentId,
) -> CallerCodecClient raise ReflectError {
  let parts = agent_id.parts()
  {
    transport: Transport::create(
      parts.type_name,
      parts.constructor_value,
      parts.phantom_id,
    ),
    reusable_identity: Some(agent_id),
  }
}

///|
pub async fn[I : @schema.IntoSchema, O : @schema.FromSchema] CallerCodecClient::invoke(
  self : CallerCodecClient,
  method_name : String,
  input : I,
) -> Invocation[O] {
  let invocation = self.transport.invoke_value(
    method_name,
    @schema.to_value_as(input),
  )
  let value = match invocation.value {
    Some(value) =>
      @schema.from_value_as(value) catch {
        error =>
          raise ReflectError::Schema(
            "failed to decode typed output: \{repr(error)}",
          )
      }
    None => raise ReflectError::Schema("typed method returned unit")
  }
  { metadata: invocation.metadata, value, }
}

///|
pub async fn[I : @schema.IntoSchema] CallerCodecClient::invoke_unit(
  self : CallerCodecClient,
  method_name : String,
  input : I,
) -> Invocation[Unit] {
  let invocation = self.transport.invoke_value(
    method_name,
    @schema.to_value_as(input),
  )
  guard invocation.value is None else {
    raise ReflectError::Schema("unit method returned a value")
  }
  { metadata: invocation.metadata, value: (), }
}

///|
pub fn[I : @schema.IntoSchema, O : @schema.IntoSchema] CallerCodecClient::trigger(
  self : CallerCodecClient,
  method_name : String,
  input : I,
  output : @schema.TypeTag[O],
) -> InvocationMetadata raise ReflectError {
  reject_caller_codec_non_awaited_streams(
    method_name,
    "trigger",
    (@schema.type_tag() : @schema.TypeTag[I]),
    output,
  )
  self.transport.trigger_value(method_name, @schema.to_value_as(input))
}

///|
pub fn[I : @schema.IntoSchema, O : @schema.IntoSchema] CallerCodecClient::schedule(
  self : CallerCodecClient,
  scheduled_at : @systemClock.Instant,
  method_name : String,
  input : I,
  output : @schema.TypeTag[O],
) -> ScheduledInvocation raise ReflectError {
  reject_caller_codec_non_awaited_streams(
    method_name,
    "schedule",
    (@schema.type_tag() : @schema.TypeTag[I]),
    output,
  )
  self.transport.schedule_value(
    scheduled_at,
    method_name,
    @schema.to_value_as(input),
  )
}

///|
fn[I : @schema.IntoSchema, O : @schema.IntoSchema] reject_caller_codec_non_awaited_streams(
  method_name : String,
  operation : String,
  input : @schema.TypeTag[I],
  output : @schema.TypeTag[O],
) -> Unit raise ReflectError {
  let input_graph = @schema.schema_graph_of_tag(input) catch {
    error => raise Schema("failed to build caller-owned schema: \{repr(error)}")
  }
  let output_graph = @schema.schema_graph_of_tag(output) catch {
    error => raise Schema("failed to build caller-owned schema: \{repr(error)}")
  }
  guard !SchemaRef::new(input_graph).contains_stream() &&
    !SchemaRef::new(output_graph).contains_stream() else {
    raise Schema(
      operation + " is unavailable for streaming method '" + method_name + "'",
    )
  }
}

///|
pub fn CallerCodecClient::drop(self : CallerCodecClient) -> Unit {
  self.transport.drop()
}
