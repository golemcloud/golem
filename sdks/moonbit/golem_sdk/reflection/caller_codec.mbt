///|
/// Typed client whose `IntoSchema` and `FromSchema` implementations are
/// authored by the caller. The environment-unique type name is the invocation
/// address; no reflected schema is consulted.
pub(all) struct FullAgentClient {
  priv transport : Transport
  reusable_identity : ParsedAgentId?
}

///|
/// Typed method codecs bound to an existing durable identity. The ID itself
/// supplies the target name, constructor value, and phantom identity.
pub(all) struct MethodOnlyAgentClient {
  priv inner : FullAgentClient
}

///|
fn[C : @schema.IntoSchema] full_client_create(
  type_name : String,
  constructor_input : C,
  phantom_id : @types.Uuid?,
  agent_config : Array[@common.TypedAgentConfigValue],
  reusable : Bool,
) -> FullAgentClient raise ReflectError {
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
/// Constructs an ordinary durable client from caller-owned schemas and codecs.
pub fn[C : @schema.IntoSchema] FullAgentClient::get(
  type_name : String,
  constructor_input : C,
) -> FullAgentClient raise ReflectError {
  full_client_create(type_name, constructor_input, None, [], true)
}

///|
pub fn[C : @schema.IntoSchema, Config : @rpc.AgentConfigOverrides] FullAgentClient::get_with_config(
  type_name : String,
  constructor_input : C,
  config : Config,
) -> FullAgentClient raise ReflectError {
  let values = config.typed_values() catch {
    error =>
      raise Schema(
        "failed to encode config overrides: " + agent_error_message(error),
      )
  }
  full_client_create(type_name, constructor_input, None, values, true)
}

///|
/// Constructs a durable phantom client with a known phantom identity.
pub fn[C : @schema.IntoSchema] FullAgentClient::get_phantom(
  type_name : String,
  constructor_input : C,
  phantom_id : @types.Uuid,
) -> FullAgentClient raise ReflectError {
  full_client_create(type_name, constructor_input, Some(phantom_id), [], true)
}

///|
pub fn[C : @schema.IntoSchema, Config : @rpc.AgentConfigOverrides] FullAgentClient::get_phantom_with_config(
  type_name : String,
  constructor_input : C,
  phantom_id : @types.Uuid,
  config : Config,
) -> FullAgentClient raise ReflectError {
  let values = config.typed_values() catch {
    error =>
      raise Schema(
        "failed to encode config overrides: " + agent_error_message(error),
      )
  }
  full_client_create(
    type_name,
    constructor_input,
    Some(phantom_id),
    values,
    true,
  )
}

///|
/// Constructs a durable phantom client with a fresh phantom identity.
pub fn[C : @schema.IntoSchema] FullAgentClient::new_phantom(
  type_name : String,
  constructor_input : C,
) -> FullAgentClient raise ReflectError {
  let phantom_id = @apiHost.generate_idempotency_key()
  FullAgentClient::get_phantom(type_name, constructor_input, phantom_id)
}

///|
pub fn[C : @schema.IntoSchema, Config : @rpc.AgentConfigOverrides] FullAgentClient::new_phantom_with_config(
  type_name : String,
  constructor_input : C,
  config : Config,
) -> FullAgentClient raise ReflectError {
  let phantom_id = @apiHost.generate_idempotency_key()
  FullAgentClient::get_phantom_with_config(
    type_name, constructor_input, phantom_id, config,
  )
}

///|
/// Constructs the reusable identity for a full caller-defined client.
pub fn[C : @schema.IntoSchema] FullAgentClient::agent_id(
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
/// Binds a full caller-defined client, rejecting an identity whose agent name
/// or constructor shape differs from the caller-owned definition.
pub fn[C : @schema.FromSchema] FullAgentClient::bind(
  type_name : String,
  agent_id : ParsedAgentId,
  constructor_type : @schema.TypeTag[C],
) -> FullAgentClient raise ReflectError {
  let parts = agent_id.parts()
  validate_full_id(type_name, parts, constructor_type)
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
pub fn[C : @schema.FromSchema, Config : @rpc.AgentConfigOverrides] FullAgentClient::bind_with_config(
  type_name : String,
  agent_id : ParsedAgentId,
  constructor_type : @schema.TypeTag[C],
  config : Config,
) -> FullAgentClient raise ReflectError {
  let parts = agent_id.parts()
  validate_full_id(type_name, parts, constructor_type)
  let values = config.typed_values() catch {
    error =>
      raise Schema(
        "failed to encode config overrides: " + agent_error_message(error),
      )
  }
  full_from_id_with_config(agent_id, values)
}

///|
fn[C : @schema.FromSchema] validate_full_id(
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
        "agent ID constructor does not match the caller-defined schema: \{repr(error)}",
      )
  }
}

///|
/// Constructs a logical ephemeral client. Its final identity is available only
/// in invocation metadata.
pub fn[C : @schema.IntoSchema] FullAgentClient::ephemeral(
  type_name : String,
  constructor_input : C,
) -> FullAgentClient raise ReflectError {
  full_client_create(type_name, constructor_input, None, [], false)
}

///|
pub fn[C : @schema.IntoSchema, Config : @rpc.AgentConfigOverrides] FullAgentClient::ephemeral_with_config(
  type_name : String,
  constructor_input : C,
  config : Config,
) -> FullAgentClient raise ReflectError {
  let values = config.typed_values() catch {
    error =>
      raise Schema(
        "failed to encode config overrides: " + agent_error_message(error),
      )
  }
  full_client_create(type_name, constructor_input, None, values, false)
}

///|
/// Constructs an ephemeral client with a known phantom identity.
pub fn[C : @schema.IntoSchema] FullAgentClient::ephemeral_phantom(
  type_name : String,
  constructor_input : C,
  phantom_id : @types.Uuid,
) -> FullAgentClient raise ReflectError {
  full_client_create(type_name, constructor_input, Some(phantom_id), [], true)
}

///|
pub fn[C : @schema.IntoSchema, Config : @rpc.AgentConfigOverrides] FullAgentClient::ephemeral_phantom_with_config(
  type_name : String,
  constructor_input : C,
  phantom_id : @types.Uuid,
  config : Config,
) -> FullAgentClient raise ReflectError {
  let values = config.typed_values() catch {
    error =>
      raise Schema(
        "failed to encode config overrides: " + agent_error_message(error),
      )
  }
  full_client_create(
    type_name,
    constructor_input,
    Some(phantom_id),
    values,
    true,
  )
}

///|
pub fn MethodOnlyAgentClient::bind(
  agent_id : ParsedAgentId,
) -> MethodOnlyAgentClient raise ReflectError {
  MethodOnlyAgentClient::bind_with_config(agent_id, [])
}

///|
/// Raw typed entries are accepted without declaration-aware validation.
pub fn MethodOnlyAgentClient::bind_with_config(
  agent_id : ParsedAgentId,
  config : Array[@common.TypedAgentConfigValue],
) -> MethodOnlyAgentClient raise ReflectError {
  { inner: full_from_id_with_config(agent_id, config), }
}

///|
fn full_from_id_with_config(
  agent_id : ParsedAgentId,
  config : Array[@common.TypedAgentConfigValue],
) -> FullAgentClient raise ReflectError {
  let parts = agent_id.parts()
  {
    transport: Transport::create_with_config(
      parts.type_name,
      parts.constructor_value,
      parts.phantom_id,
      config,
    ),
    reusable_identity: Some(agent_id),
  }
}

///|
pub async fn[I : @schema.IntoSchema, O : @schema.FromSchema] FullAgentClient::invoke(
  self : FullAgentClient,
  method_name : String,
  input : I,
) -> Invocation[O] {
  let invocation = self.transport.invoke_value(
    method_name,
    @schema.to_value_as(input),
  )
  let value = match invocation.value {
    Some(value) => decode_typed_output(value)
    None => raise ReflectError::Schema("typed method returned unit")
  }
  { metadata: invocation.metadata, value, }
}

///|
pub async fn[I : @schema.IntoSchema] FullAgentClient::invoke_unit(
  self : FullAgentClient,
  method_name : String,
  input : I,
) -> Invocation[Unit] {
  let invocation = self.transport.invoke_value(
    method_name,
    @schema.to_value_as(input),
  )
  require_unit_output(invocation.value)
  { metadata: invocation.metadata, value: (), }
}

///|
fn[O : @schema.FromSchema] decode_typed_output(
  value : @model.SchemaValue,
) -> O raise ReflectError {
  errdefer value.release_resources(@model_host.resource_drops())
  @schema.from_value_as(value) catch {
    error =>
      raise ReflectError::Schema(
        "failed to decode typed output: \{repr(error)}",
      )
  }
}

///|
fn require_unit_output(value : @model.SchemaValue?) -> Unit raise ReflectError {
  match value {
    None => ()
    Some(value) => {
      value.release_resources(@model_host.resource_drops())
      raise ReflectError::Schema("unit method returned a value")
    }
  }
}

///|
pub fn[I : @schema.IntoSchema, O : @schema.IntoSchema] FullAgentClient::trigger(
  self : FullAgentClient,
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
pub fn[I : @schema.IntoSchema, O : @schema.IntoSchema] FullAgentClient::schedule(
  self : FullAgentClient,
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
pub fn FullAgentClient::drop(self : FullAgentClient) -> Unit {
  self.transport.drop()
}

///|
pub async fn[I : @schema.IntoSchema, O : @schema.FromSchema] MethodOnlyAgentClient::invoke(
  self : MethodOnlyAgentClient,
  method_name : String,
  input : I,
) -> Invocation[O] {
  self.inner.invoke(method_name, input)
}

///|
pub async fn[I : @schema.IntoSchema] MethodOnlyAgentClient::invoke_unit(
  self : MethodOnlyAgentClient,
  method_name : String,
  input : I,
) -> Invocation[Unit] {
  self.inner.invoke_unit(method_name, input)
}

///|
pub fn[I : @schema.IntoSchema, O : @schema.IntoSchema] MethodOnlyAgentClient::trigger(
  self : MethodOnlyAgentClient,
  method_name : String,
  input : I,
  output : @schema.TypeTag[O],
) -> InvocationMetadata raise ReflectError {
  self.inner.trigger(method_name, input, output)
}

///|
pub fn[I : @schema.IntoSchema, O : @schema.IntoSchema] MethodOnlyAgentClient::schedule(
  self : MethodOnlyAgentClient,
  scheduled_at : @systemClock.Instant,
  method_name : String,
  input : I,
  output : @schema.TypeTag[O],
) -> ScheduledInvocation raise ReflectError {
  self.inner.schedule(scheduled_at, method_name, input, output)
}

///|
pub fn MethodOnlyAgentClient::drop(self : MethodOnlyAgentClient) -> Unit {
  self.inner.drop()
}
