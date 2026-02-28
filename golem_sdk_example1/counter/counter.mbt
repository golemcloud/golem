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
  { name, value: 0 }
}

///|
pub fn Counter::increment(self : Self) -> Unit {
  self.value += 1
}

///|
impl @agents.RawAgent for Counter with invoke(self, method_name, input) {
  match method_name {
    "increment" => {
      let result = self.increment()
      Ok(@common.DataValue::Tuple([]))
    }
    _ =>
      @agents.panic_with_message(
        "Counter::invoke",
        "Invalid method name: \{method_name}",
      )
  }
}

///|
fn main {

}
