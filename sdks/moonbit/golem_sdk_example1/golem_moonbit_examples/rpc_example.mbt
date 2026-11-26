///|
/// Demonstrates agent-to-agent RPC using generated client stubs
#derive.agent
struct RpcExampleAgent {
  mut last_value : UInt64
}

///|
/// Creates a new RPC example agent
fn RpcExampleAgent::new() -> RpcExampleAgent {
  { last_value: 0, }
}

///|
/// Calls a remote Counter agent, increments three times, reads value, then
/// triggers a decrement and drops its handle without waiting for it
pub async fn RpcExampleAgent::counter_rpc_example(self : Self) -> UInt64 {
  CounterClient::scoped_upsert("my-counter", async fn(counter) {
    counter.increment()
    counter.increment()
    counter.increment()
    let value = counter.get_value()
    self.last_value = value
    counter.trigger_decrement().drop()
    value
  })
}

///|
/// Calls a remote TaskManager agent using custom types like TaskInfo and Priority
pub async fn RpcExampleAgent::task_rpc_example(self : Self) -> UInt64 {
  TaskManagerClient::scoped_upsert(async fn(tm) {
    let count = tm.add_task({
      title: "Build RPC support",
      priority: High,
      description: Some("Implement agent-to-agent communication"),
    })
    self.last_value = count
    let high_tasks = tm.get_by_priority(High)
    let _ = high_tasks
    count
  })
}

///|
/// Returns the last value seen by any RPC call
pub fn RpcExampleAgent::get_last_value(self : Self) -> UInt64 {
  self.last_value
}
