///|
/// Installed by generated registration only when the component defines agents.
pub(all) struct Handlers {
  initialize : async (
    String,
    @types.SchemaValueTree,
    @common.Principal,
    @async-core.TaskGroup[Unit],
  ) -> Result[Unit, @common.AgentError]
  invoke : async (
    String,
    @types.SchemaValueTree,
    @common.Principal,
    @async-core.TaskGroup[Unit],
  ) -> Result[@types.SchemaValueTree?, @common.AgentError]
  get_definition : () -> @common.AgentType
  discover_agent_types : () -> Result[
    Array[@common.AgentType],
    @common.AgentError,
  ]
}

///|
pub let handlers : Ref[Handlers?] = Ref(None)

///|
pub async fn initialize(
  agent_type : String,
  input : @types.SchemaValueTree,
  principal : @common.Principal,
  background_group : @async-core.TaskGroup[Unit],
) -> Result[Unit, @common.AgentError] {
  match handlers.val {
    Some(h) => (h.initialize)(agent_type, input, principal, background_group)
    None => {
      @model.drop_wit_capabilities(input)
      Err(@common.AgentError::InvalidType(agent_type))
    }
  }
}

///|
pub async fn invoke(
  method_name : String,
  input : @types.SchemaValueTree,
  principal : @common.Principal,
  background_group : @async-core.TaskGroup[Unit],
) -> Result[@types.SchemaValueTree?, @common.AgentError] {
  match handlers.val {
    Some(h) => (h.invoke)(method_name, input, principal, background_group)
    None => {
      @model.drop_wit_capabilities(input)
      Err(@common.AgentError::InvalidMethod(method_name))
    }
  }
}

///|
pub fn get_definition() -> @common.AgentType {
  match handlers.val {
    Some(h) => (h.get_definition)()
    None => abort("Agent instance is not initialized")
  }
}

///|
pub fn discover_agent_types() -> Result[
  Array[@common.AgentType],
  @common.AgentError,
] {
  match handlers.val {
    Some(h) => (h.discover_agent_types)()
    None => Ok([])
  }
}
