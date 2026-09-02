///|
/// Discovery-free typed client whose `IntoSchema` and `FromSchema`
/// implementations are authored by the caller. The runtime does not supply or
/// verify these codecs.
pub(all) struct CallerCodecClient {
  priv transport : Transport
  reusable_identity : AgentId?
}

///|
pub fn[C : @schema.IntoSchema] CallerCodecClient::create(
  component_id : @types.ComponentId,
  type_name : String,
  constructor_input : C,
  phantom_id? : @types.Uuid? = None,
) -> CallerCodecClient raise ReflectError {
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
) -> CallerCodecClient raise ReflectError {
  let parts = agent_id.parts()
  {
    transport: Transport::create(
      agent_id.component_id,
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
pub fn[I : @schema.IntoSchema] CallerCodecClient::trigger(
  self : CallerCodecClient,
  method_name : String,
  input : I,
) -> InvocationMetadata raise ReflectError {
  self.transport.trigger_value(method_name, @schema.to_value_as(input))
}

///|
pub fn[I : @schema.IntoSchema] CallerCodecClient::schedule(
  self : CallerCodecClient,
  scheduled_at : @systemClock.Instant,
  method_name : String,
  input : I,
) -> ScheduledInvocation raise ReflectError {
  self.transport.schedule_value(
    scheduled_at,
    method_name,
    @schema.to_value_as(input),
  )
}

///|
pub fn CallerCodecClient::drop(self : CallerCodecClient) -> Unit {
  self.transport.drop()
}
