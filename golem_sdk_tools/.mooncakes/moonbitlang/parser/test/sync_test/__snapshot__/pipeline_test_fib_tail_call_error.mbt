

fn fib(n : Int) -> Int {
  fn aux(n : Int, a : Int, b : Int) -> Int raise {
    if n < 0 {
      raise (Failure::Failure ("negative argument"))
    }  
    if n == 0 {
      a
    } else {
      aux(n - 1, b, a + b)
    }
  }
  try! aux(n, 0, 1)
}

fn main {
  fib(5) |> println
}
