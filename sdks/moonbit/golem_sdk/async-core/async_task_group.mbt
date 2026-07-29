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
priv enum TaskGroupState {
  Done
  Fail(Error)
  Running
}

///|
/// A `TaskGroup` can be used to spawn children tasks that run in parallel.
/// Task groups implements *structured concurrency*:
/// a task group will only return after all its children task terminates.
///
/// Task groups also handles *error propagation*:
/// by default, if any child task raises error,
/// the whole task group will also raise that error,
/// and all other remaining child tasks will be cancelled.
///
/// The type parameter `X` in `TaskGroup[X]` is the result type of the group,
/// see `with_task_group` for more detail.
pub struct TaskGroup[X] {
  priv children : Set[Coroutine]
  priv parent : Coroutine
  priv mut waiting : Int
  priv mut state : TaskGroupState
  priv mut result : X?
  priv group_defer : Array[async () -> Unit]
}

///|
fn[X] TaskGroup::spawn_coroutine(
  self : TaskGroup[X],
  f : async () -> Unit,
  no_wait~ : Bool,
  allow_failure~ : Bool,
) -> Coroutine {
  guard self.state is Running || !self.children.is_empty() else {
    abort("trying to spawn from a terminated task group")
  }
  if !no_wait {
    self.waiting += 1
  }
  async fn worker() {
    let coro = current_coroutine()
    defer {
      self.children.remove(coro)
      if !no_wait {
        self.waiting -= 1
        if self.waiting == 0 && self.state is Running {
          for child in self.children {
            child.cancel()
          }
          self.state = Done
        }
      }
      if self.children.is_empty() {
        self.parent.wake()
      }
    }
    f() catch {
      err if allow_failure => raise err
      Cancelled::Cancelled as err if is_being_cancelled() => raise err
      err => {
        if self.state is Running {
          for child in self.children {
            child.cancel()
          }
          self.state = Fail(err)
        } else {
          self.state = Fail(err)
        }
        raise err
      }
    }
  }

  let coro = spawn(worker)
  self.children.add(coro)
  if !(self.state is Running) {
    coro.cancel()
  }
  coro
}

///|
/// Spawn a child task in a task group, and run it asynchronously in the background.
///
/// Unless `no_wait` (`false` by default) is `true`,
/// the task group will wait for this task to complete before normal exit.
/// No matter what the value of `no_wait` is,
/// `with_task_group` will only return after all child tasks terminate.
/// The task will be cancelled automatically if it is still running
/// when the task group wishes to terminate.
///
/// Unless `allow_failure` (`false` by default) is `true`,
/// the whole task group will also fail if the spawned task fails,
/// other tasks in the group will be cancelled in this case.
///
/// Cancelled tasks are not considered failing, although they raise a
/// cancellation error. A different error raised during cancellation is still
/// considered a failure.
///
/// A task spawned while the group is cancelling still starts, but begins in a
/// cancelled state so its entry cleanup can run. Spawning after the group has
/// terminated or from a group defer is invalid and aborts.
///
/// It is undefined whether the child task will start running immediately
/// before `spawn_bg` returns.
pub fn[X] TaskGroup::spawn_bg(
  self : TaskGroup[X],
  f : async () -> Unit,
  no_wait? : Bool = false,
  allow_failure? : Bool = false,
) -> Unit {
  ignore(self.spawn_coroutine(f, no_wait~, allow_failure~))
}

