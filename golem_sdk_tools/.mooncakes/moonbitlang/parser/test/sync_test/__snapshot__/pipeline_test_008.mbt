fn fib(n: Int) -> Int {
  match n {
    0 | 1 => 1
    _ => fib(n - 1) + fib(n - 2)
  }
}

fn init {
  println(fib(10))
}

