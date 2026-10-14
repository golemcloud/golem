///|
pub let handler : Ref[(async (@async-core.TaskGroup[Unit]) -> @host.Snapshot)?] = Ref(
  None,
)

///|
pub async fn save(
  background_group : @async-core.TaskGroup[Unit],
) -> @host.Snapshot {
  match handler.val {
    Some(save) => save(background_group)
    None => { payload: [], mime_type: "application/octet-stream", }
  }
}
