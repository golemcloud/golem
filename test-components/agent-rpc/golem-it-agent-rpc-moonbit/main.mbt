///|
#derive.golem_schema
pub(all) struct NestedStreamInput {
  labels : @schema.AgentStream[String]
  values : @schema.AgentStream[UInt]
}

///|
#derive.golem_schema
pub(all) struct NestedStreamItem {
  label : String
  values : @schema.AgentStream[UInt]
}

///|
#derive.golem_schema
pub(all) struct MoonbitStreamingRpcReport {
  consumed : Array[UInt]
  produced : Array[UInt]
  transformed : Array[UInt]
  scalar_label : String
  scalar_values : Array[UInt]
  nested_labels : Array[String]
  nested_values : Array[UInt]
  nested_item_labels : Array[String]
  nested_item_values : Array[Array[UInt]]
  first_sibling : Array[UInt]
  second_sibling : Array[UInt]
  forwarded : Array[UInt]
  recoverable : Array[Result[UInt, String]]
  early_value : UInt
  ping : String
  increment : UInt64
}

///|
fn uint_range(start : Int, count : Int) -> Array[UInt] {
  let values : Array[UInt] = []
  for i in 0..<count {
    values.push((start + i).reinterpret_as_uint())
  }
  values
}

///|
fn uint_stream(values : Array[UInt]) -> @schema.AgentStream[UInt] {
  @schema.AgentStream::produce(async fn(writer) {
    let _ = writer.write_all(values)
  })
}

///|
async fn collect_uint_stream(stream : @schema.AgentStream[UInt]) -> Array[UInt] {
  defer stream.drop()
  let values : Array[UInt] = []
  for ;; {
    match stream.read() {
      Some(value) => values.push(value)
      None => return values
    }
  }
}

///|
async fn collect_string_stream(
  stream : @schema.AgentStream[String],
) -> Array[String] {
  defer stream.drop()
  let values : Array[String] = []
  for ;; {
    match stream.read() {
      Some(value) => values.push(value)
      None => return values
    }
  }
}

///|
async fn collect_result_stream(
  stream : @schema.AgentStream[Result[UInt, String]],
) -> Array[Result[UInt, String]] {
  defer stream.drop()
  let values : Array[Result[UInt, String]] = []
  for ;; {
    match stream.read() {
      Some(value) => values.push(value)
      None => return values
    }
  }
}

///|
#derive.agent("ephemeral")
struct MoonbitStreamingRpcTarget {
  name : String
  mut count : UInt64
}

///|
fn MoonbitStreamingRpcTarget::new(name : String) -> MoonbitStreamingRpcTarget {
  { name, count: 0, }
}

///|
pub async fn MoonbitStreamingRpcTarget::consume(
  self : Self,
  input : @schema.AgentStream[UInt],
) -> Array[UInt] {
  let _ = self
  collect_uint_stream(input)
}

///|
pub fn MoonbitStreamingRpcTarget::produce(
  self : Self,
) -> @schema.AgentStream[UInt] {
  let _ = self
  uint_stream([3U, 5U, 8U])
}

///|
pub fn MoonbitStreamingRpcTarget::transform(
  self : Self,
  input : @schema.AgentStream[UInt],
) -> @schema.AgentStream[UInt] {
  let _ = self
  @schema.AgentStream::produce(
    async fn(writer) {
      defer input.drop()
      for ;; {
        match input.read() {
          Some(value) =>
            if writer.write_one(value + 100U) is @schema.PeerDropped {
              return
            }
          None => return
        }
      }
    },
    on_unstarted_drop=fn() { input.drop() },
  )
}

///|
pub fn MoonbitStreamingRpcTarget::scalar_and_stream(
  self : Self,
) -> (String, @schema.AgentStream[UInt]) {
  let _ = self
  ("moonbit", uint_stream([13U, 21U, 34U]))
}

///|
pub async fn MoonbitStreamingRpcTarget::consume_nested(
  self : Self,
  input : NestedStreamInput,
) -> (Array[String], Array[UInt]) {
  let _ = self
  let { labels, values, } = input
  @asyncCore.with_task_group(async fn(group) {
    let labels_task = group.spawn(async fn() { collect_string_stream(labels) })
    let values_task = group.spawn(async fn() { collect_uint_stream(values) })
    (labels_task.wait(), values_task.wait())
  })
}

