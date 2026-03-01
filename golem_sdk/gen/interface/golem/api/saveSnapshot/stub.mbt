///|
/// Saves the component's state into a user-defined snapshot.
/// Delegates to the agent's Snapshottable implementation if available.
pub fn save() -> FixedArray[Byte] {
  match @agents.get_agent_instance() {
    Some(instance) =>
      match instance.snapshottable {
        Some(s) => s.save_snapshot().to_fixedarray()
        None => []
      }
    None => []
  }
}