///|
/// Spawn a child task in a task group, compute a result asynchronously.
/// A task handle will be returned, the result value of the task can be waited
/// and retrieved using `.wait()`, or cancelled using `.cancel()`.
///
/// Unless `no_wait` (`false` by default) is `true`,
/// the task group will wait for this task to complete before normal exit.
/// No matter what the value of `no_wait` is,
/// `with_task_group` will only return after all child tasks terminate.
/// The task will be cancelled automatically if it is still running
/// when the task group wishes to terminate.
///
/// Unless `allow_failure` (`false` by default) is `true`,
/// the whole task group will also fail if the spawned task fails,
/// other tasks in the group will be cancelled in this case.
///
/// Cancelled tasks are not considered failing, although they raise a
/// cancellation error. A different error raised during cancellation is still
/// considered a failure.
///
/// A task spawned while the group is cancelling still starts, but begins in a
/// cancelled state so its entry cleanup can run. Spawning after the group has
/// terminated or from a group defer is invalid and aborts.
///
/// It is undefined whether the child task will start running immediately
/// before `spawn` returns.
pub fn[G, X] TaskGroup::spawn(
  self : TaskGroup[G],
  f : async () -> X,
  no_wait? : Bool = false,
  allow_failure? : Bool = false,
) -> Task[X] {
  let value = @ref.new(Option::None)
  let coro = self.spawn_coroutine(
    () => value.val = Some(f()),
    no_wait~,
    allow_failure~,
  )
  { value, coro }
}

///|
/// Attach a defer block, represented as a cleanup function, to a task group.
/// The cleanup function runs after every child task has terminated. Spawning a
/// new child from a group defer is invalid and aborts.
/// Group scoped defer blocks are executed in FILO order, just like normal `defer`.
/// `with_task_group` will only exit after all group defer blocks terminate.
///
/// Note that if the whole task group is cancelled,
/// async operations in group defer block will be cancelled immediately too.
/// Users can use `protect_from_cancel` to prevent async tasks from being cancelled.
/// It is highly recommended to add a hard timeout to async defer block in this case,
/// to avoid infinite hanging due to blocked operation.
pub fn[X] TaskGroup::add_defer(
  self : TaskGroup[X],
  block : async () -> Unit,
) -> Unit {
  guard self.state is Running else {
    abort("trying to attach defer to a terminated task group")
  }
  self.group_defer.push(block)
}

///|
/// `with_task_group(f)` creates a new task group and run `f` with the new group.
/// `f` itself will be run in a child task of the new group.
/// `with_task_group` exits after all the whole group terminates,
/// which means all child tasks in the group have terminated, including `f`.
///
/// If all children task terminate successfully,
/// `with_task_group` will return the result of `f`.
pub async fn[X] with_task_group(f : async (TaskGroup[X]) -> X) -> X {
  let tg = {
    children: Set([]),
    parent: current_coroutine(),
    waiting: 0,
    state: Running,
    result: None,
    group_defer: [],
  }
  let curr = current_coroutine()
  let prev = curr.spawner
  curr.spawner = Some(fn(child) { tg.spawn_bg(child) })
  defer {
    curr.spawner = prev
  }
  tg.spawn_bg(() => {
    let value = f(tg)
    if tg.result is None {
      tg.result = Some(value)
    }
  })
  suspend() catch {
    err => {
      if tg.state is Running {
        tg.state = Fail(err)
      }
      if !tg.children.is_empty() {
        for child in tg.children {
          child.cancel()
        }
        protect_from_cancel(() => suspend(), resume_on_cancel=true)
      }
    }
  }
  guard tg.children.is_empty()
  while tg.group_defer.pop() is Some(defer_block) {
    defer_block() catch {
      err => if tg.state is Done { tg.state = Fail(err) }
    }
  }
  match tg.state {
    Done => tg.result.unwrap()
    Fail(err) => raise err
    Running => panic()
  }
}

///|
/// Force a task group to terminate immediately with the given result value.
/// All child tasks in the group, including potentially the current one,
/// will be cancelled.
pub fn[X] TaskGroup::return_immediately(
  self : TaskGroup[X],
  value : X,
) -> Unit raise {
  if self.result is None {
    self.result = Some(value)
  }
  if self.state is Running {
    self.state = Done
    for child in self.children {
      child.cancel()
    }
  }
  if is_being_cancelled() {
    raise Cancelled::Cancelled
  }
}
