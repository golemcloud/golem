suberror StringError {
  StringError(String)
} derive(Show)

fn f(x: Int) -> Int raise {
  return g(x)
}

fn g(x: Int) -> Int raise {
  if x > 0 { x } else { raise StringError("xxx") }
}

fn init {
  println((try! f(42)))
}
