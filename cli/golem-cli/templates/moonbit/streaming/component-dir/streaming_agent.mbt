///|
/// Demonstrates the lifecycle of schema-native agent streams.
#derive.mount("/durable-stream-agents/{name}")
#derive.agent
struct StreamingAgent {
  name : String
  external_auth : @config.Secret[String]
  mut cancelled_producers : UInt
}

///|
#derive.config
pub(all) struct StreamingConfig {
  external_auth : @config.Secret[String]
}

///|
fn StreamingAgent::new(
  name : String,
  config : @config.Config[StreamingConfig],
) -> StreamingAgent {
  { name, external_auth: config.value.external_auth, cancelled_producers: 0, }
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
#derive.endpoint(put="/echo")
#derive.durable_stream_slot(input="input")
#derive.durable_stream_slot(output="$result")
#derive.durable_streams(allow_external_writes=true)
pub fn StreamingAgent::durable_echo(
  self : Self,
  input : @schema.AgentStream[String],
) -> @schema.AgentStream[String] {
  let _ = self
  @schema.AgentStream::produce(
    async fn(writer) {
      defer input.drop()
      for ;; {
        match input.read() {
          Some(value) =>
            if writer.write_one("echo:\{value}") is @schema.PeerDropped {
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
fn external_json_writer(
  url : String,
  producer_id : String,
  auth : @types.Secret,
) -> @durable_streams.Writer[String] raise {
  @durable_streams.json_writer(url, producer_id~, auth~)
}

///|
fn external_json_reader(
  url : String,
  auth : @types.Secret,
) -> @durable_streams.Reader[String] raise {
  @durable_streams.json_reader(url, auth~)
}

///|
pub async fn StreamingAgent::append_external(
  self : Self,
  url : String,
  producer_id : String,
  values : Array[String],
  close : Bool,
) -> String? {
  let writer = self.external_auth.with_handle(auth =>
    external_json_writer(url, producer_id, auth))
  defer writer.drop()
  writer.append(values, close~).next_offset
}

///|
pub async fn StreamingAgent::read_external(
  self : Self,
  url : String,
) -> Array[String] {
  let reader = self.external_auth.with_handle(auth =>
    external_json_reader(url, auth))
  defer reader.close()
  let values : Array[String] = []
  for ;; {
    match reader.read() {
      Some(value) => values.push(value)
      None => return values
    }
  }
}

///|
fn main {

}
