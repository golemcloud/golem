// Copyright 2025 International Digital Economy Academy
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

///|
priv struct EventLoop {
  subscribes : Map[WaitableSet, Map[Int, Subscriber]]
  wakeups : Map[WaitableSet, TaskWakeup]
  tasks : Map[WaitableSet, Coroutine]
  owned_coroutines : Map[WaitableSet, @set.Set[Coroutine]]
  finished : Map[WaitableSet, Bool]
  resolved : Map[WaitableSet, Bool]
  cancellations : Map[WaitableSet, TaskCancellation]
  post_return_flush : Map[WaitableSet, Bool]
}

///|
priv struct TaskWakeup {
  reader : Int
  writer : Int
  mut reading : Bool
  mut signaled : Bool
}

///|
priv struct Subscriber {
  mut event : Events?
  coro : @set.Set[Coroutine]
}

///|
priv enum TaskCancellation {
  Requested
  Acknowledged
}

///|
fn current_waitableset() -> WaitableSet {
  WaitableSet(tls_get())
}

///|
fn subscribers_for(waitable_set : WaitableSet) -> Map[Int, Subscriber] {
  match ev.subscribes.get(waitable_set) {
    Some(subscribers) => subscribers
    None => {
      let subscribers = Map([])
      ev.subscribes.set(waitable_set, subscribers)
      subscribers
    }
  }
}

///|
fn clear_subscribers(waitable_set : WaitableSet) -> Unit {
  if ev.subscribes.get(waitable_set) is Some(subscribers) {
    subscribers.each(fn(waitable_id, _subscriber) {
      waitable_join(waitable_id, 0)
    })
    ev.subscribes.remove(waitable_set)
  }
}

///|
fn finish_waitableset(waitable_set : WaitableSet) -> Unit {
  ev.tasks.remove(waitable_set)
  ev.owned_coroutines.remove(waitable_set)
  ev.finished.remove(waitable_set)
  ev.resolved.remove(waitable_set)
  ev.cancellations.remove(waitable_set)
  ev.post_return_flush.remove(waitable_set)
  drop_task_wakeup(waitable_set)
  clear_subscribers(waitable_set)
  forget_schedule(waitable_set)
  waitable_set.drop()
  tls_set(0)
}

///|
fn acknowledge_cancellation(waitable_set : WaitableSet) -> Unit {
  guard ev.cancellations.get(waitable_set) is Some(Requested) else { return }
  guard ev.finished.get(waitable_set) is Some(true) else { return }
  guard ev.tasks.get(waitable_set) is Some(coro) else { return }
  match coro.state {
    Done => () // The generated wrapper already called task.return.
    Fail(_) =>
      if !(ev.resolved.get(waitable_set) is Some(true)) {
        task_cancel()
        ev.resolved.set(waitable_set, true)
      }
    Running | Suspend(_) => return
  }
  ev.cancellations.set(waitable_set, Acknowledged)
}

///|
fn notify_subscriber(
  waitable_set : WaitableSet,
  waitable_id : Int,
  event : Events,
) -> Unit {
  if ev.wakeups.get(waitable_set) is Some(wakeup) &&
    wakeup.reader == waitable_id {
    guard event is StreamRead(i, { progress: 1, copy_result: Completed }) &&
      i == waitable_id
    wakeup.reading = false
    wakeup.signaled = false
    remove_subscriber(waitable_set, waitable_id)
    waitable_join(waitable_id, 0)
    return
  }
  guard ev.subscribes.get(waitable_set) is Some(subscribers)
  guard subscribers.get(waitable_id) is Some(subscriber)
  subscriber.event = Some(event)
  subscriber.coro.each(Coroutine::wake)
  // Keep the delivered event discoverable until its waiter consumes it. A
  // local cancellation can race with the wakeup, and cancelling an already
  // terminal canonical operation would trap.
  waitable_join(waitable_id, 0)
}

