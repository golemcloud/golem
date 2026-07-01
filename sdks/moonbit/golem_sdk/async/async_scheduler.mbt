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
      mut blocking : Int
      run_later : @deque.Deque[Coroutine]
}

///|
let scheduler : Scheduler = {
      coro_id: 0,
      curr_coro: None,
      blocking: 0,
      run_later: @deque.new(),
}

///|
pub fn current_coroutine() -> Coroutine {
      scheduler.curr_coro.unwrap()
}

///|
pub fn has_immediately_ready_task() -> Bool {
      !scheduler.run_later.is_empty()
}

///|
pub fn no_more_work() -> Bool {
      scheduler.blocking == 0 && scheduler.run_later.is_empty()
}

///|
pub fn reschedule() -> Unit {
      while scheduler.run_later.pop_front() is Some(coro) {
            coro.ready = false
            guard coro.state is Suspend(ok_cont~, err_cont~) else {  }
            coro.state = Running
            let last_coro = scheduler.curr_coro
            scheduler.curr_coro = Some(coro)
            if coro.cancelled && not(coro.shielded) {
                  err_cont(Cancelled::Cancelled)
            } else {
                  ok_cont(())
            }
            scheduler.curr_coro = last_coro
      }
}
