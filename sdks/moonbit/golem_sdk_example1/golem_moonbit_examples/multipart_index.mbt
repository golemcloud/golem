///|
/// An in-memory binary index with a separately encoded snapshot state.
#derive.agent(snapshotting="every_n(3)")
struct MultipartIndex {
  name : String
  mut revision : Int
  mut index : Bytes
}

///|
priv struct MultipartIndexState {
  revision : Int
} derive(ToJson, FromJson)

///|
fn MultipartIndex::new(name : String) -> MultipartIndex {
  { name, revision: 0, index: b"", }
}

///|
pub fn MultipartIndex::append(self : Self, bytes : Bytes) -> Int {
  self.index = self.index + bytes
  self.revision += 1
  self.revision
}

///|
pub fn MultipartIndex::read_index(self : Self) -> (Int, Bytes) {
  (self.revision, self.index)
}

///|
impl @agents.MultipartSnapshottable for MultipartIndex with fn save_snapshot_parts(
  self,
) {
  Ok(
    @agents.MultipartSnapshot::from_state(
      MultipartIndexState::{ revision: self.revision, },
      {
        "index": {
          bytes: self.index,
          content_type: "application/octet-stream",
        },
      },
    ),
  )
}

///|
pub async fn MultipartIndex::load_snapshot_parts(
  snapshot : @agents.MultipartSnapshot,
  context : @agents.SnapshotRestoreContext,
) -> Result[MultipartIndex, String] {
  let saved : MultipartIndexState = match snapshot.decode_state() {
    Ok(saved) => saved
    Err(message) => return Err(message)
  }
  let index = match snapshot.require_part("index", "application/octet-stream") {
    Ok(index) => index
    Err(message) => return Err(message)
  }
  let name : String = context.identity(0)
  Ok({ name, revision: saved.revision, index, })
}
