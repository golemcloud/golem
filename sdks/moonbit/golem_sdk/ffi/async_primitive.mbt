///|
async fn[T, E : Error] async_suspend(
cb : ((T) -> Unit, (E) -> Unit) -> Unit,
) -> T raise E = "%async.suspend"

///|
fn run_async(f : async () -> Unit noraise) = "%async.run"

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
      mut state : State
      mut shielded : Bool
      mut cancelled : Bool
      mut ready : Bool
      downstream : Map[Int, Coroutine]
}

///|
pub impl Eq for Coroutine with equal(c1, c2) {
      c1.coro_id == c2.coro_id
}

///|
pub impl Hash for Coroutine with hash_combine(self, hasher) {
      self.coro_id.hash_combine(hasher)
}

///|
pub fn Coroutine::wake(self : Coroutine) -> Unit {
      self.ready = true
      scheduler.run_later.push_back(self)
}

///|
pub fn Coroutine::run(self : Coroutine) -> Unit {
      self.ready = true
      scheduler.run_later.push_front(self)
}

///|
pub fn Coroutine::is_done(self : Coroutine) -> Bool {
      match self.state {
            Done => true
            Fail(_) => true
            Running | Suspend(_) => false
      }
}

///|
pub fn is_being_cancelled() -> Bool {
      current_coroutine().cancelled
}

///|
pub fn current_coroutine_done() -> Bool {
      guard scheduler.curr_coro is Some(coro) else { return true }
      coro.is_done()
}

///|
pub(all) suberror Cancelled derive(Show)

///|
pub fn Coroutine::cancel(self : Coroutine) -> Unit {
      self.cancelled = true
      if not(self.shielded || self.ready) {
            self.wake()
      }
}

///|
pub async fn pause() -> Unit {
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
pub async fn suspend() -> Unit {
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
pub fn spawn(f : async () -> Unit) -> Coroutine {
      scheduler.coro_id += 1
      let coro = {
            state: Running,
            ready: true,
            shielded: false,
            downstream: {},
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
                  for _, coro in coro.downstream {
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
pub fn Coroutine::unwrap(self : Coroutine) -> Unit raise {
      match self.state {
            Done => ()
            Fail(err) => raise err
            Running | Suspend(_) => panic()
      }
}

///|
pub async fn Coroutine::wait(target : Coroutine) -> Unit {
      guard scheduler.curr_coro is Some(coro)
      guard not(physical_equal(coro, target))
      match target.state {
            Done => return
            Fail(err) => raise err
            Running | Suspend(_) => ()
      }
      target.downstream[coro.coro_id] = coro
      try suspend() catch {
            err => {
                  target.downstream.remove(coro.coro_id)
                  raise err
            }
      } noraise {
            _ => target.unwrap()
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
pub fn no_more_work() -> Bool {
      scheduler.blocking == 0 && scheduler.run_later.is_empty()
}

///|
pub fn rschedule() -> Unit {
      while scheduler.run_later.pop_front() is Some(coro) {
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
