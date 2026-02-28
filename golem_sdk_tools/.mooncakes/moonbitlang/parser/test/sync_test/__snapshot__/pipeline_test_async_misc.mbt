//!build: -w -67

fn run_async(f : async () -> Unit noraise) -> Unit = "%async.run"

async fn tuple() -> (Int, String) noraise {
  (1, "2")
}

fn init {
  run_async(fn () {
    println(tuple())
  })
}

async fn switch_constant(x : Int) -> Unit noraise {
  match x {
    42 => println("ans")
    _ => ()
  }
  println("after")
}

fn init {
  run_async(fn () { switch_constant(42) })
}

suberror Err

async fn join_let() -> Unit noraise {
  try {
    raise Err
  } catch {
    Err => ()
  }
  println("join let")
}

fn init {
  run_async(join_let)
}

async fn join_in_loop_without_async(input : Array[Int]) -> Unit noraise {
  for x in input {
    match x {
      4 | 2 => println("42")
      _ => println("other")
    }
  }
}

fn init {
  run_async(fn () { join_in_loop_without_async([ 1, 2, 3 ]) })
}
