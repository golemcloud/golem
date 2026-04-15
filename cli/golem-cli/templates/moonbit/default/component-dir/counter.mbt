///|
/// Counter agent in MoonBit
#derive.agent
#derive.mount("/counters/{name}")
struct Counter {
  name : String
  mut value : UInt64
}

///|
/// Creates a new counter with the given name
fn Counter::new(name : String) -> Counter {
  { name, value: 0 }
}

///|
/// Increments the counter and returns the new value
#derive.prompt_hint("Increase the count by one")
#derive.endpoint(post="/increment")
pub fn Counter::increment(self : Self) -> UInt64 {
  self.value += 1
  self.value
}

///|
/// Returns the current value of the counter
#derive.endpoint(get="/value")
pub fn Counter::get_value(self : Self) -> UInt64 {
  self.value
}

///|
fn main {

}