///|
pub fn MoonbitStreamingRpcTarget::produce_nested_items(
  self : Self,
) -> @schema.AgentStream[NestedStreamItem] {
  let _ = self
  @schema.AgentStream::produce(async fn(writer) {
    for i in 0..<3 {
      let item = {
        label: "item-\{i}",
        values: uint_stream(uint_range(i * 10, 4)),
      }
      if writer.write_one(item) is @schema.PeerDropped {
        return
      }
    }
  })
}

///|
pub fn MoonbitStreamingRpcTarget::produce_siblings(
  self : Self,
) -> (@schema.AgentStream[UInt], @schema.AgentStream[UInt]) {
  let _ = self
  (uint_stream(uint_range(0, 192)), uint_stream(uint_range(1000, 192)))
}

///|
pub fn MoonbitStreamingRpcTarget::forward_unread(
  self : Self,
  input : @schema.AgentStream[UInt],
) -> @schema.AgentStream[UInt] {
  let _ = self
  input
}

///|
pub fn MoonbitStreamingRpcTarget::produce_recoverable_results(
  self : Self,
) -> @schema.AgentStream[Result[UInt, String]] {
  let _ = self
  @schema.AgentStream::produce(async fn(writer) {
    let _ = writer.write_all([Ok(1U), Err("recoverable"), Ok(2U)])
  })
}

///|
pub fn MoonbitStreamingRpcTarget::produce_failure(
  self : Self,
) -> @schema.AgentStream[UInt] {
  let _ = self
  @schema.AgentStream::produce(async fn(writer) {
    let _ = writer.write_one(41U)
    abort("moonbit stream producer failure")
  })
}

///|
pub fn MoonbitStreamingRpcTarget::produce_cancellable(
  self : Self,
) -> @schema.AgentStream[UInt] {
  let _ = self
  @schema.AgentStream::produce(async fn(writer) {
    if writer.write_one(43U) is @schema.PeerDropped {
      return
    }
    for ;; {
      if writer.write_all(uint_range(44, 192)) is @schema.PeerDropped {
        return
      }
    }
  })
}

///|
pub fn MoonbitStreamingRpcTarget::produce_many(
  self : Self,
) -> @schema.AgentStream[UInt] {
  let _ = self
  uint_stream(uint_range(0, 192))
}

///|
pub fn MoonbitStreamingRpcTarget::ping(self : Self) -> String {
  "pong:\{self.name}"
}

///|
pub fn MoonbitStreamingRpcTarget::increment(self : Self) -> UInt64 {
  self.count = self.count + 1
  self.count
}

///|
#derive.agent
struct MoonbitStreamingRpcCrossTarget {
  name : String
  mut count : UInt64
}

///|
fn MoonbitStreamingRpcCrossTarget::new(
  name : String,
) -> MoonbitStreamingRpcCrossTarget {
  { name, count: 0, }
}

///|
pub async fn MoonbitStreamingRpcCrossTarget::consume(
  self : Self,
  input : @schema.AgentStream[UInt],
) -> Array[UInt] {
  let _ = self
  collect_uint_stream(input)
}

///|
pub async fn MoonbitStreamingRpcCrossTarget::consume_one_then_drop(
  self : Self,
  input : @schema.AgentStream[UInt],
) -> UInt {
  let _ = self
  defer input.drop()
  input.read().unwrap()
}

///|
pub fn MoonbitStreamingRpcCrossTarget::produce(
  self : Self,
) -> @schema.AgentStream[UInt] {
  let _ = self
  uint_stream([3U, 5U, 8U])
}

///|
pub fn MoonbitStreamingRpcCrossTarget::transform(
  self : Self,
  input : @schema.AgentStream[UInt],
) -> @schema.AgentStream[UInt] {
  let _ = self
  @schema.AgentStream::produce(
    async fn(writer) {
      defer input.drop()
      for ;; {
        match input.read() {
          Some(value) =>
            if writer.write_one(value + 100U) is @schema.PeerDropped {
              return
            }
          None => return
        }
      }
    },
    on_unstarted_drop=fn() { input.drop() },
  )
}

