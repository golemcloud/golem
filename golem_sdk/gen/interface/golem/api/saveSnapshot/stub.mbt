///|
/// Saves the component's state into a user-defined snapshot.
/// Delegates to the agent's Snapshottable implementation if available.
pub fn save() -> @host.Snapshot {
  match @agents.get_agent_instance() {
    Some(instance) =>
      match instance.snapshottable {
        Some(s) => {
          let data = s.save_snapshot()
          { data: data.to_fixedarray(), mime_type: "application/octet-stream" }
        }
        None => { data: [], mime_type: "application/octet-stream" }
      }
    None => { data: [], mime_type: "application/octet-stream" }
  }
}
