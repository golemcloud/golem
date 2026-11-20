///|
/// Typed client whose `IntoSchema` and `FromSchema` implementations are
/// authored by the caller. The environment-unique type name is the invocation
/// address; no reflected schema is consulted.
pub(all) struct FullAgentClient {
  priv transport : Transport
}

///|
/// Typed method codecs bound to an existing durable agent. The host resolves
/// the agent type from the type tag of the ID.
pub(all) struct MethodOnlyAgentClient {
  priv inner : FullAgentClient
}

///|
/// Connects to the keyed agent of the constructor input, which the host
/// creates when it is missing.
pub fn[C : @schema.IntoSchema] FullAgentClient::upsert(
  type_name : String,
  constructor_input : C,
) -> FullAgentClient raise ReflectError {
  {
    transport: Transport::upsert(
      type_name,
      @schema.to_value_as(constructor_input),
    ),
  }
}

///|
/// Connects to the existing agent of the type with the given ID. An unknown
/// ID, or the ID of an agent of another type, raises `Remote(NotFound(_))`.
pub fn FullAgentClient::get_by_id(
  type_name : String,
  agent_id : @schema.AgentId,
) -> FullAgentClient raise ReflectError {
  { transport: Transport::get_by_id(type_name, agent_id), }
}

///|
/// Creates a new durable agent. With `keyed`, the new agent is the keyed agent
/// of the constructor input, and the creation raises `Remote(Conflict(_))`
/// when that agent already exists; otherwise it has no key and a new ID.
/// `config` overrides values of the agent type's local configuration, and
/// `env` adds environment variables to the ones the manifest defines.
pub async fn[C : @schema.IntoSchema] FullAgentClient::create(
  type_name : String,
  constructor_input : C,
  keyed~ : Bool = false,
  config? : &@rpc.AgentConfigOverrides,
  env~ : Array[(String, String)] = [],
) -> FullAgentClient {
  let values = match config {
    Some(config) =>
      config.typed_values() catch {
        error =>
          raise Schema(
            "failed to encode config overrides: " + agent_error_message(error),
          )
      }
    None => []
  }
  {
    transport: Transport::create(
      type_name,
      @schema.to_value_as(constructor_input),
      keyed,
      values,
      env,
    ),
  }
}

///|
/// Addresses an ephemeral agent type: every invocation runs on a new one-shot
/// instance, whose ID the invocation metadata returns.
pub fn[C : @schema.IntoSchema] FullAgentClient::ephemeral(
  type_name : String,
  constructor_input : C,
) -> FullAgentClient raise ReflectError {
  FullAgentClient::upsert(type_name, constructor_input)
}

///|
/// The ID of the agent this client connects to. On an upsert client the host
/// creates the agent when it is missing, without invoking a method.
pub async fn FullAgentClient::agent_id(
  self : FullAgentClient,
) -> @schema.AgentId {
  self.transport.agent_id()
}

///|
/// Binds to the existing agent with the given ID. Raises `Discovery` when no
/// deployed agent type has the type tag of the ID, and `Remote(NotFound(_))`
/// when no agent has the ID.
pub fn MethodOnlyAgentClient::bind(
  agent_id : @schema.AgentId,
) -> MethodOnlyAgentClient raise ReflectError {
  guard @agentHost.get_agent_type_by_agent_id(agent_id.to_wit())
    is Some(registered) else {
    raise Discovery("no deployed agent type has the type of agent \{agent_id}")
  }
  {
    inner: FullAgentClient::get_by_id(registered.agent_type.type_name, agent_id),
  }
}

///|
/// The ID of the agent this client is bound to.
pub async fn MethodOnlyAgentClient::agent_id(
  self : MethodOnlyAgentClient,
) -> @schema.AgentId {
  self.inner.agent_id()
}

///|
/// Sets the scope card that awaited invocations of this client attach when the
/// call does not pass its own `scope_card`. `None` removes the default. The
/// client borrows the card; trigger and schedule invocations never attach it.
pub fn FullAgentClient::set_default_scope_card(
  self : FullAgentClient,
  card : @permissions.ScopeCard?,
) -> Unit {
  self.transport.default_scope_card = card
}

///|
/// `scope_card` lends an invocation-local scope card to the callee for this
/// call and overrides the client default. The card is borrowed, not consumed.
pub async fn[I : @schema.IntoSchema, O : @schema.FromSchema] FullAgentClient::invoke(
  self : FullAgentClient,
  method_name : String,
  input : I,
  scope_card? : @permissions.ScopeCard,
) -> Invocation[O] {
  let invocation = self.transport.invoke_value(
    method_name,
    @schema.to_value_as(input),
    scope_card,
  )
  let value = match invocation.value {
    Some(value) => decode_typed_output(value)
    None => raise ReflectError::Schema("typed method returned unit")
  }
  { metadata: invocation.metadata, value, }
}

///|
/// `scope_card` lends an invocation-local scope card to the callee for this
/// call and overrides the client default. The card is borrowed, not consumed.
pub async fn[I : @schema.IntoSchema] FullAgentClient::invoke_unit(
  self : FullAgentClient,
  method_name : String,
  input : I,
  scope_card? : @permissions.ScopeCard,
) -> Invocation[Unit] {
  let invocation = self.transport.invoke_value(
    method_name,
    @schema.to_value_as(input),
    scope_card,
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
/// Sets the scope card that awaited invocations of this client attach when the
/// call does not pass its own `scope_card`. `None` removes the default. The
/// client borrows the card; trigger and schedule invocations never attach it.
pub fn MethodOnlyAgentClient::set_default_scope_card(
  self : MethodOnlyAgentClient,
  card : @permissions.ScopeCard?,
) -> Unit {
  self.inner.set_default_scope_card(card)
}

///|
/// `scope_card` lends an invocation-local scope card to the callee for this
/// call and overrides the client default. The card is borrowed, not consumed.
pub async fn[I : @schema.IntoSchema, O : @schema.FromSchema] MethodOnlyAgentClient::invoke(
  self : MethodOnlyAgentClient,
  method_name : String,
  input : I,
  scope_card? : @permissions.ScopeCard,
) -> Invocation[O] {
  self.inner.invoke(method_name, input, scope_card?)
}

///|
/// `scope_card` lends an invocation-local scope card to the callee for this
/// call and overrides the client default. The card is borrowed, not consumed.
pub async fn[I : @schema.IntoSchema] MethodOnlyAgentClient::invoke_unit(
  self : MethodOnlyAgentClient,
  method_name : String,
  input : I,
  scope_card? : @permissions.ScopeCard,
) -> Invocation[Unit] {
  self.inner.invoke_unit(method_name, input, scope_card?)
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
