///|
/// Counter agent in MoonBit
#derive.agent(snapshotting="every_n(1)")
struct Counter {
  name : String
  mut value : UInt64
} derive(ToJson, @json.FromJson)

///|
/// Creates a new counter with the given name
fn Counter::new(name : String) -> Counter {
  { name, value: 0 }
}

///|
/// Increments the counter
pub fn Counter::increment(self : Self) -> Unit {
  self.value += 1
}

///|
/// Decrements the counter
pub fn Counter::decrement(self : Self) -> Unit {
  self.value -= 1
}

///|
/// Returns the current value of the counter
pub fn Counter::get_value(self : Self) -> UInt64 {
  self.value
}

///|
fn main {

}
