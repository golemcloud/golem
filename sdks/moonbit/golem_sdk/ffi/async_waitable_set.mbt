///|
priv enum TaskStatus {
      Fail(Error)
      Running
      Done
}

///|
/// A `Task` represents a waitable task context that can manage waitables and child coroutines.
///
struct Task {
      id : Int
      children : Map[Int, (&Waitable, Coroutine)]
      task_defer : Array[() -> Unit raise]
      resources : @deque.Deque[Int]
      mut task : Coroutine?
      mut waiting : Int
      mut status : TaskStatus
}

///|
pub let task_map : Map[Int, Task] = {}

///|
pub fn Task::new() -> Task {
      let waitable_set = waitable_set_new()
      _async_debug("waitable-set-new(\{waitable_set})")
      context_set(waitable_set)
      {
            id: waitable_set,
            children: {},
            resources: @deque.Deque::new(),
            task_defer: [],
            status: Running,
            waiting: 0,
            task: None,
      }
}

///|
pub fn Task::from_raw(raw : Int) -> Task {
      guard raw != 0
      context_set(raw)
      _async_debug("context-set(\{raw})")
      {
            id: raw,
            children: {},
            resources: @deque.Deque::new(),
            task_defer: [],
            status: Running,
            waiting: 0,
            task: None,
      }
}

///|
/// Check if the task is failed and return the error
pub fn Task::is_fail(self : Self) -> Error? {
      match self.status {
            Fail(err) => Some(err)
            _ => None
      }
}

///|
/// Check if all waitables are done
pub fn Task::no_wait(self : Self) -> Bool {
      self.waiting == 0
}

///|
/// Check if the task is done or failed
pub fn Task::is_done(self : Self) -> Bool {
      match self.status {
            Done => true
            Fail(_) => true
            Running => false
      }
}

///|
pub fn Task::handle(self : Self) -> Int {
      self.id
}

///|
pub fn Task::blocking_wait(self : Self) -> (Int, Int, Int) {
      let result : FixedArray[Int] = FixedArray::make(2, 0)
      let result_ptr = int_array2ptr(result)
      let event0 = waitable_set_wait(self.id, result_ptr)
      _async_debug("waitable_set_wait(\{event0}, \{result[0]}, \{result[1]})")
      (event0, result[0], result[1])
}

///|
pub fn Task::blocking_poll(self : Self) -> (Int, Int, Int) {
      let result : FixedArray[Int] = FixedArray::make(2, 0)
      let result_ptr = int_array2ptr(result)
      let event0 = waitable_set_poll(self.id, result_ptr)
      _async_debug("waitable-set-poll(\{event0}, \{result[0]}, \{result[1]})")
      (event0, result[0], result[1])
}

///|
/// Add a waitable to the waitable set and increase the waiting count
pub fn[T : Waitable] Task::add_waitable(
self : Self,
waitable : T,
coro : Coroutine,
) -> Unit {
      waitable_join(waitable.handle(), self.id)
      self.children[waitable.handle()] = (waitable, coro)
      self.resources.push_back(waitable.handle())
      _async_debug("waitable-set-join(\{waitable.handle()}, \{self.id})")
      self.waiting += 1
}

///|
/// When a waitable is done will be removed from the waitable set
/// then waitable will be try to drop
pub fn[T : Waitable] Task::remove_waitable(self : Self, state : T) -> Unit {
      _async_debug("waitable-set-join(\{state.handle()}, 0)")
      waitable_join(state.handle(), 0)
      self.waiting -= 1
}

///|
pub fn[T : Waitable] Task::drop_waitable(self : Self, state : T) -> Unit {
      let _ = state.drop()
      if self.resources.search(state.handle()) is Some(idx) {
            let _ = self.resources.remove(idx)

      }
      self.children.remove(state.handle())
}

///|
/// Cancel a waitable, remove it from the waitable set and force drop it
pub fn[T : Waitable] Task::cancel_waitable(self : Self, state : T) -> Unit {
      waitable_join(state.handle(), 0)
      _async_debug("waitable-set-join(\{state.handle()}, 0)")
      self.waiting -= 1
      state.cancel()
      let _ = state.drop()
      self.children.remove(state.handle())
}

