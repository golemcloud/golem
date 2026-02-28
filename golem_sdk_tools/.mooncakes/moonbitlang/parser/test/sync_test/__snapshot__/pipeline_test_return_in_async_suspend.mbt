///|
async fn[T] suspend(f : ((T) -> Unit) -> Unit) -> T noraise = "%async.suspend"

///|
fn run_async(f : async () -> Unit noraise) -> Unit = "%async.run"

///|
async fn return_in_suspend(cond : Bool) -> String noraise {
  suspend(_ => {
    if cond {
      return
    }
    println("after return")
  })
  "42"
}

///|
fn main {
  run_async(() => println(return_in_suspend(true)))
  run_async(() => println(return_in_suspend(false)))
}
