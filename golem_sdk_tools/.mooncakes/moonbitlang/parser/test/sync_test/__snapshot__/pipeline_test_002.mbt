pub fn fib(n: Int) -> Int {
  if n == 0 || n == 1 { 1 } else { fib(n - 1) + fib(n - 2) }
}

fn init {

    // last line
    println(fib(10))

}

// last line
