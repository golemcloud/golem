async fn[T] suspend(f : ((T) -> Unit) -> Unit) -> T noraise = "%async.suspend"
fn run_async(f : async () -> Unit noraise) -> Unit = "%async.run"

fn[X : Show] tee(x : X) -> X {
  println(x)
  x
}

let state : Ref[Int] = { val: 0 }

fn[X : Show, Y : Show, Z : Show] print3(x : X, y : Y, z : Z) -> Unit {
  println("f(\{x}, \{y}, \{z})")
}

async fn eval_order() -> Unit noraise {
  print3(tee(1), tee(2), suspend(fn (k) {
    println("enter suspendsion")
    k(tee(3))
    println("after suspension")
  }))
}

fn init {
  run_async(eval_order)
}

suberror Err

fn sync_with_err() -> Int raise {
  if state.val == 42 {
    raise Err
  }
  state.val
}

async fn err_in_pipe_lhs() -> Int raise {
  sync_with_err() |> println
  suspend(fn (k) {
    k(42)
  })
}

fn init {
  run_async(fn () {
    try println(err_in_pipe_lhs()) catch { _ => panic() }
  })
}
