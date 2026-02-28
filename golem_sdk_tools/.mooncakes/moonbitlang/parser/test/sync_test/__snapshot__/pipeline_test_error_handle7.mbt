suberror StringError {
  StringError(String)
} derive(Show)

fn f(x: Int) -> Int raise StringError {
  if x < 0 {
    raise StringError("error")
  }
  x
}

fn init {
  let a = try? f(42)
  println(a)
  let b = try? f(-42)
  println(b)
}

fn g(x: Int) -> Option[Int] raise StringError {
  if x < 0 {
    raise StringError("error")
  }
  Some(x)
}

