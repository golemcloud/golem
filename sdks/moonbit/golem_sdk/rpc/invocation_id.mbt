///|
/// Identifies one invocation: the ID of the agent and the UUID of the
/// invocation. `Show` and `InvocationId::to_string` print
/// `<agent-id>.<uuid>`, where both parts are UUIDs.
pub struct InvocationId {
  agent_id : @schema.AgentId
  uuid : @schema.Uuid
} derive(Debug, Eq, Hash)

///|
pub extend InvocationId with Debug::{to_repr}

///|
pub extend InvocationId with Eq::{equal, not_equal}

///|
pub extend InvocationId with Hash::{hash, hash_combine}

///|
pub extend InvocationId with Show::{output}

///|
pub suberror InvocationIdParseError {
  InvocationIdParseError(String)
} derive(Debug, Eq)

///|
pub extend InvocationIdParseError with Debug::{to_repr}

///|
pub extend InvocationIdParseError with Eq::{equal, not_equal}

///|
pub fn InvocationId::new(
  agent_id : @schema.AgentId,
  uuid : @schema.Uuid,
) -> InvocationId {
  { agent_id, uuid, }
}

///|
pub fn InvocationId::agent_id(self : InvocationId) -> @schema.AgentId {
  self.agent_id
}

///|
pub fn InvocationId::uuid(self : InvocationId) -> @schema.Uuid {
  self.uuid
}

///|
pub fn InvocationId::to_string(self : InvocationId) -> String {
  "\{self.agent_id}.\{self.uuid}"
}

///|
pub impl Show for InvocationId with fn output(self, logger) {
  logger.write_string(self.to_string())
}

///|
/// The length of the hyphenated UUID text form.
let uuid_text_length : Int32 = 36

///|
/// Parses the text form `<agent-id>.<uuid>`, where both parts are hyphenated
/// UUIDs.
pub fn InvocationId::parse(
  value : String,
) -> InvocationId raise InvocationIdParseError {
  let invalid = InvocationIdParseError(
    "invalid invocation id: expected <agent-id>.<uuid>, got '\{value}'",
  )
  if value.length() != 2 * uuid_text_length + 1 ||
    value.code_unit_at(uuid_text_length).to_int() != '.' {
    raise invalid
  }
  let agent_id = @schema.AgentId::parse(value[:uuid_text_length].to_owned()) catch {
    _ => raise invalid
  }
  let uuid = @schema.Uuid::parse(value[uuid_text_length + 1:].to_owned()) catch {
    _ => raise invalid
  }
  { agent_id, uuid, }
}

///|
/// Builds the invocation ID of host invocation metadata from its agent ID and
/// the UUID of its invocation ID.
pub fn InvocationId::from_host(
  metadata : @agentHost.InvocationMetadata,
) -> InvocationId {
  {
    agent_id: @schema.AgentId::from_wit(metadata.agent_id),
    uuid: @schema.Uuid::from_wit(metadata.invocation_id.id),
  }
}

///|
/// Reads the ID of an agent invocation. An agent method invocation always
/// belongs to an agent, so another owner is a broken host and aborts.
pub fn InvocationId::from_wit(id : @types.InvocationId) -> InvocationId {
  match id.owner {
    Agent(agent_id) =>
      {
        agent_id: @schema.AgentId::from_wit(agent_id),
        uuid: @schema.Uuid::from_wit(id.id),
      }
    Tool(tool) =>
      abort(
        "the host returned an agent invocation that the tool '\{tool}' owns",
      )
  }
}

///|
/// The host form of the invocation ID.
pub fn InvocationId::to_wit(self : InvocationId) -> @types.InvocationId {
  { owner: Agent(self.agent_id.to_wit()), id: self.uuid.to_wit(), }
}
