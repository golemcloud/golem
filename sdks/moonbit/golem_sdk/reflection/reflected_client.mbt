///|
/// A method discovered from the runtime-owned agent schema.
pub(all) struct AgentMethod {
  name : String
  description : String
  prompt_hint : String?
  input : SchemaRef
  output : SchemaRef?
} derive(Debug, Eq)

///|
/// An agent type discovered from the runtime registry.
pub(all) struct AgentType {
  name : String
  description : String
  source_language : String
  mode : AgentMode
  implemented_by : @types.ComponentId
  constructor_input : SchemaRef
  methods : Array[AgentMethod]
} derive(Debug, Eq)

///|
/// A reflected client and any reusable identity created before invocation.
/// Ephemeral clients have no reusable identity until invocation metadata is
/// returned by the runtime.
pub(all) struct ReflectedAgentClient {
  priv agent_type : AgentType
  priv transport : Transport
  reusable_identity : AgentId?
  phantom_id : @types.Uuid?
}

///|
fn decode_root_type(
  graph : @types.SchemaGraph,
  root : Int,
) -> @model.SchemaType raise ReflectError {
  let decoded = @model.schema_graph_from_wit({ ..graph, root, }) catch {
    error => raise Schema("failed to decode reflected schema: \{repr(error)}")
  }
  decoded.root
}

///|
fn decode_input(
  graph : @types.SchemaGraph,
  decoded : @model.SchemaGraph,
  input : @common.InputSchema,
) -> SchemaRef raise ReflectError {
  let raw_fields = match input {
    Parameters(fields) => fields
  }
  let fields : Array[@model.NamedFieldType] = []
  for field in raw_fields {
    match field.source {
      UserSupplied =>
        fields.push({
          name: field.name,
          body: decode_root_type(graph, field.schema),
          metadata: field.metadata,
        })
      AutoInjected(_) => ()
    }
  }
  SchemaRef::new({
    defs: decoded.defs,
    root: @model.schema_type(Record(fields)),
  })
}

///|
fn decode_output(
  graph : @types.SchemaGraph,
  decoded : @model.SchemaGraph,
  output : @common.OutputSchema,
) -> SchemaRef? raise ReflectError {
  match output {
    Unit => None
    Single(root) =>
      Some(
        SchemaRef::new({
          defs: decoded.defs,
          root: decode_root_type(graph, root),
        }),
      )
  }
}

///|
fn decode_agent_type(
  registered : @common.RegisteredAgentType,
) -> AgentType raise ReflectError {
  let raw = registered.agent_type
  let decoded = @model.schema_graph_from_wit(raw.schema) catch {
    error => raise Schema("failed to decode reflected schema: \{repr(error)}")
  }
  let methods : Array[AgentMethod] = []
  for raw_method in raw.methods {
    methods.push({
      name: raw_method.name,
      description: raw_method.description,
      prompt_hint: raw_method.prompt_hint,
      input: decode_input(raw.schema, decoded, raw_method.input_schema),
      output: decode_output(raw.schema, decoded, raw_method.output_schema),
    })
  }
  {
    name: raw.type_name,
    description: raw.description,
    source_language: raw.source_language,
    mode: match raw.mode {
      DURABLE => Durable
      EPHEMERAL => Ephemeral
    },
    implemented_by: registered.implemented_by,
    constructor_input: decode_input(
      raw.schema,
      decoded,
      raw.constructor_.input_schema,
    ),
    methods,
  }
}

///|
pub fn get_all_agent_types() -> Array[AgentType] raise ReflectError {
  @agentHost.get_all_agent_types().map(decode_agent_type)
}

///|
pub fn get_agent_type(name : String) -> AgentType? raise ReflectError {
  match @agentHost.get_agent_type(name) {
    Some(agent_type) => Some(decode_agent_type(agent_type))
    None => None
  }
}

///|
/// Resolves the component implementing an environment-unique agent type name.
/// Unlike `get_agent_type`, absence is an error because identity metadata
/// cannot be constructed without an implementing component.
pub fn component_id_for(name : String) -> @types.ComponentId raise ReflectError {
  match @agentHost.get_agent_type(name) {
    Some(agent_type) => agent_type.implemented_by
    None =>
      raise Discovery(
        "agent type '" + name + "' is not registered in the current environment",
      )
  }
}

