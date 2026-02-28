suberror StringError {
  StringError(String)
}

// basic usage
fn f(x: Int) -> Int raise StringError {
  if x >= 10 {
    x
  } else {
    raise StringError("less than 10")
  }
}

fn g(x: Int) -> Int raise StringError {
  let a = f(x)
  a+10
}

fn init {
  println(try! f(42))
  println(try! g(2))
}
