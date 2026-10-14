///|
#derive.agent(snapshotting="every_n(10)")
struct MultipartIndex {
  name : String
  mut revision : Int
  mut index : Bytes
  restored : Bool
}

///|
priv struct Saved {
  revision : Int
} derive(ToJson, FromJson)

///|
fn MultipartIndex::new(name : String) -> MultipartIndex {
  {
    name,
    revision: 0,
    index: Bytes::from_array([ for i in 0..<256 => i.to_byte() ]),
    restored: false,
  }
}

///|
pub fn MultipartIndex::append(self : Self, byte : Int) -> Unit {
  self.revision += 1
  self.index = self.index + Bytes::from_array([byte.to_byte()])
}

///|
pub fn MultipartIndex::inspect(self : Self) -> String {
  let values = [
    for i in 0..<self.index.length() => self.index[i].to_int().to_string()
  ]
  let index = values.join(",")
  "revision-0|\{self.revision}|\{self.restored}|\{index}"
}

///|
impl @agents.MultipartSnapshottable for MultipartIndex with fn save_snapshot_parts(
  self,
) {
  Ok(
    @agents.MultipartSnapshot::from_state(Saved::{ revision: self.revision, }, {
      "index": { bytes: self.index, content_type: "application/octet-stream", },
    }),
  )
}

///|
pub async fn MultipartIndex::load_snapshot_parts(
  snapshot : @agents.MultipartSnapshot,
  context : @agents.SnapshotRestoreContext,
) -> Result[MultipartIndex, String] {
  let saved : Saved = match snapshot.decode_state() {
    Ok(saved) => saved
    Err(error) => return Err(error)
  }
  let index = match snapshot.require_part("index", "application/octet-stream") {
    Ok(index) => index
    Err(error) => return Err(error)
  }
  let name : String = context.identity(0)
  Ok({ name, revision: saved.revision, index, restored: true, })
}
