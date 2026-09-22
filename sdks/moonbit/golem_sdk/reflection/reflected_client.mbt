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
pub(all) struct ReflectedConfigDeclaration {
  path : Array[String]
  source : @common.AgentConfigSource
  schema : SchemaRef
} derive(Debug, Eq)

///|
pub(all) struct ReflectedConfigJsonValue {
  path : Array[String]
  value : Json
} derive(Debug, Eq)

///|
pub(all) struct ReflectedConfigValue {
  path : Array[String]
  value : @model.SchemaValue
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
  config : Array[ReflectedConfigDeclaration]
} derive(Debug, Eq)

///|
/// A reflected client and any reusable identity created before invocation.
/// Ephemeral clients have no reusable identity until invocation metadata is
/// returned by the runtime.
pub(all) struct ReflectedAgentClient {
  priv agent_type : AgentType
  priv transport : Transport
  reusable_identity : ParsedAgentId?
  phantom_id : @types.Uuid?
}

///|
fn decode_root_type(
  decoded : @model.DecodedSchemaGraph,
  root : Int,
) -> @model.SchemaType raise ReflectError {
  guard decoded.roots.get(root) is Some(selected) else {
    raise Schema("reflected schema root index \{root} is out of range")
  }
  selected
}

///|
fn decode_input(
  decoded : @model.DecodedSchemaGraph,
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
          body: decode_root_type(decoded, field.schema),
          metadata: field.metadata,
        })
      AutoInjected(_) => ()
    }
  }
  SchemaRef::new({
    defs: decoded.graph.defs,
    root: @model.schema_type(Record(fields)),
  })
}

///|
fn decode_output(
  decoded : @model.DecodedSchemaGraph,
  output : @common.OutputSchema,
) -> SchemaRef? raise ReflectError {
  match output {
    Unit => None
    Single(root) =>
      Some(
        SchemaRef::new({
          defs: decoded.graph.defs,
          root: decode_root_type(decoded, root),
        }),
      )
  }
}

