
fn init {
  loop x {
    _ => body
  }
  loop (x, y) {
    (1, 2) => body1
    (x, _) => body2
  }
  loop (1 + 2, 3 * 4) {
  }
  loop (x, {}) {
  }
  // syntax error
  loop (x,) {
  }
  loop 1 * * {
    _ => 1
  }
  f(1, loop 0 { x => x + 1 })
  loop 0 { x => x + 1 }
  |> f
}

fn[X] length(xs: List[X]) -> Int {
  loop (0, xs) {
    (acc, Nil) => acc
    (acc, Cons(_, tl)) => continue acc + 1, tl
  }
}

fn fib(n: Int) -> Int {
  loop (n, 0, 1) {
    (0, p2, _) => break p2
    (n, p2, p1) => continue n - 1, p1, p1 + p2
  }
}

fn f() {
  return(loop 0 {  })
}
