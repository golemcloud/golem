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
/// `Task[X]` represents a running task with result type `X`,
/// it can be used to wait and retrieve the result value of the task.
struct Task_[X] {
      value : Ref[X?]
      coro : Coroutine
}

///|
/// Wait for a task and retrieve its result value.
/// If the task fails, `wait` will also fail with the same error.
///
/// If the current task is cancelled, `wait` return immediately with error.
pub async fn[X] Task_::wait(self : Task_[X]) -> X {
      self.coro.wait()
      self.value.val.unwrap()
}

///|
/// Try to obtain the result of the task.
/// If the task already terminated, its result value will be returned.
/// If the task already failed, `try_wait` will fail immediately.
/// If the task is still running, `try_wait` returns `None`.
/// `try_wait` is a synchoronous function: it never blocks.
pub fn[X] Task_::try_wait(self : Task_[X]) -> X? raise {
      self.coro.check_error()
      self.value.val
}

///|
/// Cancel a task. Subsequent attempt to wait for the task will receive error.
/// Note that if the task is *not* spawned with `allow_failure=true`,
/// the whole task group will fail too.
pub fn[X] Task_::cancel(self : Task_[X]) -> Unit {
      self.coro.cancel()
}
