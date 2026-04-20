///|
/// Counter agent in MoonBit
#derive.agent
#derive.mount("/counters/{name}")
struct CounterAgent {
  name : String
  mut value : UInt64
}

///|
/// Creates a new counter with the given name
fn CounterAgent::new(name : String) -> CounterAgent {
  { name, value: 0 }
}

///|
/// Increments the counter and returns the new value
#derive.prompt_hint("Increase the count by one")
#derive.endpoint(post="/increment")
pub fn CounterAgent::increment(self : Self) -> UInt64 {
  self.value += 1
  self.value
}

///|
/// Returns the current value of the counter
#derive.endpoint(get="/value")
pub fn CounterAgent::get_value(self : Self) -> UInt64 {
  self.value
}

///|
fn main {

}