///|
fn next_callback(waitable_set : WaitableSet) -> Int {
  if ev.post_return_flush.get(waitable_set) is Some(true) {
    ev.post_return_flush.remove(waitable_set)
    reschedule(waitable_set)
  }
  acknowledge_cancellation(waitable_set)
  if ev.finished.get(waitable_set) is Some(true) && no_more_work(waitable_set) {
    let resolved = ev.resolved.get(waitable_set) is Some(true)
    let failed = match ev.tasks.get(waitable_set) {
      Some(coro) => coro.state is Fail(_)
      None => false
    }
    finish_waitableset(waitable_set)
    if !resolved {
      if failed {
        abort("async export failed before task return")
      } else {
        abort("async export completed without task return")
      }
    }
    return CallbackCode::Completed.encode()
  }
  if has_immediately_ready_task(waitable_set) {
    if ev.subscribes.get(waitable_set) is Some(subscribers) &&
      !subscribers.is_empty() {
      let (event, waitable_id) = waitable_set.poll()
      match event {
        None => ()
        TaskCancelled => panic()
        _ => notify_subscriber(waitable_set, waitable_id, event)
      }
    }
    tls_set(waitable_set.0)
    return CallbackCode::Yield.encode()
  }
  tls_set(waitable_set.0)
  arm_task_wakeup(waitable_set)
  CallbackCode::Wait(waitable_set.0).encode()
}

///|
#internal(wit_bindgen, "generated binding code only")
#doc(hidden)
pub fn with_waitableset(f : async () -> Unit) -> Int {
  let waitable_set = WaitableSet::new()
  let owned = @set.Set([])
  tls_set(waitable_set.0)
  let coro = spawn_owned(
    async fn() -> Unit {
      let coro = current_coroutine()
      defer owned.remove(coro)
      defer ev.finished.set(waitable_set, true)
      f()
    },
    waitable_set,
    None,
  )
  ev.tasks.set(waitable_set, coro)
  owned.add(coro)
  ev.owned_coroutines.set(waitable_set, owned)
  ev.finished.set(waitable_set, false)
  ev.resolved.set(waitable_set, false)
  reschedule(waitable_set)
  next_callback(waitable_set)
}

///|
/// Record that the generated export wrapper has resolved the component-model
/// task. One additional fair scheduling round lets the enclosing MoonBit task
/// group observe its body completing without draining runnable background work.
#internal(wit_bindgen, "generated binding code only")
#doc(hidden)
pub fn task_returned() -> Unit {
  let waitable_set = current_waitableset()
  guard ev.tasks.get(waitable_set) is Some(_)
  ev.post_return_flush.set(waitable_set, true)
  ev.resolved.set(waitable_set, true)
}

///|
#internal(wit_bindgen, "generated binding code only")
#doc(hidden)
pub fn cb(event : Int, waitable_id : Int, code : Int) -> Int {
  let waitable_set = current_waitableset()
  let events = Events::new(EventCode::from(event), waitable_id, code)
  let preserve_task_wakeup = match events {
    StreamRead(i, _) =>
      match ev.wakeups.get(waitable_set) {
        Some(wakeup) => wakeup.reader == i
        None => false
      }
    _ => false
  }
  cancel_task_wakeup_read(waitable_set, preserve=preserve_task_wakeup)
  match events {
    None => {
      reschedule(waitable_set)
      next_callback(waitable_set)
    }
    TaskCancelled => {
      guard ev.tasks.get(waitable_set) is Some(coro)
      ev.cancellations.set(waitable_set, Requested)
      coro.cancel()
      if ev.owned_coroutines.get(waitable_set) is Some(owned) {
        owned.each(Coroutine::cancel)
      }
      reschedule(waitable_set)
      next_callback(waitable_set)
    }
    _ => {
      notify_subscriber(waitable_set, waitable_id, events)
      reschedule(waitable_set)
      next_callback(waitable_set)
    }
  }
}

///|
let ev : EventLoop = {
  subscribes: Map([]),
  wakeups: Map([]),
  tasks: Map([]),
  owned_coroutines: Map([]),
  finished: Map([]),
  resolved: Map([]),
  cancellations: Map([]),
  post_return_flush: Map([]),
}

///|
/// Spawn a coroutine owned by the current component-model async task.
///
/// This is intended for runtime bridge work such as lowering local futures and
/// streams to component-model handles. The coroutine does not participate in
/// `TaskGroup` structured concurrency, but it is cancelled when the current
/// component-model task is cancelled and keeps the waitable-set alive until it
/// terminates.
#internal(wit_bindgen, "generated binding code only")
#doc(hidden)
pub fn spawn_component_task_current(f : async () -> Unit) -> Unit {
  let waitable_set = current_waitableset()
  guard ev.tasks.get(waitable_set) is Some(_)
  guard ev.owned_coroutines.get(waitable_set) is Some(owned)
  let coro = spawn_owned(
    async fn() -> Unit {
      let coro = current_coroutine()
      defer owned.remove(coro)
      f()
    },
    waitable_set,
    Some(spawn_component_task_current),
  )
  owned.add(coro)
}

