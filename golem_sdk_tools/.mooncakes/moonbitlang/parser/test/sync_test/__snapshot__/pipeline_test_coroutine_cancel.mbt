async fn[T, E : Error] suspend_with_error(f : ((T) -> Unit, (E) -> Unit) -> Unit) -> T raise E = "%async.suspend"
fn run_async(f : async () -> Unit noraise) -> Unit = "%async.run"

suberror CancellationError

struct Coroutine {
  ok : () -> Unit
  err : (CancellationError) -> Unit
}

struct Scheduler {
  mut next_id : Int
  mut curr_id : Int
  mut err : Error?
  coros : @queue.Queue[Coroutine]
}

fn Scheduler::new() -> Scheduler {
  { next_id: 0, curr_id: 0, err: None, coros: @queue.new() }
}

fn Scheduler::run_all(self : Scheduler) -> Unit raise {
  loop self.coros.pop() {
    None => ()
    Some (coro) => {
      (coro.ok)()
      match self.err {
        None => continue self.coros.pop()
        Some(err) => {
          for coro in self.coros {
            (coro.err)(CancellationError)
          }
          self.coros.clear()
          raise err
        }
      }
    }
  }
}


fn Scheduler::spawn(self : Scheduler, f : async (Int) -> Unit raise) -> Unit {
  let id = self.next_id
  self.next_id += 1
  fn ok() {
    run_async(fn () {
      try f(id) catch {
        err => self.err = Some(err)
      }
    })
  }
  fn err(_err) {}
  self.coros.push({ ok, err })
}

async fn Scheduler::suspend(self : Scheduler) -> Unit raise CancellationError {
  suspend_with_error(fn (ok, err) {
    self.coros.push({ ok: fn () { ok(()) }, err })
  })
}


fn init {
  let ctx = Scheduler::new()
  async fn worker(id : Int) -> Unit raise {
    try {
      for i in 0..<10 {
        println("thread \{id}: \{i}")
        if id == 2 && i == 3 {
          fail("thread \{id}: failure")
        }
        ctx.suspend()
      }
    } catch {
      err => {
        println("thread \{id} terminated due to \{err}")
        raise err
      }
    }
  }
  for _ in 0..<5 {
    ctx.spawn(worker)
  }
  println(try? ctx.run_all())
}