///|
pub fn MoonbitStreamingRpcCrossTarget::scalar_and_stream(
  self : Self,
) -> (String, @schema.AgentStream[UInt]) {
  let _ = self
  ("moonbit", uint_stream([13U, 21U, 34U]))
}

///|
pub async fn MoonbitStreamingRpcCrossTarget::consume_nested(
  self : Self,
  input : NestedStreamInput,
) -> (Array[String], Array[UInt]) {
  let _ = self
  let { labels, values, } = input
  @asyncCore.with_task_group(async fn(group) {
    let labels_task = group.spawn(async fn() { collect_string_stream(labels) })
    let values_task = group.spawn(async fn() { collect_uint_stream(values) })
    (labels_task.wait(), values_task.wait())
  })
}

///|
pub fn MoonbitStreamingRpcCrossTarget::produce_nested_items(
  self : Self,
) -> @schema.AgentStream[NestedStreamItem] {
  let _ = self
  @schema.AgentStream::produce(async fn(writer) {
    for i in 0..<3 {
      let item = {
        label: "item-\{i}",
        values: uint_stream(uint_range(i * 10, 4)),
      }
      if writer.write_one(item) is @schema.PeerDropped {
        return
      }
    }
  })
}

///|
pub fn MoonbitStreamingRpcCrossTarget::produce_siblings(
  self : Self,
) -> (@schema.AgentStream[UInt], @schema.AgentStream[UInt]) {
  let _ = self
  (uint_stream(uint_range(0, 192)), uint_stream(uint_range(1000, 192)))
}

///|
pub fn MoonbitStreamingRpcCrossTarget::forward_unread(
  self : Self,
  input : @schema.AgentStream[UInt],
) -> @schema.AgentStream[UInt] {
  let _ = self
  input
}

///|
pub fn MoonbitStreamingRpcCrossTarget::produce_recoverable_results(
  self : Self,
) -> @schema.AgentStream[Result[UInt, String]] {
  let _ = self
  @schema.AgentStream::produce(async fn(writer) {
    let _ = writer.write_all([Ok(1U), Err("recoverable"), Ok(2U)])
  })
}

///|
pub fn MoonbitStreamingRpcCrossTarget::produce_failure(
  self : Self,
) -> @schema.AgentStream[UInt] {
  let _ = self
  @schema.AgentStream::produce(async fn(writer) {
    let _ = writer.write_one(41U)
    abort("moonbit stream producer failure")
  })
}

///|
pub fn MoonbitStreamingRpcCrossTarget::produce_cancellable(
  self : Self,
) -> @schema.AgentStream[UInt] {
  let _ = self
  @schema.AgentStream::produce(async fn(writer) {
    if writer.write_one(43U) is @schema.PeerDropped {
      return
    }
    for ;; {
      if writer.write_all(uint_range(44, 192)) is @schema.PeerDropped {
        return
      }
    }
  })
}

///|
pub fn MoonbitStreamingRpcCrossTarget::produce_many(
  self : Self,
) -> @schema.AgentStream[UInt] {
  let _ = self
  uint_stream(uint_range(0, 192))
}

///|
pub fn MoonbitStreamingRpcCrossTarget::ping(self : Self) -> String {
  "pong:\{self.name}"
}

///|
pub fn MoonbitStreamingRpcCrossTarget::increment(self : Self) -> UInt64 {
  self.count = self.count + 1
  self.count
}

///|
#derive.agent
struct MoonbitStreamingRpcCaller {
  target_name : String
}

///|
fn MoonbitStreamingRpcCaller::new(
  target_name : String,
) -> MoonbitStreamingRpcCaller {
  { target_name, }
}