///|
#internal(wit_bindgen, "generated binding code only")
#doc(hidden)
pub fn has_component_task_scope() -> Bool {
  let waitable_set = current_waitableset()
  ev.tasks.get(waitable_set) is Some(_) &&
  ev.owned_coroutines.get(waitable_set) is Some(_)
}

///|
#internal(wit_bindgen, "generated binding code only")
#doc(hidden)
pub fn current_component_task_token() -> Int {
  current_waitableset().0
}

///|
fn detach_waitable(waitable_id : Int) -> Unit {
  let waitable_set = current_waitableset()
  if ev.subscribes.get(waitable_set) is Some(subscribers) {
    if subscribers.get(waitable_id) is Some(subscriber) {
      subscriber.coro.clear()
      subscribers.remove(waitable_id)
    }
    if subscribers.is_empty() {
      ev.subscribes.remove(waitable_set)
    }
  }
  waitable_join(waitable_id, 0)
}

///|
fn remove_subscriber(waitable_set : WaitableSet, waitable_id : Int) -> Unit {
  if ev.subscribes.get(waitable_set) is Some(subscribers) {
    subscribers.remove(waitable_id)
    if subscribers.is_empty() {
      ev.subscribes.remove(waitable_set)
    }
  }
}

///|
fn task_wakeup(waitable_set : WaitableSet) -> TaskWakeup {
  match ev.wakeups.get(waitable_set) {
    Some(wakeup) => wakeup
    None => {
      let pair = task_wakeup_stream_new()
      let wakeup = {
        reader: pair.to_int(),
        writer: (pair >> 32).to_int(),
        reading: false,
        signaled: false,
      }
      ev.wakeups.set(waitable_set, wakeup)
      wakeup
    }
  }
}

///|
fn arm_task_wakeup(waitable_set : WaitableSet) -> Unit {
  let wakeup = task_wakeup(waitable_set)
  if wakeup.reading {
    return
  }
  let result = task_wakeup_stream_read(wakeup.reader, 0, 1)
  guard result == -1
  let subscribers = subscribers_for(waitable_set)
  guard subscribers.get(wakeup.reader) is None
  subscribers.set(wakeup.reader, { event: None, coro: Set([]) })
  waitable_join(wakeup.reader, waitable_set.0)
  wakeup.reading = true
  wakeup.signaled = false
}

///|
fn signal_component_task(waitable_set : WaitableSet) -> Unit {
  guard ev.wakeups.get(waitable_set) is Some(wakeup) else { return }
  if !wakeup.reading || wakeup.signaled {
    return
  }
  let result = StreamResult::from(task_wakeup_stream_write(wakeup.writer, 0, 1))
  guard result.progress == 1 && result.copy_result is Completed
  wakeup.signaled = true
}

///|
fn cancel_task_wakeup_read(
  waitable_set : WaitableSet,
  preserve~ : Bool,
) -> Unit {
  guard ev.wakeups.get(waitable_set) is Some(wakeup) else { return }
  if !wakeup.reading || preserve {
    return
  }
  waitable_join(wakeup.reader, 0)
  remove_subscriber(waitable_set, wakeup.reader)
  ignore(task_wakeup_stream_cancel_read(wakeup.reader))
  wakeup.reading = false
  wakeup.signaled = false
}

///|
fn drop_task_wakeup(waitable_set : WaitableSet) -> Unit {
  guard ev.wakeups.get(waitable_set) is Some(wakeup) else { return }
  if wakeup.reading {
    waitable_join(wakeup.reader, 0)
    remove_subscriber(waitable_set, wakeup.reader)
    ignore(task_wakeup_stream_cancel_read(wakeup.reader))
  }
  task_wakeup_stream_drop_readable(wakeup.reader)
  task_wakeup_stream_drop_writable(wakeup.writer)
  ev.wakeups.remove(waitable_set)
}

///|
fn cancel_waitable_event(
  owner_task : Int,
  waitable_id : Int,
  cancel : () -> Events,
) -> Events {
  let waitable_set = WaitableSet(owner_task)
  guard ev.subscribes.get(waitable_set) is Some(subscribers)
  guard subscribers.get(waitable_id) is Some(subscriber)

  // Baseline cancel-read is synchronous and traps while its endpoint belongs
  // to a waitable set. Detach before invoking the generated intrinsic.
  waitable_join(waitable_id, 0)
  let event = match subscriber.event {
    Some(event) => event
    None => cancel()
  }
  subscriber.event = Some(event)
  subscriber.coro.each(Coroutine::wake)
  remove_subscriber(waitable_set, waitable_id)
  event
}

