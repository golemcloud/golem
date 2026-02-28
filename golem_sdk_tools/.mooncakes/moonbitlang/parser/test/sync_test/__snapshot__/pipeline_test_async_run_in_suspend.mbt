async fn[T] suspend(f : ((T) -> Unit) -> Unit) -> T noraise = "%async.suspend"
fn run_async(f : async () -> Unit noraise) -> Unit = "%async.run"

async fn f() -> Int noraise {
  suspend(k => k(42))
}

async fn run_in_suspend() -> Unit noraise {
  suspend(fn (k) {
    run_async(fn () {
      println(f())
    })
    k(())
  })
}

fn init {
  run_async(run_in_suspend)
}
