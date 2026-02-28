struct R[X]((R[X]) -> X)

fn[X, Y] y(f : ((X) -> Y) -> (X) -> Y) -> (X) -> Y {
  fn ff (x: R[(X) -> Y]) -> (X) -> Y {
    fn(a) { f((x.0)(x))(a) }
  }
  ff(R::R(fn(x) { fn (a) { f((x.0)(x))(a) } }))
}

fn factx(f: ((Int) -> Int)) -> (Int) -> Int {
  fn(n: Int) -> Int {
    if n <= 1 { 1 } else { n * f(n-1)}
  }
}

fn init {
  let fact = y(factx)
  let n = fact(10)
  println(n)
}

