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
/// A mutex for synchronization between tasks.
#valtype
pub struct Mutex {
  priv semaphore : Semaphore
}

///|
/// Create a mutex in the released state.
pub fn Mutex::Mutex() -> Mutex {
  { semaphore: Semaphore(1) }
}

///|
/// Acquire the mutex in FIFO order.
pub async fn Mutex::acquire(self : Mutex) -> Unit {
  self.semaphore.acquire()
}

///|
/// Try to acquire the mutex without waiting.
pub fn Mutex::try_acquire(self : Mutex) -> Bool {
  self.semaphore.try_acquire()
}

///|
/// Release the mutex. The mutex must already be acquired.
pub fn Mutex::release(self : Mutex) -> Unit {
  self.semaphore.release()
}
