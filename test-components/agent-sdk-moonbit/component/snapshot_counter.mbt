///|
/// A counter whose state is restored from a snapshot taken after every invocation.
#derive.agent(snapshotting="every_n(1)")
struct SnapshotCounter {
  name : String
  mut value : UInt64
} derive(ToJson, @json.FromJson)

///|
fn SnapshotCounter::new(name : String) -> SnapshotCounter {
  { name, value: 0, }
}

///|
/// Adds `amount` to the counter and returns the new value.
pub fn SnapshotCounter::add(self : Self, amount : UInt64) -> UInt64 {
  self.value += amount
  self.value
}

///|
/// Returns the current value of the counter.
pub fn SnapshotCounter::get_value(self : Self) -> UInt64 {
  ignore(self.name)
  self.value
}
