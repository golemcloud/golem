fn f(x : Int) -> String raise Error {
  if x == 42 {
    fail("!!")
  }
  x.to_string()
}

fn g(x : Int) -> String raise Error {
  match x {
    0 | 1 => return f(0)
    2 | 3 => return f(1)
    x => x.to_string()
  }
}

fn init {
  println(try? g(42))
}