///|
pub fn get_agent_type_by_agent_id(
  agent_id : AgentId,
) -> AgentType? raise ReflectError {
  match @agentHost.get_agent_type_by_agent_id(agent_id.to_wit()) {
    Some(agent_type) => Some(decode_agent_type(agent_type))
    None => None
  }
}

///|
pub fn AgentType::find_method(self : AgentType, name : String) -> AgentMethod? {
  self.methods.iter().find_first(entry => entry.name == name)
}

///|
pub fn AgentType::agent_id_value(
  self : AgentType,
  constructor_value : @model.SchemaValue,
  phantom_id? : @types.Uuid? = None,
) -> AgentId raise ReflectError {
  AgentId::create(
    self.implemented_by,
    self.name,
    constructor_value,
    phantom_id~,
  )
}

///|
pub fn AgentType::agent_id_json(
  self : AgentType,
  constructor_json : Json,
  phantom_id? : @types.Uuid? = None,
) -> AgentId raise ReflectError {
  self.agent_id_value(
    self.constructor_input.pack_json(constructor_json),
    phantom_id~,
  )
}

///|
fn AgentType::client_value(
  self : AgentType,
  constructor_value : @model.SchemaValue,
  phantom_id : @types.Uuid?,
  reusable_identity : AgentId?,
) -> ReflectedAgentClient raise ReflectError {
  {
    agent_type: self,
    transport: Transport::create(
      self.implemented_by,
      self.name,
      constructor_value,
      phantom_id,
    ),
    reusable_identity,
    phantom_id,
  }
}

///|
pub fn AgentType::get_value(
  self : AgentType,
  constructor_value : @model.SchemaValue,
) -> ReflectedAgentClient raise ReflectError {
  guard self.mode == Durable else {
    raise Identity(
      "existing identity lookup is unavailable for ephemeral agents",
    )
  }
  let identity = self.agent_id_value(constructor_value)
  self.client_value(constructor_value, None, Some(identity))
}

///|
pub fn AgentType::get_json(
  self : AgentType,
  constructor_json : Json,
) -> ReflectedAgentClient raise ReflectError {
  self.get_value(self.constructor_input.pack_json(constructor_json))
}

///|
pub fn AgentType::resume_phantom_value(
  self : AgentType,
  constructor_value : @model.SchemaValue,
  phantom_id : @types.Uuid,
) -> ReflectedAgentClient raise ReflectError {
  guard self.mode == Durable else {
    raise Identity("phantom identities are unavailable for ephemeral agents")
  }
  let identity = self.agent_id_value(
    constructor_value,
    phantom_id=Some(phantom_id),
  )
  self.client_value(constructor_value, Some(phantom_id), Some(identity))
}

///|
pub fn AgentType::resume_phantom_json(
  self : AgentType,
  constructor_json : Json,
  phantom_id : @types.Uuid,
) -> ReflectedAgentClient raise ReflectError {
  self.resume_phantom_value(
    self.constructor_input.pack_json(constructor_json),
    phantom_id,
  )
}

///|
pub fn AgentType::new_phantom_value(
  self : AgentType,
  constructor_value : @model.SchemaValue,
) -> ReflectedAgentClient raise ReflectError {
  guard self.mode == Durable else {
    raise Identity("phantom identities are unavailable for ephemeral agents")
  }
  let phantom_id = @apiHost.generate_idempotency_key()
  self.resume_phantom_value(constructor_value, phantom_id)
}

///|
pub fn AgentType::new_phantom_json(
  self : AgentType,
  constructor_json : Json,
) -> ReflectedAgentClient raise ReflectError {
  self.new_phantom_value(self.constructor_input.pack_json(constructor_json))
}

///|
pub fn AgentType::ephemeral_value(
  self : AgentType,
  constructor_value : @model.SchemaValue,
) -> ReflectedAgentClient raise ReflectError {
  guard self.mode == Ephemeral else {
    raise Identity("ephemeral invocation is unavailable for durable agents")
  }
  self.client_value(constructor_value, None, None)
}

