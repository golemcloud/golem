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
priv struct CondVarWaiter {
  // Once a signal is assigned, cancellation must not lose it before the
  // waiter resumes. This matches moonbitlang/async's condition variable.
  mut woken : Bool
  mut coro : Coroutine?
}

///|
/// A condition variable for synchronization between tasks.
pub struct CondVar {
  priv waiters : @deque.Deque[CondVarWaiter]
}

///|
/// Create a new condition variable.
#alias(new, deprecated)
pub fn CondVar::CondVar() -> CondVar {
  { waiters: Deque([]) }
}

///|
/// Wait until the condition variable is signaled.
pub async fn CondVar::wait(self : CondVar) -> Unit {
  let waiter = { woken: false, coro: Some(current_coroutine()) }
  self.waiters.push_back(waiter)
  suspend() catch {
    _ if waiter.woken => ()
    err => {
      waiter.coro = None
      raise err
    }
  }
}

///|
/// Wake the first waiting task. Does nothing when no task is waiting.
pub fn CondVar::signal(self : CondVar) -> Unit {
  while self.waiters.pop_front() is Some(waiter) {
    if waiter.coro is Some(coro) {
      waiter.woken = true
      coro.wake()
      break
    }
  }
}

///|
/// Wake all waiting tasks. Does nothing when no task is waiting.
pub fn CondVar::broadcast(self : CondVar) -> Unit {
  for waiter in self.waiters {
    if waiter.coro is Some(coro) {
      waiter.woken = true
      coro.wake()
    }
  }
  self.waiters.clear()
}
