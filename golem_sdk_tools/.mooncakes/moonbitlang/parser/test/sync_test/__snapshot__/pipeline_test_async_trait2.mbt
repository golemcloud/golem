async fn[T] suspend(cb : ((T) -> Unit) -> Unit) -> T noraise = "%async.suspend"
fn run_async(f : async () -> Unit noraise) -> Unit = "%async.run"

trait I {
  async f(Self) -> Self noraise
}

impl I for Int with f(self) {
  suspend(k => k(self))
}

fn main {
  run_async(fn () {
    I::f(42) |> println
  })
}