///|
pub fn AgentType::ephemeral_json(
  self : AgentType,
  constructor_json : Json,
) -> ReflectedAgentClient raise ReflectError {
  self.ephemeral_value(self.constructor_input.pack_json(constructor_json))
}

///|
fn ReflectedAgentClient::require_method(
  self : ReflectedAgentClient,
  name : String,
) -> AgentMethod raise ReflectError {
  match self.agent_type.find_method(name) {
    Some(entry) => entry
    None =>
      raise Discovery(
        "agent type '" + self.agent_type.name + "' has no method '" + name + "'",
      )
  }
}

///|
pub async fn ReflectedAgentClient::invoke_value(
  self : ReflectedAgentClient,
  method_name : String,
  input : @model.SchemaValue,
) -> Invocation[@model.SchemaValue?] {
  let entry = self.require_method(method_name)
  let invocation = self.transport.invoke_value(method_name, input)
  validate_invocation_output(entry, invocation)
}

///|
pub async fn ReflectedAgentClient::invoke_json(
  self : ReflectedAgentClient,
  method_name : String,
  input : Json,
) -> Invocation[Json?] {
  let entry = self.require_method(method_name)
  let invocation = self.transport.invoke_value(
    method_name,
    entry.input.pack_json(input),
  )
  let value = match (entry.output, invocation.value) {
    (None, None) => None
    (Some(schema), Some(value)) => Some(schema.unpack_json(value))
    (None, Some(_)) => raise Schema("unit method returned a value")
    (Some(_), None) => raise Schema("value-returning method returned unit")
  }
  { metadata: invocation.metadata, value, }
}

///|
pub fn ReflectedAgentClient::trigger_value(
  self : ReflectedAgentClient,
  method_name : String,
  input : @model.SchemaValue,
) -> InvocationMetadata raise ReflectError {
  let entry = self.require_method(method_name)
  reject_non_awaited_streams(entry, "trigger")
  self.transport.trigger_value(method_name, input)
}

///|
pub fn ReflectedAgentClient::trigger_json(
  self : ReflectedAgentClient,
  method_name : String,
  input : Json,
) -> InvocationMetadata raise ReflectError {
  let entry = self.require_method(method_name)
  reject_non_awaited_streams(entry, "trigger")
  self.transport.trigger_value(method_name, entry.input.pack_json(input))
}

///|
pub fn ReflectedAgentClient::schedule_value(
  self : ReflectedAgentClient,
  scheduled_at : @systemClock.Instant,
  method_name : String,
  input : @model.SchemaValue,
) -> ScheduledInvocation raise ReflectError {
  let entry = self.require_method(method_name)
  reject_non_awaited_streams(entry, "schedule")
  self.transport.schedule_value(scheduled_at, method_name, input)
}

///|
pub fn ReflectedAgentClient::schedule_json(
  self : ReflectedAgentClient,
  scheduled_at : @systemClock.Instant,
  method_name : String,
  input : Json,
) -> ScheduledInvocation raise ReflectError {
  let entry = self.require_method(method_name)
  reject_non_awaited_streams(entry, "schedule")
  self.transport.schedule_value(
    scheduled_at,
    method_name,
    entry.input.pack_json(input),
  )
}

///|
fn reject_non_awaited_streams(
  entry : AgentMethod,
  operation : String,
) -> Unit raise ReflectError {
  let output_contains_stream = match entry.output {
    Some(output) => output.contains_stream()
    None => false
  }
  guard !entry.input.contains_stream() && !output_contains_stream else {
    raise Schema(
      operation + " is unavailable for streaming method '" + entry.name + "'",
    )
  }
}

///|
fn validate_invocation_output(
  entry : AgentMethod,
  invocation : Invocation[@model.SchemaValue?],
) -> Invocation[@model.SchemaValue?] raise ReflectError {
  match (entry.output, invocation.value) {
    (None, None) | (Some(_), Some(_)) => invocation
    (None, Some(_)) => raise Schema("unit method returned a value")
    (Some(_), None) => raise Schema("single-output method returned no value")
  }
}

///|
pub fn ReflectedAgentClient::drop(self : ReflectedAgentClient) -> Unit {
  self.transport.drop()
}
