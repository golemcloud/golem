///|
/// An agent that tracks actions with principal-based access control
#derive.agent
struct AuditLog {
  name : String
  created_by : String
  entries : Array[String]
}

///|
/// Creates a new audit log, recording who created it
fn AuditLog::new(name : String, principal : @common.Principal) -> AuditLog {
  let creator = principal_to_string(principal)
  { name, created_by: creator, entries: [] }
}

///|
/// Adds an entry to the audit log, recording who performed the action
pub fn AuditLog::add_entry(
  self : Self,
  message : String,
  principal : @common.Principal,
) -> Unit {
  let who = principal_to_string(principal)
  self.entries.push("[\{who}] \{message}")
}

///|
/// Returns who created this audit log
pub fn AuditLog::get_creator(self : Self) -> String {
  self.created_by
}

///|
/// Returns all audit log entries
pub fn AuditLog::get_entries(self : Self) -> Array[String] {
  self.entries
}

///|
/// Converts a Principal to a human-readable string
fn principal_to_string(p : @common.Principal) -> String {
  match p {
    Oidc(oidc) =>
      match oidc.preferred_username {
        Some(name) => name
        None => oidc.sub
      }
    Agent(_) => "agent"
    GolemUser(_) => "golem-user"
    Anonymous => "anonymous"
  }
}
