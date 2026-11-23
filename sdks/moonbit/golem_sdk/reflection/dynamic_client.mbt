///|
/// Fully dynamic client for manually packed `SchemaValue` calls. Constructing
/// this client performs no discovery or local lifecycle verification; the
/// runtime authoritatively accepts or rejects every operation.
pub(all) struct DynamicAgentClient {
  priv transport : Transport
}

///|
/// Connects to the keyed agent of the constructor value, which the host
/// creates when it is missing. For an ephemeral type every invocation runs on
/// a new one-shot instance, whose ID the invocation metadata returns.
pub fn DynamicAgentClient::upsert(
  type_name : String,
  constructor_value : @model.SchemaValue,
) -> DynamicAgentClient raise ReflectError {
  { transport: Transport::upsert(type_name, constructor_value), }
}

///|
/// Connects to the existing agent with the given ID. An unknown ID, or the ID
/// of an agent of another type, raises `Remote(NotFound(_))`.
pub fn DynamicAgentClient::get_by_id(
  type_name : String,
  agent_id : @schema.AgentId,
) -> DynamicAgentClient raise ReflectError {
  { transport: Transport::get_by_id(type_name, agent_id), }
}

///|
/// Creates a new durable agent. With `keyed`, the new agent is the keyed agent
/// of the constructor value, and the creation raises `Remote(Conflict(_))`
/// when that agent already exists; otherwise it has no key and a new ID.
/// `config` entries are accepted without declaration-aware validation, and
/// `env` adds environment variables to the ones the manifest defines.
pub async fn DynamicAgentClient::create(
  type_name : String,
  constructor_value : @model.SchemaValue,
  keyed~ : Bool = false,
  config~ : Array[@common.TypedAgentConfigValue] = [],
  env~ : Array[(String, String)] = [],
) -> DynamicAgentClient {
  {
    transport: Transport::create(
      type_name, constructor_value, keyed, config, env,
    ),
  }
}

///|
/// The ID of the agent this client connects to. On an upsert client the host
/// creates the agent when it is missing, without invoking a method. An
/// ephemeral type has no agent ID before an invocation.
pub async fn DynamicAgentClient::agent_id(
  self : DynamicAgentClient,
) -> @schema.AgentId {
  self.transport.agent_id()
}

///|
/// Sets the scope card that awaited invocations of this client attach when the
/// call does not pass its own `scope_card`. `None` removes the default. The
/// client borrows the card; trigger and schedule invocations never attach it.
pub fn DynamicAgentClient::set_default_scope_card(
  self : DynamicAgentClient,
  card : @permissions.ScopeCard?,
) -> Unit {
  self.transport.default_scope_card = card
}

///|
/// `scope_card` lends an invocation-local scope card to the callee for this
/// call and overrides the client default. The card is borrowed, not consumed.
pub async fn DynamicAgentClient::invoke_value(
  self : DynamicAgentClient,
  method_name : String,
  input : @model.SchemaValue,
  scope_card? : @permissions.ScopeCard,
) -> Invocation[@model.SchemaValue?] {
  self.transport.invoke_value(method_name, input, scope_card)
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
