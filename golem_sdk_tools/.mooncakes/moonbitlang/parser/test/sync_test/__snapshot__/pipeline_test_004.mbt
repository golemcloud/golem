enum H[M, N] {
  A
  D
  E (M)
  F (N)
}

fn init {
  let f = fn(x) {
    match x {
      H::A => println('a')
      E((x:Int)) => println(x)
      F(c) => println('f')
    }
  }
  f(E(1))
  f(A)
}

