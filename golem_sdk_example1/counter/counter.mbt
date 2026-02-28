///|
struct Counter {
  name : String
  mut value : UInt64
}

///|
fn Counter::new_raw(
  input : @common.DataValue,
) -> Result[&@agents.RawAgent, @common.AgentError] {
  Ok({ name: "TODO", value: 0 })
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
fn init {
  @agents.register_agent({
    name: "Counter",
    agent_type: {
      type_name: "Counter",
      description: "Counter agent in MoonBit",
      constructor_: {
        name: None,
        description: "Constructs the counter",
        prompt_hint: None,
        input_schema: @common.DataSchema::Tuple([
          (
            "name",
            @common.ElementSchema::ComponentModel({
              nodes: [
                {
                  name: None,
                  owner: None,
                  type_: @types.WitTypeNode::PrimStringType,
                },
              ],
            }),
          ),
        ]),
      },
      methods: [],
      dependencies: [],
    },
    construct: Counter::new_raw,
  })
}

///|
fn main {

}
