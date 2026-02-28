enum T {
  A (Int)
  B (Char)
}

struct R[A, B] {
  foo:(A) -> B
  bar:T
}

impl Show for T with output(self, logger) {
  match self {
    A(i) => logger.write_string(i.to_string())
    B(c) => logger.write_string(c.to_string())
  }
}

impl[A, B] Show for R[A, B] with output(self, logger) {
  logger.write_string("{ foo; bar = ")
  Show::output(self.bar, logger)
  logger.write_string("}")
}

fn init {
  let x : T = A(1)
  println(x)
  let y : T = B('c')
  println(y)
  let z = { foo:fn(x: Int) { x }, bar:y }
  println(z)
}

