enum T {
  A(Double, Int)
  B(Double)
  C(Char)
}

pub fn g(x: T) -> Unit {
  match x {
    A(f, i) => { println(f); println(i) }
    B(f) => println(f)
    C(c) => println(c)
  }
}

fn init {
  g(A(8.28, 2023))
}