///|
pub async fn MoonbitStreamingRpcCaller::run(
  self : Self,
) -> MoonbitStreamingRpcReport {
  MoonbitStreamingRpcCrossTargetClient::scoped(self.target_name, async fn(
    target,
  ) {
    let consumed = target.consume(uint_stream([1U, 2U, 3U]))
    let produced = collect_uint_stream(target.produce())
    let transformed = collect_uint_stream(
      target.transform(uint_stream([4U, 5U, 6U])),
    )
    let (scalar_label, scalar_stream) = target.scalar_and_stream()
    let scalar_values = collect_uint_stream(scalar_stream)
    let (nested_labels, nested_values) = target.consume_nested({
      labels: @schema.AgentStream::produce(async fn(writer) {
        let _ = writer.write_all(["alpha", "beta", "gamma"])
      }),
      values: uint_stream([7U, 8U, 9U]),
    })
    let nested_items = target.produce_nested_items()
    defer nested_items.drop()
    let nested_item_labels : Array[String] = []
    let nested_item_values : Array[Array[UInt]] = []
    for ;; {
      match nested_items.read() {
        Some(item) => {
          nested_item_labels.push(item.label)
          nested_item_values.push(collect_uint_stream(item.values))
        }
        None => break
      }
    }
    let (first_stream, second_stream) = target.produce_siblings()
    let (first_sibling, second_sibling) = @asyncCore.with_task_group(async fn(
      group,
    ) {
      let first = group.spawn(async fn() { collect_uint_stream(first_stream) })
      let second = group.spawn(async fn() { collect_uint_stream(second_stream) })
      (first.wait(), second.wait())
    })
    let forwarded = collect_uint_stream(
      target.forward_unread(uint_stream(uint_range(2000, 192))),
    )
    let recoverable = collect_result_stream(
      target.produce_recoverable_results(),
    )
    let early_stream = target.produce_many()
    let early_value = early_stream.read().unwrap()
    early_stream.drop()
    let ping = target.ping()
    let increment = target.increment()
    {
      consumed,
      produced,
      transformed,
      scalar_label,
      scalar_values,
      nested_labels,
      nested_values,
      nested_item_labels,
      nested_item_values,
      first_sibling,
      second_sibling,
      forwarded,
      recoverable,
      early_value,
      ping,
      increment,
    }
  })
}

///|
pub async fn MoonbitStreamingRpcCaller::call_producer_failure(
  self : Self,
) -> Array[UInt] {
  MoonbitStreamingRpcCrossTargetClient::scoped(self.target_name, async fn(
    target,
  ) {
    collect_uint_stream(target.produce_failure())
  })
}

///|
pub async fn MoonbitStreamingRpcCaller::call_producer_cancellation(
  self : Self,
) -> (UInt, String) {
  MoonbitStreamingRpcCrossTargetClient::scoped(self.target_name, async fn(
    target,
  ) {
    let stream = target.produce_cancellable()
    let first = stream.read().unwrap()
    stream.drop()
    (first, target.ping())
  })
}

///|
pub async fn MoonbitStreamingRpcCaller::call_server_cancellation(
  self : Self,
) -> (UInt, Bool, String) {
  MoonbitStreamingRpcCrossTargetClient::scoped(self.target_name, async fn(
    target,
  ) {
    let (producer_outcome, producer_completion) = @asyncCore.Future::new()
    let input = @schema.AgentStream::produce(async fn(writer) {
      for ;; {
        if writer.write_all(uint_range(50, 192)) is @schema.PeerDropped {
          ignore(producer_completion.complete(true))
          return
        }
      }
    })
    let first = target.consume_one_then_drop(input)
    (first, producer_outcome.get(), target.ping())
  })
}

///|
pub async fn MoonbitStreamingRpcCaller::drop_output_early(
  self : Self,
) -> (UInt, String) {
  MoonbitStreamingRpcCrossTargetClient::scoped(self.target_name, async fn(
    target,
  ) {
    let stream = target.produce_many()
    let first = stream.read().unwrap()
    stream.drop()
    (first, target.ping())
  })
}

///|
pub async fn MoonbitStreamingRpcCaller::call_scalar(self : Self) -> UInt64 {
  MoonbitStreamingRpcCrossTargetClient::scoped(self.target_name, async fn(
    target,
  ) {
    target.increment()
  })
}

///|
fn main {

}
