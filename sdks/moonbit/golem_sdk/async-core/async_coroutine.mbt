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
  Suspend(ok_cont~ : (Unit) -> Unit, err_cont~ : (Error) -> Unit)
}

///|
struct Coroutine {
  coro_id : Int
  waitable_set : WaitableSet
  mut state : State
  mut shielded : Bool
  mut cancelled : Bool
  mut ready : Bool
  mut spawner : ((async () -> Unit) -> Unit)?
  downstream : Set[Coroutine]
}

///|
impl Eq for Coroutine with fn equal(c1, c2) {
  c1.coro_id == c2.coro_id
}

///|
impl Hash for Coroutine with fn hash_combine(self, hasher) {
  self.coro_id.hash_combine(hasher)
}

///|
fn Coroutine::wake(self : Coroutine) -> Unit {
  if !self.ready {
    self.ready = true
    enqueue(self)
    signal_component_task(self.waitable_set)
  }
}

///|
pub fn is_being_cancelled() -> Bool {
  let coro = current_coroutine()
  coro.cancelled && !coro.shielded
}

///|
pub fn check_cancellation() -> Unit raise {
  if is_being_cancelled() {
    raise Cancelled::Cancelled
  }
}

///|
pub(all) suberror Cancelled derive(Debug)

///|
fn Coroutine::cancel(self : Coroutine) -> Unit {
  self.cancelled = true
  if !self.shielded {
    self.wake()
  }
}

///|
async fn suspend() -> Unit {
  guard scheduler.curr_coro is Some(coro)
  if coro.cancelled && !coro.shielded {
    raise Cancelled::Cancelled
  }
  let schedule = task_schedule(coro.waitable_set)
  schedule.blocking += 1
  defer {
    schedule.blocking -= 1
  }
  async_suspend(fn(ok_cont, err_cont) {
    guard coro.state is Running
    coro.state = Suspend(ok_cont~, err_cont~)
  })
}

///|
fn spawn_owned(
  f : async () -> Unit,
  waitable_set : WaitableSet,
  inherited_spawner : ((async () -> Unit) -> Unit)?,
) -> Coroutine {
  scheduler.coro_id += 1
  let coro = {
    state: Running,
    ready: true,
    shielded: true,
    downstream: Set([]),
    coro_id: scheduler.coro_id,
    waitable_set,
    cancelled: false,
    spawner: inherited_spawner,
  }
  fn run(_) {
    run_async(() => {
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
  enqueue(coro)
  coro
}

///|
fn spawn(f : async () -> Unit) -> Coroutine {
  match scheduler.curr_coro {
    Some(parent) => spawn_owned(f, parent.waitable_set, parent.spawner)
    None => spawn_owned(f, current_waitableset(), None)
  }
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
  guard !physical_equal(coro, target)
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
pub async fn[X] protect_from_cancel(
  f : async () -> X,
  resume_on_cancel? : Bool = false,
) -> X {
  guard scheduler.curr_coro is Some(coro)
  if coro.shielded {
    // already in a shield, do nothing
    f()
  } else {
    coro.shielded = true
    defer {
      coro.shielded = false
    }
    let result = f()
    if !resume_on_cancel && coro.cancelled {
      raise Cancelled::Cancelled
    }
    result
  }
}
