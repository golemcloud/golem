///|
/// Demonstrates the lifecycle of schema-native agent streams.
#derive.agent
struct StreamingAgent {
  name : String
  mut cancelled_producers : UInt
}

///|
fn StreamingAgent::new(name : String) -> StreamingAgent {
  { name, cancelled_producers: 0, }
}

///|
fn StreamingAgent::stream(
  self : Self,
  values : Array[UInt],
) -> @schema.AgentStream[UInt] {
  @schema.AgentStream::produce(async fn(writer) {
    if writer.write_all(values) is @schema.PeerDropped {
      // A rejected write is how the producer observes an early reader drop.
      self.cancelled_producers += 1
    }
  })
}

///|
/// Consumes values until the producer closes the input stream normally.
pub async fn StreamingAgent::sum(
  self : Self,
  input : @schema.AgentStream[UInt],
) -> UInt {
  defer input.drop()
  let mut total : UInt = 0
  for ;; {
    match input.read() {
      Some(value) => total += value
      None => return total
    }
  }
}

///|
/// Produces three values and then closes normally.
pub fn StreamingAgent::produce(self : Self) -> @schema.AgentStream[UInt] {
  self.stream([1, 2, 3])
}

///|
/// Combines scalar input with an input stream and an output stream.
pub fn StreamingAgent::transform(
  self : Self,
  prefix : String,
  input : @schema.AgentStream[UInt],
) -> @schema.AgentStream[String] {
  @schema.AgentStream::produce(
    async fn(writer) {
      defer input.drop()
      for ;; {
        match input.read() {
          Some(value) =>
            if writer.write_one("\{prefix}:\{value}") is @schema.PeerDropped {
              self.cancelled_producers += 1
              return
            }
          None => return
        }
      }
    },
    on_unstarted_drop=() => input.drop(),
  )
}

///|
/// Returns multiple independent sibling streams.
pub fn StreamingAgent::siblings(
  self : Self,
) -> Array[@schema.AgentStream[UInt]] {
  [self.stream([10, 20]), self.stream([30, 40])]
}

///|
/// Keeps recoverable producer failures local to individual stream items.
pub fn StreamingAgent::recoverable(
  self : Self,
) -> @schema.AgentStream[Result[UInt, String]] {
  let values : Array[Result[UInt, String]] = [
    Result::Ok(1),
    Result::Err("this item could not be produced"),
    Result::Ok(2),
  ]
  @schema.AgentStream::produce(async fn(writer) {
    if writer.write_all(values) is @schema.PeerDropped {
      self.cancelled_producers += 1
    }
  })
}

///|
/// An ordinary non-streaming method alongside the streaming methods.
pub fn StreamingAgent::status(self : Self) -> String {
  "ready (\{self.cancelled_producers} cancelled producers)"
}

///|
fn main {

}
