///|
/// Counter agent with custom snapshotting in MoonBit
#derive.agent(snapshotting="every_n(1)")
#derive.mount("/snapshot-counters/{name}")
struct CounterWithSnapshotAgent {
  name : String
  mut value : UInt64
}

///|
/// Creates a new counter with the given name
fn CounterWithSnapshotAgent::new(name : String) -> CounterWithSnapshotAgent {
  { name, value: 0 }
}

///|
/// Increments the counter and returns the new value
#derive.prompt_hint("Increase the count by one")
#derive.endpoint(post="/increment")
pub fn CounterWithSnapshotAgent::increment(self : Self) -> UInt64 {
  self.value += 1
  self.value
}

///|
/// Custom snapshot: serialize value as 8 big-endian bytes
pub impl @agents.Snapshottable for CounterWithSnapshotAgent with save_snapshot(self) {
  let v = self.value
  let arr : Array[Byte] = []
  for i = 7; i >= 0; i = i - 1 {
    arr.push(((v >> (i * 8)).to_int() & 0xff).to_byte())
  }
  Bytes::from_array(arr)
}

///|
/// Custom snapshot: restore value from 8 big-endian bytes
pub impl @agents.Snapshottable for CounterWithSnapshotAgent with load_snapshot(self, bytes) {
  if bytes.length() != 8 {
    return Err("Expected an 8-byte long snapshot")
  }
  let mut v : UInt64 = 0
  for i = 0; i < 8; i = i + 1 {
    v = v | (bytes[i].to_uint64() << ((7 - i) * 8))
  }
  self.value = v
  Ok(())
}

///|
fn main {

}
