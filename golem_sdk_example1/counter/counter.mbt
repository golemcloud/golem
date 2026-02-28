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
impl @agents.RawAgent for Counter with invoke(self, method_name, input) {
  match method_name {
    "increment" => {
      self.increment()
      Ok(@common.DataValue::Tuple([]))
    }
    "decrement" => {
      self.decrement()
      Ok(@common.DataValue::Tuple([]))
    }
    "get_value" => {
      let result = self.get_value()
      Ok(
        @common.DataValue::Tuple([
          @common.ElementValue::ComponentModel(
            { nodes: [@types.WitNode::PrimU64(result)] },
          ),
        ]),
      )
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
