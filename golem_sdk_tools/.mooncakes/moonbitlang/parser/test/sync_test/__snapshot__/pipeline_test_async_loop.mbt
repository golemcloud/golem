//!build: -w -67

async fn[T] suspend(f : ((T) -> Unit) -> Unit) -> T noraise = "%async.suspend"
fn run_async(f : async () -> Unit noraise) -> Unit = "%async.run"

async fn f() -> Int noraise {
  suspend(k => k(42))
}

suberror Err derive(Show)

// loop may transist to state after loop 
async fn f1() -> Unit noraise {
  let x = f()
  for i = 0; i < 10; i = i + 1 {
    let _ = f()
  }
  println(x)
}

// loop may perform early return
async fn f2() -> Unit raise Err {
  let x = f()
  for i = 0; i < 10; i = i + 1 {
    return
  }
  raise Err
}

// loop may raise error to caller
async fn f3() -> Unit raise Err {
  for i = 0; i < 10; i = i + 1 {
    raise Err
  }
}

// loop may raise error to try block
async fn f4() -> Unit noraise {
  let x = f()
  try {
    for i = 0; i < 10; i = i + 1 {
      raise Err
    }
  } catch {
    Err => println(x)
  }
}

// loop may break outer loop
async fn f5() -> Unit noraise {
  let x = f()
  outer~: for i = 0; i < 10; i = i + 1 {
    for j = 0; j < 10; j = j + 1 {
      break outer~
    }
    return
  }
  println(x)
}

// loop may continue outer loop
async fn f6() -> Unit noraise {
  let x = f()
  outer~: for i = 0; i < 2; i = i + 1 {
    println(x)
    for j = 0; j < 10; j = j + 1 {
      continue outer~
    }
  }
}

// [continue] may discard continuation and have a different type
async fn f7() -> Unit noraise {
  loop 0 {
    3..<_ => ()
    i => {
      if i == 0 {
        let x : String = { continue i + 2 }
        println(x)
      }
      continue i + 1
    }
  }
}

// [break] may discard continuation and have a different type
async fn f8() -> Unit noraise {
  loop 0 {
    3..<_ => ()
    i => {
      if i == 2 {
        let x : String = { break }
        println(x)
      }
      continue i + 1
    }
  }
}

// join point should not be treated as function
async fn f9(x : Int, y : Int) -> Unit noraise {
  outer~: for _ in 0..<2 {
    println(y)
    for _ in 0..<1 {
      ignore(f())
      match x {
        // the `or` pattern here creates a join point
        42 | 24 =>
          // when calculating the captures of the loop,
          // we must take the `continue` here in consideration.
          // otherwise `y` will be missed.
          // But this `continue` is inside a `fn` (which is in fact a join point)
          continue outer~
        _ => ()
      }
    }
    break
  }
}

fn main {
  letrec run = fn (f) {
    println("===")
    run_async(f)
  }
  and run_err = fn (f : async () -> Unit raise Err) {
    run(fn () {
      try f() catch { err => println(err) }
    })
  }
  run(f1)
  run_err(f2)
  run_err(f3)
  run(f4)
  run(f5)
  run(f6)
  run(f7)
  run(f8)
  run(() => f9(42, 42))
}