///|
/// Complete an operation immediately or suspend until its terminal waitable
/// event arrives. This is the single owner of subscription registration and
/// detachment for future and stream copy operations.
async fn waitable_event(waitable_id : Int, immediate : Events?) -> Events {
  let waitable_set = current_waitableset()
  match immediate {
    Some(event) => {
      if ev.subscribes.get(waitable_set) is Some(subscribers) {
        if subscribers.get(waitable_id) is Some(subscriber) {
          subscriber.event = Some(event)
          subscriber.coro.each(Coroutine::wake)
          subscriber.coro.clear()
          waitable_join(waitable_id, 0)
        }
        remove_subscriber(waitable_set, waitable_id)
      }
      event
    }
    None => {
      let subscribers = subscribers_for(waitable_set)
      let subscriber = if subscribers.get(waitable_id) is Some(subscriber) {
        subscriber
      } else {
        let subscriber = { event: None, coro: Set([]) }
        subscribers.set(waitable_id, subscriber)
        subscriber
      }
      guard subscriber.event is None
      waitable_join(waitable_id, waitable_set.0)
      let coro = current_coroutine()
      subscriber.coro.add(coro)
      defer subscriber.coro.remove(coro)
      let event = try suspend() catch {
        err =>
          match subscriber.event {
            // Completion owns the canonical buffer once its event is
            // delivered, even if local cancellation wakes the same coroutine.
            Some(event) => event
            None => raise err
          }
      } noraise {
        _ => subscriber.event.unwrap()
      }
      remove_subscriber(waitable_set, waitable_id)
      event
    }
  }
}

///|
#internal(wit_bindgen, "generated binding code only")
#doc(hidden)
pub async fn suspend_for_subtask(
  val : Int,
  settle_lowered_arguments : (Bool) -> Unit,
  drop_returned_result : () -> Unit,
) -> Unit {
  let task = SubTask::from(val)
  let mut cleaned = false

  // Once the subtask leaves Starting, canonical arguments are either
  // transferred or rejected. Generated code owns the corresponding action.
  fn settle_arguments(state : SubTaskState) -> Unit {
    if !cleaned && !(state is Starting) {
      cleaned = true
      settle_lowered_arguments(state is Cancelled_before_started)
    }
  }

  // Immediate completion without a handle.
  if task.handle == 0 {
    settle_arguments(task.state)
    match task.state {
      Returned => return
      Cancelled_before_started => raise SubTaskCancelled(before_started=true)
      Cancelled_before_returned => raise SubTaskCancelled(before_started=false)
      _ => panic()
    }
  }

  defer subtask_drop(task.handle)

  // Initial state, return if finished
  settle_arguments(task.state)
  match task.state {
    Returned => return
    Cancelled_before_started => raise SubTaskCancelled(before_started=true)
    Cancelled_before_returned => raise SubTaskCancelled(before_started=false)
    _ => ()
  }

  // A subtask can report intermediate states, so register the same subscriber
  // again after each callback until a terminal state arrives.
  let waitable_set = current_waitableset()
  let set = @set.Set([])
  let subscriber = { event: None, coro: set }
  let coro = current_coroutine()
  set.add(coro)
  defer {
    set.remove(coro)
    detach_waitable(task.handle)
  }
  for ;; {
    let subscribers = subscribers_for(waitable_set)
    guard subscribers.get(task.handle) is None
    subscribers.set(task.handle, subscriber)
    waitable_join(task.handle, waitable_set.0)
    suspend() catch {
      Cancelled::Cancelled =>
        // Cancel the subtask
        return protect_from_cancel(() => {
          detach_waitable(task.handle)
          // A terminal event can race with local cancellation after waking
          // this coroutine. Once delivered, cancelling the subtask again
          // traps, so consume that event before requesting cancellation.
          if subscriber.event is Some(Subtask(i, state)) {
            guard i == task.handle
            settle_arguments(state)
            match state {
              Returned => {
                drop_returned_result()
                return
              }
              Cancelled_before_started | Cancelled_before_returned =>
                raise Cancelled::Cancelled
              Starting | Started => subscriber.event = None
            }
          }
          let state = task.cancel()
          settle_arguments(state)
          match state {
            Returned => {
              drop_returned_result()
              return
            }
            Cancelled_before_started | Cancelled_before_returned =>
              raise Cancelled::Cancelled
            Starting | Started => panic()
          }
        })
      err => raise err
    }

    remove_subscriber(waitable_set, task.handle)

    // Subsequent state, return if finished
    if subscriber.event is Some(Subtask(i, state)) {
      guard i == task.handle
      settle_arguments(state)
      match state {
        Returned => return
        Cancelled_before_started => raise SubTaskCancelled(before_started=true)
        Cancelled_before_returned =>
          raise SubTaskCancelled(before_started=false)
        _ => subscriber.event = None
      }
    }
  }
}

