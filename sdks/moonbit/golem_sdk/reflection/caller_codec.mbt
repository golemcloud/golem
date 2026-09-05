///|
/// Typed client whose `IntoSchema` and `FromSchema` implementations are
/// authored by the caller. Runtime lookup supplies only the component metadata
/// for the environment-unique type name; it does not supply or verify codecs.
pub(all) struct CallerCodecClient {
  priv transport : Transport
  reusable_identity : AgentId?
}

///|
pub fn[C : @schema.IntoSchema] CallerCodecClient::create(
  type_name : String,
  constructor_input : C,
  phantom_id? : @types.Uuid? = None,
) -> CallerCodecClient raise ReflectError {
  let component_id = component_id_for(type_name)
  let constructor_value = @schema.to_value_as(constructor_input)
  let identity = AgentId::create(
    component_id,
    type_name,
    constructor_value,
    phantom_id~,
  )
  {
    transport: Transport::create(
      component_id, type_name, constructor_value, phantom_id,
    ),
    reusable_identity: Some(identity),
  }
}

///|
pub fn CallerCodecClient::bind(
  agent_id : AgentId,
  type_name : String,
) -> CallerCodecClient raise ReflectError {
  let parts = agent_id.parts()
  guard parts.type_name == type_name else {
    raise Identity(
      "caller-codec contract '" +
      type_name +
      "' cannot bind '" +
      parts.type_name +
      "'",
    )
  }
  let component_id = component_id_for(type_name)
  {
    transport: Transport::create(
      component_id,
      type_name,
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
