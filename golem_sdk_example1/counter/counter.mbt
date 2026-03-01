///|
let logger : @logging.Logger = @logging.with_name("counter")

///|
/// Counter agent in MoonBit
#derive.agent
struct Counter {
  name : String
  mut value : UInt64
}

///|
/// Creates a new counter with the given name
fn Counter::new(name : String) -> Counter {
  logger.info("Creating counter: " + name)
  { name, value: 0 }
}

///|
/// Increments the counter
pub fn Counter::increment(self : Self) -> Unit {
  logger.debug("Incrementing counter: " + self.name)
  self.value += 1
}

///|
/// Decrements the counter
pub fn Counter::decrement(self : Self) -> Unit {
  logger.debug("Decrementing counter: " + self.name)
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
