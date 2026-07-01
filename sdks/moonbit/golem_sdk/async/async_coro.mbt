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
priv enum State {
      Done
      Fail(Error)
      Running
      Suspend(ok_cont~ : (Unit) -> Unit, err_cont~ : (Cancelled) -> Unit)
}

///|
struct Coroutine {
      coro_id : Int
      mut state : State
      mut shielded : Bool
      mut cancelled : Bool
      mut ready : Bool
      downstream : Set[Coroutine]
}

///|
impl Eq for Coroutine with equal(c1, c2) {
      c1.coro_id == c2.coro_id
}

///|
impl Hash for Coroutine with hash_combine(self, hasher) {
      self.coro_id.hash_combine(hasher)
}

///|
fn Coroutine::wake(self : Coroutine) -> Unit {
      self.ready = true
      scheduler.run_later.push_back(self)
}

///|
pub fn is_being_cancelled() -> Bool {
      let coro = current_coroutine()
      coro.cancelled && not(coro.shielded)
}

///|
pub(all) suberror Cancelled derive(Show)

///|
fn Coroutine::cancel(self : Coroutine) -> Unit {
      self.cancelled = true
      if not(self.shielded || self.ready) {
            self.wake()
      }
}

///|
pub async fn pause() -> Unit raise Cancelled {
      guard scheduler.curr_coro is Some(coro)
      if coro.cancelled && not(coro.shielded) {
            raise Cancelled::Cancelled
      }
      async_suspend(fn(ok_cont, err_cont) {
            guard coro.state is Running
            coro.state = Suspend(ok_cont~, err_cont~)
            coro.ready = true
            scheduler.run_later.push_back(coro)
      })
}

///|
pub async fn suspend() -> Unit raise Cancelled {
      guard scheduler.curr_coro is Some(coro)
      if coro.cancelled && not(coro.shielded) {
            raise Cancelled::Cancelled
      }
      scheduler.blocking += 1
      defer {
            scheduler.blocking -= 1
      }
      async_suspend(fn(ok_cont, err_cont) {
            guard coro.state is Running
            coro.state = Suspend(ok_cont~, err_cont~)
      })
}

///|
fn spawn(f : async () -> Unit) -> Coroutine {
      scheduler.coro_id += 1
      let coro = {
            state: Running,
            ready: true,
            shielded: true,
            downstream: Set::new(),
            coro_id: scheduler.coro_id,
            cancelled: false,
      }
      fn run(_) {
            run_async(fn() {
                  coro.shielded = false
                  try f() catch {
                        err => coro.state = Fail(err)
                  } noraise {
                        _ => coro.state = Done
                  }
                  for coro in coro.downstream {
                        coro.wake()
                  }
                  coro.downstream.clear()
            })
      }

      coro.state = Suspend(ok_cont=run, err_cont=_ => ())
      scheduler.run_later.push_back(coro)
      coro
}

///|
fn Coroutine::unwrap(self : Coroutine) -> Unit raise {
      match self.state {
            Done => ()
            Fail(err) => raise err
            Running | Suspend(_) => panic()
      }
}

///|
async fn Coroutine::wait(target : Coroutine) -> Unit {
      guard scheduler.curr_coro is Some(coro)
      guard not(physical_equal(coro, target))
      match target.state {
            Done => return
            Fail(err) => raise err
            Running | Suspend(_) => ()
      }
      target.downstream.add(coro)
      try suspend() catch {
            err => {
                  target.downstream.remove(coro)
                  raise err
            }
      } noraise {
            _ => target.unwrap()
      }
}

///|
fn Coroutine::check_error(coro : Coroutine) -> Unit raise {
      match coro.state {
            Fail(err) => raise err
            Done | Running | Suspend(_) => ()
      }
}

///|
pub async fn protect_from_cancel(f : async () -> Unit) -> Unit {
      guard scheduler.curr_coro is Some(coro)
      if coro.shielded {
            // already in a shield, do nothing
            f()
      } else {
            coro.shielded = true
            defer {
                  coro.shielded = false
            }
            f()
            if coro.cancelled {
                  raise Cancelled::Cancelled
            }
      }
}
