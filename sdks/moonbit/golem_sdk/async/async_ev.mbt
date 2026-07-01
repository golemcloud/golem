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
      subscribes : Map[Int, Subscriber]
      tasks : Map[WaitableSet, Coroutine]
      finished : Map[WaitableSet, Bool]
}

///|
priv struct Subscriber {
      mut event : Events?
      coro : Coroutine
}

///|
fn current_waitableset() -> WaitableSet {
      WaitableSet(tls_get())
}

///|
pub fn with_waitableset(f : async () -> Unit) -> Int {
      let waitable_set = WaitableSet::new()
      tls_set(waitable_set.0)
      let coro = spawn(async fn() -> Unit {
            defer ev.finished.set(waitable_set, true)
            f()
      })
      ev.tasks.set(waitable_set, coro)
      ev.finished.set(waitable_set, false)
      reschedule()
      if ev.finished.get(waitable_set) is Some(true) {
            ev.tasks.remove(waitable_set)
            ev.finished.remove(waitable_set)
            waitable_set.drop()
            return CallbackCode::Completed.encode()
      } else {
            return CallbackCode::Wait(waitable_set.0).encode()
      }
}

///|
pub fn cb(event : Int, waitable_id : Int, code : Int) -> Int {
      let waitable_set = current_waitableset()
      let events = Events::new(EventCode::from(event), waitable_id, code)
      match events {
            TaskCancelled => {
                  guard ev.tasks.get(waitable_set) is Some(coro)
                  coro.cancel()
                  reschedule()
                  if ev.finished.get(waitable_set) is Some(true) {
                        ev.tasks.remove(waitable_set)
                        ev.finished.remove(waitable_set)
                        waitable_set.drop()
                        task_cancel()
                        return CallbackCode::Completed.encode()
                  } else {
                        // Unlikely to reach here
                        return CallbackCode::Wait(waitable_set.0).encode()
                  }
            }
            _ => {
                  let sub = ev.subscribes.get(waitable_id)
                  guard sub is Some(subscriber)
                  subscriber.event = Some(events)
                  subscriber.coro.wake()
                  reschedule()
                  if ev.finished.get(waitable_set) is Some(true) {
                        ev.tasks.remove(waitable_set)
                        ev.finished.remove(waitable_set)
                        waitable_set.drop()
                        return CallbackCode::Completed.encode()
                  } else {
                        return CallbackCode::Wait(waitable_set.0).encode()
                  }
            }
      }
}

///|
let ev : EventLoop = { subscribes: {}, tasks: {}, finished: {} }

///|
pub async fn suspend_for_subtask(
val : Int,
cleanup_after_started : () -> Unit,
) -> Unit {
      let task = SubTask::from(val)
      defer subtask_drop(task.handle)
      let mut cleaned = false

      // Helper: ensure cleanup is called once we've moved past Starting state
      fn ensure_cleanup(state : SubTaskState) -> Unit {
            if not(cleaned) && !(state is Starting) {
                  cleanup_after_started()
                  cleaned = true
            }
      }

      // Initial state, return if finished
      ensure_cleanup(task.state)
      match task.state {
            Returned => return
            Cancelled_before_started => raise SubTaskCancelled(before_started=true)
            Cancelled_before_returned => raise SubTaskCancelled(before_started=false)
            _ => ()
      }

      // Create subscriber to wait for events
      let subscriber = { event: None, coro: current_coroutine() }
      ev.subscribes.set(task.handle, subscriber)
      defer ev.subscribes.remove(task.handle)
      waitable_join(task.handle, current_waitableset().0)
      defer waitable_join(task.handle, 0)
      for {
            suspend() catch {
                  Cancelled::Cancelled =>
                  // Cancel the subtask
                  return protect_from_cancel(() => {
                        subscriber.event = task
                        .cancel()
                        .map(state => Subtask(task.handle, state))
                        while subscriber.event is None {
                              suspend()
                        }
                        guard subscriber.event is Some(Subtask(i, state)) && i == task.handle
                        ensure_cleanup(state)
                        match state {
                              Returned => return
                              Cancelled_before_started =>
                              raise SubTaskCancelled(before_started=true)
                              Cancelled_before_returned =>
                              raise SubTaskCancelled(before_started=false)
                              _ => panic() // should not happen
                        }
                  })
            }

            // Subsequent state, return if finished
            if subscriber.event is Some(Subtask(i, state)) {
                  guard i == task.handle
                  ensure_cleanup(state)
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
pub async fn suspend_for_future_read(idx : Int, val : Int) -> Unit {
      let result = if val == -1 {
            let subscriber = { event: None, coro: current_coroutine() }
            ev.subscribes.set(idx, subscriber)
            defer ev.subscribes.remove(idx)
            waitable_join(idx, current_waitableset().0)
            defer waitable_join(idx, 0)
            suspend()
            guard subscriber.event is Some(FutureRead(i, result)) && i == idx
            result
      } else {
            FutureReadResult::from(val)
      }
      match result {
            Completed => return
            Cancelled => raise FutureReadCancelled
      }
}

///|
pub async fn suspend_for_future_write(idx : Int) -> Bool {
      let subscriber = { event: None, coro: current_coroutine() }
      ev.subscribes.set(idx, subscriber)
      defer ev.subscribes.remove(idx)
      waitable_join(idx, current_waitableset().0)
      defer waitable_join(idx, 0)
      suspend()
      guard subscriber.event is Some(FutureWrite(i, result)) && i == idx
      match result {
            Completed => true
            Dropped => false
            Cancelled => raise FutureWriteCancelled
      }
}

///|
pub async fn suspend_for_stream_read(idx : Int, val : Int) -> (Int, Bool) {
      let { progress, copy_result } = if val == -1 {
            // Blocked, wait for event
            let subscriber = { event: None, coro: current_coroutine() }
            ev.subscribes.set(idx, subscriber)
            defer ev.subscribes.remove(idx)
            waitable_join(idx, current_waitableset().0)
            defer waitable_join(idx, 0)
            suspend()
            guard subscriber.event is Some(StreamRead(i, result)) && i == idx
            result
      } else {
            StreamResult::from(val)
      }
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
pub async fn suspend_for_stream_write(idx : Int, val : Int) -> (Int, Bool) {
      let { progress, copy_result } = if val != -1 {
            // Not blocked
            StreamResult::from(val)
      } else {
            // Blocked, wait for event
            let subscriber = { event: None, coro: current_coroutine() }
            ev.subscribes.set(idx, subscriber)
            defer ev.subscribes.remove(idx)
            waitable_join(idx, current_waitableset().0)
            defer waitable_join(idx, 0)
            suspend()
            guard subscriber.event is Some(StreamWrite(i, result)) && i == idx
            result
      }
      match copy_result {
            Completed => return (progress, false)
            Dropped => return (progress, true)
            Cancelled =>
            if progress > 0 {
                  return (progress, false)
            } else {
                  raise StreamWriteCancelled
            }
      }
}

///|
pub suberror OpCancelled {
      SubTaskCancelled(before_started~ : Bool)
      StreamWriteCancelled
      StreamReadCancelled
      FutureWriteCancelled
      FutureReadCancelled
}
