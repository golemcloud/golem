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
priv struct SemaphoreWaiter {
  // Once a permit is assigned, cancellation must not lose it before the
  // waiter resumes. This matches moonbitlang/async's semaphore semantics.
  mut acquired : Bool
  mut coro : Coroutine?
}

///|
pub struct Semaphore {
  size : Int
  priv mut value : Int
  priv waiters : @deque.Deque[SemaphoreWaiter]
}

///|
/// Create a semaphore with at most `size` permits.
///
/// The initial number of available permits is `initial_value`, or `size` when
/// omitted.
#alias(new, deprecated)
pub fn Semaphore::Semaphore(
  size : Int,
  initial_value? : Int = size,
) -> Semaphore {
  if size <= 0 {
    abort("size of semaphore must be positive")
  }
  guard initial_value >= 0 && initial_value <= size
  { value: initial_value, size, waiters: Deque([]) }
}

///|
/// Release one permit. Waiting coroutines acquire permits in FIFO order.
///
/// Each call must correspond one-to-one with a successful `acquire`. Calling
/// `release` without owning a permit is invalid.
pub fn Semaphore::release(self : Semaphore) -> Unit {
  if self.value >= self.size {
    abort("semaphore: too many release")
  }
  while self.waiters.pop_front() is Some(waiter) {
    if waiter.coro is Some(coro) {
      waiter.acquired = true
      coro.wake()
      break
    }
  } nobreak {
    self.value += 1
  }
}

///|
/// Acquire one permit, suspending in FIFO order when none is available.
pub async fn Semaphore::acquire(self : Semaphore) -> Unit {
  if self.value > 0 {
    self.value -= 1
  } else {
    let waiter = { coro: Some(current_coroutine()), acquired: false }
    self.waiters.push_back(waiter)
    suspend() catch {
      _ if waiter.acquired => ()
      err => {
        waiter.coro = None
        raise err
      }
    }
  }
}

///|
/// Try to acquire one permit without suspending.
pub fn Semaphore::try_acquire(self : Semaphore) -> Bool {
  if self.value > 0 {
    self.value -= 1
    true
  } else {
    false
  }
}
