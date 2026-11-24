///|
/// The ID of a Golem agent: an opaque UUID that Golem issues. Its text form,
/// printed by `Show` and `AgentId::to_string`, is the UUID.
pub struct AgentId {
  priv uuid : Uuid
} derive(Eq, Compare, Hash)

///|
pub extend AgentId with Eq::{equal, not_equal}

///|
pub extend AgentId with Compare::{compare, op_lt, op_le, op_gt, op_ge}

///|
pub extend AgentId with Hash::{hash, hash_combine}

///|
pub extend AgentId with Show::{to_string, output}

///|
pub extend AgentId with Debug::{to_repr}

///|
pub fn AgentId::from_uuid(uuid : Uuid) -> AgentId {
  { uuid, }
}

///|
pub fn AgentId::uuid(self : AgentId) -> Uuid {
  self.uuid
}

///|
/// Parses the text form, which is the hyphenated UUID.
pub fn AgentId::parse(value : String) -> AgentId raise UuidParseError {
  { uuid: Uuid::parse(value), }
}

///|
pub fn AgentId::from_wit(agent_id : @types.AgentId) -> AgentId {
  { uuid: Uuid::from_wit(agent_id.uuid), }
}

///|
pub fn AgentId::to_wit(self : AgentId) -> @types.AgentId {
  { uuid: self.uuid.to_wit(), }
}

///|
pub impl Show for AgentId with fn output(self, logger) {
  logger.write_string(self.uuid.to_string())
}

///|
pub impl Debug for AgentId with fn to_repr(self) {
  Repr::ctor("AgentId", [(None, Repr::string(self.uuid.to_string()))])
}
