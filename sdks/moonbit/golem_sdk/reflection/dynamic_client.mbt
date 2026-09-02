///|
/// Schema-free client for manually packed `SchemaValue` calls. Constructing
/// this client performs no discovery or local lifecycle verification.
pub(all) struct DynamicAgentClient {
  priv transport : Transport
  reusable_identity : AgentId?
}

///|
pub fn DynamicAgentClient::from_agent_id(
  agent_id : AgentId,
) -> DynamicAgentClient raise ReflectError {
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
/// Attempts a durable lookup by manually constructing an identity without a
/// phantom UUID. The runtime authoritatively accepts or rejects the operation.
pub fn DynamicAgentClient::lookup(
  component_id : @types.ComponentId,
  type_name : String,
  constructor_value : @model.SchemaValue,
) -> DynamicAgentClient raise ReflectError {
  DynamicAgentClient::from_agent_id(
    AgentId::create(component_id, type_name, constructor_value),
  )
}

///|
/// Attempts to resume a durable phantom from a known UUID.
pub fn DynamicAgentClient::resume_phantom(
  component_id : @types.ComponentId,
  type_name : String,
  constructor_value : @model.SchemaValue,
  phantom_id : @types.Uuid,
) -> DynamicAgentClient raise ReflectError {
  DynamicAgentClient::from_agent_id(
    AgentId::create(
      component_id,
      type_name,
      constructor_value,
      phantom_id=Some(phantom_id),
    ),
  )
}

///|
/// Generates a phantom UUID and exposes both the reusable agent identity and
/// the UUID used to construct it.
pub fn DynamicAgentClient::new_phantom(
  component_id : @types.ComponentId,
  type_name : String,
  constructor_value : @model.SchemaValue,
) -> CreatedPhantomClient raise ReflectError {
  let phantom_id = @apiHost.generate_idempotency_key()
  let agent_id = AgentId::create(
    component_id,
    type_name,
    constructor_value,
    phantom_id=Some(phantom_id),
  )
  { agent_id, phantom_id, client: DynamicAgentClient::from_agent_id(agent_id), }
}

///|
/// Constructs a raw ephemeral invocation address. No reusable pre-invocation
/// identity is guaranteed; use the final identity returned in invocation
/// metadata.
pub fn DynamicAgentClient::ephemeral(
  component_id : @types.ComponentId,
  type_name : String,
  constructor_value : @model.SchemaValue,
) -> DynamicAgentClient raise ReflectError {
  {
    transport: Transport::create(
      component_id,
      type_name,
      constructor_value,
      None,
    ),
    reusable_identity: None,
  }
}

///|
pub async fn DynamicAgentClient::invoke_value(
  self : DynamicAgentClient,
  method_name : String,
  input : @model.SchemaValue,
) -> Invocation[@model.SchemaValue?] {
  self.transport.invoke_value(method_name, input)
}

///|
pub fn DynamicAgentClient::trigger_value(
  self : DynamicAgentClient,
  method_name : String,
  input : @model.SchemaValue,
) -> InvocationMetadata raise ReflectError {
  self.transport.trigger_value(method_name, input)
}

///|
pub fn DynamicAgentClient::schedule_value(
  self : DynamicAgentClient,
  scheduled_at : @systemClock.Instant,
  method_name : String,
  input : @model.SchemaValue,
) -> ScheduledInvocation raise ReflectError {
  self.transport.schedule_value(scheduled_at, method_name, input)
}

///|
pub fn DynamicAgentClient::drop(self : DynamicAgentClient) -> Unit {
  self.transport.drop()
}
