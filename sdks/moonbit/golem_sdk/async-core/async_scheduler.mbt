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
priv struct Scheduler {
  mut coro_id : Int
  mut curr_coro : Coroutine?
  tasks : Map[WaitableSet, TaskSchedule]
}

///|
priv struct TaskSchedule {
  mut blocking : Int
  run_later : @deque.Deque[Coroutine]
}

///|
let scheduler : Scheduler = { coro_id: 0, curr_coro: None, tasks: Map([]) }

///|
fn current_coroutine() -> Coroutine {
  scheduler.curr_coro.unwrap()
}

///|
fn task_schedule(waitable_set : WaitableSet) -> TaskSchedule {
  match scheduler.tasks.get(waitable_set) {
    Some(schedule) => schedule
    None => {
      let schedule = { blocking: 0, run_later: Deque([]) }
      scheduler.tasks.set(waitable_set, schedule)
      schedule
    }
  }
}

///|
fn enqueue(coro : Coroutine) -> Unit {
  task_schedule(coro.waitable_set).run_later.push_back(coro)
}

///|
fn has_immediately_ready_task(waitable_set : WaitableSet) -> Bool {
  match scheduler.tasks.get(waitable_set) {
    Some(schedule) => !schedule.run_later.is_empty()
    None => false
  }
}

///|
fn no_more_work(waitable_set : WaitableSet) -> Bool {
  match scheduler.tasks.get(waitable_set) {
    Some(schedule) => schedule.blocking == 0 && schedule.run_later.is_empty()
    None => true
  }
}

///|
fn forget_schedule(waitable_set : WaitableSet) -> Unit {
  scheduler.tasks.remove(waitable_set)
}

///|
/// Run one fair scheduling round for a component task. Coroutines woken or
/// spawned during this round are left for the next round so the component
/// event loop gets a chance to poll completed waitables.
fn reschedule(waitable_set : WaitableSet) -> Unit {
  guard scheduler.tasks.get(waitable_set) is Some(schedule) else { return }
  let count = schedule.run_later.length()
  for _ in 0..<count {
    guard schedule.run_later.pop_front() is Some(coro) else { break }
    coro.ready = false
    guard coro.state is Suspend(ok_cont~, err_cont~) else {  }
    coro.state = Running
    let last_coro = scheduler.curr_coro
    scheduler.curr_coro = Some(coro)
    if coro.cancelled && !coro.shielded {
      err_cont(Cancelled::Cancelled)
    } else {
      ok_cont(())
    }
    scheduler.curr_coro = last_coro
  }
}
