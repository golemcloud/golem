///|
pub let handler : Ref[
  (async (@host.Snapshot, @types.SchemaValueTree, @async-core.TaskGroup[Unit]) -> Result[
    Unit,
    String,
  ])?,
] = Ref(None)

///|
pub async fn load(
  snapshot : @host.Snapshot,
  constructor_ : @types.SchemaValueTree,
  background_group : @async-core.TaskGroup[Unit],
) -> Result[Unit, String] {
  match handler.val {
    Some(load) => load(snapshot, constructor_, background_group)
    None => {
      @model.drop_wit_capabilities(constructor_)
      Err("Component defines no agents")
    }
  }
}
