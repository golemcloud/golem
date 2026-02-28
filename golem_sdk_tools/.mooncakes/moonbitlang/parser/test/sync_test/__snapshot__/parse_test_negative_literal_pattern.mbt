fn fib(n : Int) -> Int {
  match n {
    -1 => 0
    0 => 0
    1 => 1
    _ => fib(n - 1) + fib(n - 2)
  }
}


fn init{
  let n = 3.14
  match n {
    -3.14 => ()
    3.14 => ()
    _ => ()
  }
}