fn f(base: Int) -> Int {
  fn fib(n : Int) -> Int {
    match n {
      0 => base
      1 => 1
      _ => id(fib)(n - 1) + fib(n - 2)
    }
  }
  fib(10)
}

fn[T] id(x: T) -> T {
  x
}

fn init {
  println(f(1000))
}
