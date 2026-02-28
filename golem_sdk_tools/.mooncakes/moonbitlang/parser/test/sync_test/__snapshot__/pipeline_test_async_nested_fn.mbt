async fn[T] suspend(f : ((T) -> Unit) -> Unit) -> T noraise = "%async.suspend"
fn run_async(f : async () -> Unit noraise) -> Unit = "%async.run"

fn spawn(f : async () -> Unit noraise) -> Unit {
  run_async(f)
}

async fn worker() -> Int noraise {
  suspend(fn (k) {
    k(42)
  })
}

async fn f() -> Unit noraise {
  spawn(fn () { println(worker()) })
  println(worker() + 1)
}

fn init {
  spawn(fn () {
    f()
  })
}