///|
#internal(wit_bindgen, "generated binding code only")
#doc(hidden)
pub async fn suspend_for_future_read(idx : Int, val : Int) -> Unit {
  let event = if val == -1 {
    waitable_event(idx, None)
  } else {
    let result = FutureReadResult::from(val)
    waitable_event(idx, Some(FutureRead(idx, result)))
  }
  guard event is FutureRead(i, result) && i == idx
  match result {
    Completed => return
    Cancelled => raise FutureReadError::Cancelled
  }
}

///|
#internal(wit_bindgen, "generated binding code only")
#doc(hidden)
pub fn cancel_future_read(
  owner_task : Int,
  idx : Int,
  cancel : () -> Int,
) -> Bool {
  let event = cancel_waitable_event(owner_task, idx, () => {
    FutureRead(idx, FutureReadResult::from(cancel()))
  })
  guard event is FutureRead(i, result) && i == idx
  match result {
    Completed => true
    Cancelled => false
  }
}

///|
#internal(wit_bindgen, "generated binding code only")
#doc(hidden)
pub fn cancel_stream_read(
  owner_task : Int,
  idx : Int,
  cancel : () -> Int,
) -> Int {
  let event = cancel_waitable_event(owner_task, idx, () => {
    StreamRead(idx, StreamResult::from(cancel()))
  })
  guard event is StreamRead(i, result) && i == idx
  result.progress
}

///|
#internal(wit_bindgen, "generated binding code only")
#doc(hidden)
pub fn cancel_stream_write(idx : Int, cancel : () -> Int) -> Int {
  let event = cancel_waitable_event(current_component_task_token(), idx, () => {
    StreamWrite(idx, StreamResult::from(cancel()))
  })
  guard event is StreamWrite(i, result) && i == idx
  result.progress
}

///|
#internal(wit_bindgen, "generated binding code only")
#doc(hidden)
pub async fn suspend_for_future_write_terminal(idx : Int, val : Int) -> Bool? {
  let event = if val == -1 {
    waitable_event(idx, None)
  } else {
    let result = FutureWriteResult::from(val)
    waitable_event(idx, Some(FutureWrite(idx, result)))
  }
  guard event is FutureWrite(i, result) && i == idx
  match result {
    Completed => Some(true)
    Dropped => Some(false)
    Cancelled => None
  }
}

///|
#internal(wit_bindgen, "generated binding code only")
#doc(hidden)
pub async fn suspend_for_stream_read(idx : Int, val : Int) -> (Int, Bool) {
  let event = if val == -1 {
    waitable_event(idx, None)
  } else {
    let result = StreamResult::from(val)
    waitable_event(idx, Some(StreamRead(idx, result)))
  }
  guard event is StreamRead(i, { progress, copy_result }) && i == idx
  match copy_result {
    Completed => return (progress, false)
    Dropped => return (progress, true)
    Cancelled =>
      if progress > 0 {
        return (progress, false)
      } else {
        raise StreamReadCancelled
      }
  }
}

///|
#internal(wit_bindgen, "generated binding code only")
#doc(hidden)
pub async fn suspend_for_stream_write(idx : Int, val : Int) -> (Int, Bool) {
  let event = if val == -1 {
    waitable_event(idx, None)
  } else {
    let result = StreamResult::from(val)
    waitable_event(idx, Some(StreamWrite(idx, result)))
  }
  guard event is StreamWrite(i, { progress, copy_result }) && i == idx
  match copy_result {
    Completed => return (progress, false)
    Dropped => return (progress, true)
    Cancelled => return (progress, false)
  }
}

///|
pub suberror OpCancelled {
  SubTaskCancelled(before_started~ : Bool)
  StreamReadCancelled
}

///|
pub(all) suberror FutureReadError {
  Cancelled
  Dropped
}

///|
#internal(wit_bindgen, "generated binding code only")
pub(all) suberror EndpointBusy {
  Read
  Write
}
