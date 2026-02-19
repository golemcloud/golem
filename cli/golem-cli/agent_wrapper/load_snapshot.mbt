/// Tries to load a user-defined snapshot, setting up the agent's state based on it.
/// The function can return with a failure to indicate that the update is not possible.
pub fn load(snapshot : @host.Snapshot) -> Result[Unit, String] {
      @loadSnapshot.load(snapshot)
}
