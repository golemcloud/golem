///|
/// The target of the invocation handle tests: a counter, and a gate that holds
/// an invocation running until its promise is completed.
#derive.agent
struct HandleTarget {
  name : String
  mut count : Int64
  mut gate : @types.PromiseId?
}

///|
fn HandleTarget::new(name : String) -> HandleTarget {
  { name, count: 0, gate: None, }
}

///|
/// Adds `by` to the counter and returns the new value.
pub fn HandleTarget::add(self : Self, by : Int64) -> Int64 {
  self.count = self.count + by
  self.count
}

///|
/// The current value of the counter.
pub fn HandleTarget::value(self : Self) -> Int64 {
  self.count
}

///|
/// Creates the gate promise and returns its oplog index.
pub fn HandleTarget::create_gate(self : Self) -> UInt64 {
  ignore(self.name)
  let gate = @api.create_promise()
  self.gate = Some(gate)
  gate.oplog_idx
}

///|
/// Waits until the gate promise is completed, and returns its payload size.
pub async fn HandleTarget::wait_gate(self : Self) -> UInt64 {
  guard self.gate is Some(gate) else { abort("no gate was created") }
  @api.await_promise(gate).length().to_uint64()
}

///|
/// What `HandleCaller::invocation_handles` observed.
#derive.golem_schema
pub(all) struct HandlesReport {
  triggered_status : String
  cancel_completed : Bool
  rebuilt_value : Int64
  scheduled_status : String
  cancel_scheduled : Bool
  cancelled_status : String
  await_cancelled : String
  value : Int64
}

///|
/// What `HandleCaller::cancel_running` observed.
#derive.golem_schema
pub(all) struct CancelRunningReport {
  running_status : String
  cancel_running : String
}

///|
/// Calls `HandleTarget` through the handles that `trigger_*` and `schedule_*`
/// return.
#derive.agent
struct HandleCaller {
  name : String
}

///|
fn HandleCaller::new(name : String) -> HandleCaller {
  { name, }
}

///|
fn status_name(status : @rpc.InvocationStatus) -> String {
  match status {
    Pending => "pending"
    Scheduled => "scheduled"
    Running => "running"
    Waiting => "waiting"
    Retrying => "retrying"
    Interrupted => "interrupted"
    Completed => "completed"
    Failed => "failed"
    Cancelled => "cancelled"
    BackgroundFailed => "background-failed"
  }
}

///|
/// Names the case of a failed handle operation, so a test asserts the typed
/// error and not a message.
fn rpc_error_name(error : Error) -> String {
  match error {
    @agentHost.RpcError::Cancelled(_) => "cancelled"
    @agentHost.RpcError::CannotCancelRunning(_) => "cannot-cancel-running"
    error => "unexpected:\{error}"
  }
}

///|
/// Triggers and schedules calls on a `HandleTarget`, then reads, awaits,
/// cancels and rebuilds their handles. Every handle is dropped.
pub async fn HandleCaller::invocation_handles(
  self : Self,
  target : String,
) -> HandlesReport {
  ignore(self.name)
  HandleTargetClient::scoped_upsert(target, async fn(client) {
    let triggered = client.trigger_add(3L)
    defer triggered.drop()
    let _ = triggered.await_result()
    let triggered_status = status_name(triggered.status())
    let cancel_completed = triggered.cancel()

    let read = client.trigger_value()
    defer read.drop()
    let rebuilt = HandleTargetClient::get_value_invocation(read.id())
    defer rebuilt.drop()
    let rebuilt_value = rebuilt.await_result()

    let now = @systemClock.now()
    let scheduled = client.schedule_add(
      { seconds: now.seconds + 3600L, nanoseconds: now.nanoseconds, },
      100L,
    )
    defer scheduled.drop()
    let scheduled_status = status_name(scheduled.status())
    let cancel_scheduled = scheduled.cancel()
    let cancelled_status = status_name(scheduled.status())
    let await_cancelled = try scheduled.await_result() catch {
      error => rpc_error_name(error)
    } noraise {
      _ => "unexpected:completed"
    }

    {
      triggered_status,
      cancel_completed,
      rebuilt_value,
      scheduled_status,
      cancel_scheduled,
      cancelled_status,
      await_cancelled,
      value: client.value(),
    }
  })
}

///|
/// Triggers `HandleTarget::wait_gate`, waits until it has started, and tries to
/// cancel it. The invocation keeps running until its gate is completed.
pub async fn HandleCaller::cancel_running(
  self : Self,
  target : String,
) -> CancelRunningReport {
  ignore(self.name)
  HandleTargetClient::scoped_upsert(target, async fn(client) {
    let waiting = client.trigger_wait_gate()
    defer waiting.drop()
    let mut status = waiting.status()
    for attempt = 0
        attempt < 300 && !(status is Running || status is Waiting)
        attempt = attempt + 1 {
      @monotonicClock.wait_for(100_000_000UL)
      status = waiting.status()
    }
    let cancel_running = try waiting.cancel() catch {
      error => rpc_error_name(error)
    } noraise {
      true => "unexpected:withdrawn"
      false => "unexpected:not-pending"
    }
    { running_status: status_name(status), cancel_running, }
  })
}
