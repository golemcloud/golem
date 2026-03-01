///|
/// Tries to load a user-defined snapshot, setting up the agent's state based on it.
/// The function can return with a failure to indicate that the update is not possible.
///
/// When the agent is already initialized (runtime save/load), delegates to the
/// agent's `Snapshottable` implementation. When the agent is not yet initialized
/// (snapshot-based update on a fresh component instance), this currently returns
/// an error — full support requires WASI environment variable bindings to read
/// `GOLEM_AGENT_ID` and reconstruct the agent.
pub fn load(bytes : FixedArray[Byte]) -> Result[Unit, String] {
  if bytes.length() == 0 {
    return Ok(())
  }
  let user_bytes = Bytes::from_array(bytes[:])
  match @agents.get_agent_instance() {
    Some(instance) =>
      match instance.snapshottable {
        Some(s) => s.load_snapshot(user_bytes)
        None => Err("Agent does not support snapshots")
      }
    // TODO: For snapshot-based updates, Golem calls load() on a fresh instance
    // without calling initialize() first. To support this, we need to read
    // GOLEM_AGENT_ID from the environment, parse the agent type and constructor
    // params, construct the agent, then apply the snapshot.
    None =>
      Err(
        "Agent instance is not initialized; snapshot-based updates not yet supported",
      )
  }
}
