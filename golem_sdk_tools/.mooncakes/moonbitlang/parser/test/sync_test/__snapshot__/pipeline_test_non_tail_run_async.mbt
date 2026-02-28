async fn[X] suspend(
  f : ((X) -> Unit) -> Unit,
) -> X noraise = "%async.suspend"

fn run_async(f : async () -> Unit noraise) -> Unit = "%async.run"

async fn answer() -> Int noraise {
  suspend(k => k(42))
}

let flag : Ref[Bool] = Ref::new(true)

fn main {
  run_async(fn() {
    if flag.val {
      // the `return` here should be delimited by `run_async`
      return
    }
    println(answer())
  })
  println("done")
}
