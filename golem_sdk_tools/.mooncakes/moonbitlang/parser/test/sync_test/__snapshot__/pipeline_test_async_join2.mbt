fn async_run(f : async () -> Unit noraise) -> Unit = "%async.run"

async fn[T, E : Error] async_suspend(
  f : ((T) -> Unit, (E) -> Unit) -> Unit
) -> T raise E = "%async.suspend"

fn main {
  async fn a() -> &Show raise {
    async_suspend(fn(resolve : (Int) -> Unit, reject : (Error) -> Unit) {
      if true {
        resolve(42)
      } else {
        reject(Failure::Failure("a"))
      }
    })
  }

  async_run(async fn() -> Unit noraise {
    try println(a()) catch {
      e => println(e)
    }
  })
}