///|
/// set current task context to 0 and let runner drop the waitable set
pub fn Task::drop(self : Self) -> Unit {
      context_set(0)
      defer waitable_set_drop(self.id)
      _async_debug("context-set(0)")
}

///|
/// Spawns a coroutine to execute an async function and without waits for its completion
/// while managing the waitable state.
pub fn Task::spawn(_self : Self, f : async () -> Unit) -> Unit {
      let _ = spawn(f)
      // start the coroutine
      rschedule()
}

///|
/// This function spawns a coroutine to run the async function and waits for its completion
pub async fn Task::wait(_ : Self, f : async () -> Unit) -> Unit {
      let coro = spawn(f)
      // start the coroutine
      rschedule()
      Coroutine::wait(coro)
}

///|
pub fn Task::add_defer(self : Self, f : () -> Unit raise) -> Unit {
      self.task_defer.push(f)
}

///|
pub fn callback(event : Int, waitable_id : Int, code : Int) -> Int {
      let event = Event::decode(event)
      _async_debug("callback(\{event}, \{waitable_id}, \{code})")
      let task = match current_waitable_set() {
            Some(task) => task
            None => current_task()
      }
      // Handle the event for the current waitable task
      match event {
            FutureRead | FutureWrite | StreamRead | StreamWrite | Subtask => {
                  let (state, coro) = task.children[waitable_id]
                  state.update(code~)
                  // schedule next coroutine
                  coro.wake()
                  rschedule()
                  if task.no_wait() && task.task is Some(parent) {
                        // run the parent coroutine when all waitables are done
                        // parent coroutine may execute return/cancel
                        parent.wake()
                        rschedule()
                        return CallbackCode::Exit.encode()
                  }
                  return CallbackCode::Wait(task.id).encode()
            }
            TaskCancel => {
                  if task.task is Some(parent) {
                        parent.wake()
                  }
                  task.children
                  .values()
                  .each(child => {
                        let (state, coro) = child
                        task.cancel_waitable(state)
                        coro.cancel()
                  })
                  rschedule()
                  return CallbackCode::Exit.encode()
            }
            None => {
                  rschedule()
                  return CallbackCode::Exit.encode()
            }
      }
}

///|
pub fn Task::with_waitable_set(
self : Self,
f : async (Self) -> Unit,
is_drop? : Bool = false,
) -> Coroutine noraise {
      let parent = spawn(async fn() -> Unit noraise {
            self.status = Running
            defer {
                  while self.resources.pop_front() is Some(handle) {
                        let state = self.children.get(handle)
                        if state is Some((state, _)) {
                              let _ = state.drop()
                              self.children.remove(handle)
                        }
                  }
                  if self.status is Running {
                        self.status = Done
                  }
                  task_map.remove(self.id)

                  // this defer block recycles waitable task resources
                  while self.task_defer.pop() is Some(defer_block) {
                        defer_block() catch {
                              err => if self.status is Done { self.status = Fail(err) }
                        }
                  }

                  // runner will drop the waitable set
                  // export async function needs to keep the waitable set
                  if is_drop {
                        self.drop()
                  }
            }
            f(self) catch {
                  err => if self.status is Running { self.status = Fail(err) }
            }
            if !self.no_wait() {
                  _async_debug("task-wait-loop(\{self.id})")
                  suspend() catch {
                        err => if self.status is Running { self.status = Fail(err) }
                  }
            }
      })
      self.task = Some(parent)
      // start the parent coroutine
      parent.run()
      rschedule()
      parent
}

///|
fn current_waitable_set() -> Task? {
      let ctx = context_get()
      _async_debug("context-get(\{ctx})")
      if ctx == 0 {
            None
      } else {
            match task_map.get(ctx) {
                  Some(task) => Some(task)
                  None => {
                        let ctx = Task::from_raw(ctx)
                        task_map[ctx.id] = ctx
                        Some(ctx)
                  }
            }
      }
}

///|
pub fn current_task() -> Task {
      let ctx = context_get()
      if ctx == 0 {
            let ctx = Task::new()
            task_map[ctx.id] = ctx
            ctx
      } else {
            match task_map.get(ctx) {
                  Some(task) => task
                  None => {
                        let ctx = Task::from_raw(ctx)
                        task_map[ctx.id] = ctx
                        ctx
                  }
            }
      }
}