///|
fn decode_agent_type(
  registered : @common.RegisteredAgentType,
) -> AgentType raise ReflectError {
  let raw = registered.agent_type
  let decoded = @model.schema_graph_roots_from_wit(raw.schema) catch {
    error => raise Schema("failed to decode reflected schema: \{repr(error)}")
  }
  let methods : Array[AgentMethod] = []
  for raw_method in raw.methods {
    methods.push({
      name: raw_method.name,
      description: raw_method.description,
      prompt_hint: raw_method.prompt_hint,
      input: decode_input(decoded, raw_method.input_schema),
      output: decode_output(decoded, raw_method.output_schema),
    })
  }
  let config : Array[ReflectedConfigDeclaration] = []
  for declaration in raw.config {
    config.push({
      path: declaration.path,
      source: declaration.source,
      schema: SchemaRef::new({
        defs: decoded.graph.defs,
        root: decode_root_type(decoded, declaration.value_type),
      }),
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
    constructor_input: decode_input(decoded, raw.constructor_.input_schema),
    methods,
    config,
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
pub fn get_agent_type_by_agent_id(
  agent_id : ParsedAgentId,
) -> AgentType? raise ReflectError {
  match @agentHost.get_agent_type_by_agent_id(agent_id.value) {
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
) -> ParsedAgentId raise ReflectError {
  self.constructor_input.validate_value(constructor_value)
  ParsedAgentId::create(self.name, constructor_value, phantom_id~)
}

///|
pub fn AgentType::agent_id_json(
  self : AgentType,
  constructor_json : Json,
  phantom_id? : @types.Uuid? = None,
) -> ParsedAgentId raise ReflectError {
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
  reusable_identity : ParsedAgentId?,
) -> ReflectedAgentClient raise ReflectError {
  self.client_value_with_config(
    constructor_value,
    phantom_id,
    reusable_identity,
    [],
  )
}

///|
fn AgentType::client_value_with_config(
  self : AgentType,
  constructor_value : @model.SchemaValue,
  phantom_id : @types.Uuid?,
  reusable_identity : ParsedAgentId?,
  config : Array[@common.TypedAgentConfigValue],
) -> ReflectedAgentClient raise ReflectError {
  self.constructor_input.validate_value(constructor_value)
  {
    agent_type: self,
    transport: Transport::create_with_config(
      self.name,
      constructor_value,
      phantom_id,
      config,
    ),
    reusable_identity,
    phantom_id,
  }
}

///|
pub fn AgentType::get_value_with_config(
  self : AgentType,
  constructor_value : @model.SchemaValue,
  config : Array[ReflectedConfigValue],
) -> ReflectedAgentClient raise ReflectError {
  guard self.mode == Durable else {
    raise Identity(
      "existing identity lookup is unavailable for ephemeral agents",
    )
  }
  let overrides = self.validate_config_values(config)
  let identity = self.agent_id_value(constructor_value)
  self.client_value_with_config(
    constructor_value,
    None,
    Some(identity),
    overrides,
  )
}

///|
pub fn AgentType::get_json_with_config(
  self : AgentType,
  constructor_json : Json,
  config : Array[ReflectedConfigJsonValue],
) -> ReflectedAgentClient raise ReflectError {
  let packed_constructor = self.constructor_input.pack_json(constructor_json)
  guard self.mode == Durable else {
    raise Identity(
      "existing identity lookup is unavailable for ephemeral agents",
    )
  }
  let overrides = self.pack_config_json(config)
  let identity = self.agent_id_value(packed_constructor)
  self.client_value_with_config(
    packed_constructor,
    None,
    Some(identity),
    overrides,
  )
}

///|
pub fn AgentType::get_phantom_value_with_config(
  self : AgentType,
  constructor_value : @model.SchemaValue,
  phantom_id : @types.Uuid,
  config : Array[ReflectedConfigValue],
) -> ReflectedAgentClient raise ReflectError {
  let overrides = self.validate_config_values(config)
  let identity = if self.mode == Durable {
    Some(self.agent_id_value(constructor_value, phantom_id=Some(phantom_id)))
  } else {
    None
  }
  self.client_value_with_config(
    constructor_value,
    Some(phantom_id),
    identity,
    overrides,
  )
}

///|
pub fn AgentType::get_phantom_json_with_config(
  self : AgentType,
  constructor_json : Json,
  phantom_id : @types.Uuid,
  config : Array[ReflectedConfigJsonValue],
) -> ReflectedAgentClient raise ReflectError {
  let overrides = self.pack_config_json(config)
  let packed_constructor = self.constructor_input.pack_json(constructor_json)
  let identity = if self.mode == Durable {
    Some(self.agent_id_value(packed_constructor, phantom_id=Some(phantom_id)))
  } else {
    None
  }
  self.client_value_with_config(
    packed_constructor,
    Some(phantom_id),
    identity,
    overrides,
  )
}

///|
pub fn AgentType::new_phantom_value_with_config(
  self : AgentType,
  constructor_value : @model.SchemaValue,
  config : Array[ReflectedConfigValue],
) -> ReflectedAgentClient raise ReflectError {
  let overrides = self.validate_config_values(config)
  if self.mode == Ephemeral {
    self.client_value_with_config(constructor_value, None, None, overrides)
  } else {
    let phantom_id = @apiHost.generate_idempotency_key()
    let identity = self.agent_id_value(
      constructor_value,
      phantom_id=Some(phantom_id),
    )
    self.client_value_with_config(
      constructor_value,
      Some(phantom_id),
      Some(identity),
      overrides,
    )
  }
}

///|
pub fn AgentType::new_phantom_json_with_config(
  self : AgentType,
  constructor_json : Json,
  config : Array[ReflectedConfigJsonValue],
) -> ReflectedAgentClient raise ReflectError {
  let overrides = self.pack_config_json(config)
  let packed_constructor = self.constructor_input.pack_json(constructor_json)
  if self.mode == Ephemeral {
    self.client_value_with_config(packed_constructor, None, None, overrides)
  } else {
    let phantom_id = @apiHost.generate_idempotency_key()
    let identity = self.agent_id_value(
      packed_constructor,
      phantom_id=Some(phantom_id),
    )
    self.client_value_with_config(
      packed_constructor,
      Some(phantom_id),
      Some(identity),
      overrides,
    )
  }
}

///|
pub fn AgentType::ephemeral_value_with_config(
  self : AgentType,
  constructor_value : @model.SchemaValue,
  config : Array[ReflectedConfigValue],
) -> ReflectedAgentClient raise ReflectError {
  guard self.mode == Ephemeral else {
    raise Identity("ephemeral invocation is unavailable for durable agents")
  }
  self.client_value_with_config(
    constructor_value,
    None,
    None,
    self.validate_config_values(config),
  )
}

///|
pub fn AgentType::ephemeral_json_with_config(
  self : AgentType,
  constructor_json : Json,
  config : Array[ReflectedConfigJsonValue],
) -> ReflectedAgentClient raise ReflectError {
  guard self.mode == Ephemeral else {
    raise Identity("ephemeral invocation is unavailable for durable agents")
  }
  self.client_value_with_config(
    self.constructor_input.pack_json(constructor_json),
    None,
    None,
    self.pack_config_json(config),
  )
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
/// Address a known ephemeral phantom for one invocation. The final identity
/// is supplied by invocation metadata and cannot be reused as a stable ID.
pub fn AgentType::ephemeral_phantom_value(
  self : AgentType,
  constructor_value : @model.SchemaValue,
  phantom_id : @types.Uuid,
) -> ReflectedAgentClient raise ReflectError {
  guard self.mode == Ephemeral else {
    raise Identity(
      "ephemeral phantom invocation is unavailable for durable agents",
    )
  }
  self.client_value(constructor_value, Some(phantom_id), None)
}

///|
pub fn AgentType::ephemeral_phantom_json(
  self : AgentType,
  constructor_json : Json,
  phantom_id : @types.Uuid,
) -> ReflectedAgentClient raise ReflectError {
  self.ephemeral_phantom_value(
    self.constructor_input.pack_json(constructor_json),
    phantom_id,
  )
}

///|
pub fn AgentType::ephemeral_phantom_value_with_config(
  self : AgentType,
  constructor_value : @model.SchemaValue,
  phantom_id : @types.Uuid,
  config : Array[ReflectedConfigValue],
) -> ReflectedAgentClient raise ReflectError {
  guard self.mode == Ephemeral else {
    raise Identity(
      "ephemeral phantom invocation is unavailable for durable agents",
    )
  }
  self.client_value_with_config(
    constructor_value,
    Some(phantom_id),
    None,
    self.validate_config_values(config),
  )
}

///|
pub fn AgentType::ephemeral_phantom_json_with_config(
  self : AgentType,
  constructor_json : Json,
  phantom_id : @types.Uuid,
  config : Array[ReflectedConfigJsonValue],
) -> ReflectedAgentClient raise ReflectError {
  guard self.mode == Ephemeral else {
    raise Identity(
      "ephemeral phantom invocation is unavailable for durable agents",
    )
  }
  self.client_value_with_config(
    self.constructor_input.pack_json(constructor_json),
    Some(phantom_id),
    None,
    self.pack_config_json(config),
  )
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
  entry.input.validate_value(input)
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
    (Some(schema), Some(value)) => {
      let json = {
        errdefer value.release_resources(@model_host.resource_drops())
        schema.unpack_json(value)
      }
      value.release_resources(@model_host.resource_drops())
      Some(json)
    }
    (None, Some(value)) => {
      value.release_resources(@model_host.resource_drops())
      raise Schema("unit method returned a value")
    }
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
  entry.input.validate_value(input)
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
  entry.input.validate_value(input)
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
    (None, None) => invocation
    (Some(schema), Some(value)) => {
      {
        errdefer value.release_resources(@model_host.resource_drops())
        schema.validate_value(value)
      }
      invocation
    }
    (None, Some(value)) => {
      value.release_resources(@model_host.resource_drops())
      raise Schema("unit method returned a value")
    }
    (Some(_), None) => raise Schema("single-output method returned no value")
  }
}

///|
pub fn ReflectedAgentClient::drop(self : ReflectedAgentClient) -> Unit {
  self.transport.drop()
}
