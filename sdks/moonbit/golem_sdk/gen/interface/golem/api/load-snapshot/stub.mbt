///|
pub let handler : Ref[
  (async (@host.Snapshot, @async-core.TaskGroup[Unit]) -> Result[Unit, String])?,
] = Ref(None)

///|
pub async fn load(
  snapshot : @host.Snapshot,
  background_group : @async-core.TaskGroup[Unit],
) -> Result[Unit, String] {
  match handler.val {
    Some(load) => load(snapshot, background_group)
    None => Err("Component defines no agents")
  }
}
