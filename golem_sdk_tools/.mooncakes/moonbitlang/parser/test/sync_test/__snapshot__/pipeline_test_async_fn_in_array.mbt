fn run_async(f : async () -> Unit noraise) -> Unit = "%async.run"
async fn[T] suspend(cb : ((T) -> Unit) -> Unit) -> T noraise = "%async.suspend"

async fn async_id(x : Int) -> Int noraise {
  suspend(f => f(x))
}

fn main {
  let arr1 : Array[() -> Unit] = []
  arr1..push(() => println(1))..push(() => println(2))
  for f in arr1 {
    f()
  }
  let arr2 : Array[async () -> Unit noraise] = []
  arr2..push(() => println(async_id(1)))..push(() => println(async_id(2)))
  for f in arr2 {
    run_async(f)
  }
}
