///|
/// Identifies one invocation: the agent ID in its text form and the UUID of
/// the invocation. `Show` and `InvocationId::to_string` print
/// `<agent-id>.<uuid>`.
pub struct InvocationId {
  agent_id : String
  uuid : @schema.Uuid
} derive(Debug, Eq, Hash)

///|
pub suberror InvocationIdParseError {
  InvocationIdParseError(String)
} derive(Debug, Eq)

///|
pub fn InvocationId::new(
  agent_id : String,
  uuid : @schema.Uuid,
) -> InvocationId {
  { agent_id, uuid, }
}

///|
pub fn InvocationId::agent_id(self : InvocationId) -> String {
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
/// The length of the `.<uuid>` suffix of the text form.
let invocation_id_suffix_length : Int = 37

///|
/// Parses the text form `<agent-id>.<uuid>`. The last 37 characters are the
/// `.` and the UUID, so an agent ID that contains `.` parses unchanged.
pub fn InvocationId::parse(
  value : String,
) -> InvocationId raise InvocationIdParseError {
  let invalid = InvocationIdParseError(
    "invalid invocation id: expected <agent-id>.<uuid>, got '\{value}'",
  )
  let suffix_start = value.length() - invocation_id_suffix_length
  if suffix_start <= 0 || value.code_unit_at(suffix_start).to_int() != '.' {
    raise invalid
  }
  let uuid = @schema.Uuid::parse(value[suffix_start + 1:].to_owned()) catch {
    _ => raise invalid
  }
  { agent_id: value[:suffix_start].to_owned(), uuid, }
}

///|
/// Builds the invocation ID of host invocation metadata from its agent ID and
/// the UUID of its invocation ID.
pub fn InvocationId::from_host(
  metadata : @agentHost.InvocationMetadata,
) -> InvocationId {
  {
    agent_id: metadata.agent_id,
    uuid: @schema.Uuid::from_wit(metadata.invocation_id.id),
  }
}
