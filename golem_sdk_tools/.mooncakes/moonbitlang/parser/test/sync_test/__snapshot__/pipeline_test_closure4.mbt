///|
fn g() -> Unit {
  fn f(_x) {
    ignore(f)
  }

  f(1)
}

///|
fn f1() -> Int {
  fn fib(n : Int) -> Int {
    match n {
      0 => 0
      1 => 1
      _ => id(fib)(n - 1) + fib(n - 2)
    }
  }

  fib(10)
}

///|
fn f2() -> Int {
  fn fib(n : Int) -> Int {
    match n {
      0 => 0
      1 => 1
      _ => fib(n - 1) + fib(n - 2)
    }
  }

  id(fib)(10)
}

///|
fn[T] id(x : T) -> T {
  x
}

///|

///|
fn main {
  g()
  println(f1())
  println(f2())
}
