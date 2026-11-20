///|
/// An agent that tracks actions with caller-based access control
#derive.agent
#warnings("-unused_field")
struct AuditLog {
  name : String
  created_by : String
  entries : Array[String]
}

///|
/// Creates a new audit log, recording who created it
fn AuditLog::new(name : String, ctx : @agents.InvocationContext) -> AuditLog {
  let creator = ctx.caller().original.to_string()
  { name, created_by: creator, entries: [], }
}

///|
/// Adds an entry to the audit log, recording who performed the action
pub fn AuditLog::add_entry(
  self : Self,
  message : String,
  ctx : @agents.InvocationContext,
) -> Unit {
  let who = ctx.caller().original.to_string()
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
