struct F0(() -> Unit)

fn F0::apply(self : Self) -> Unit {
  self()
}

struct F2[X, Y]((X, Y) -> Unit)

fn[X, Y] F2::apply(self : F2[X, Y], x : X, y : Y) -> Unit {
  self(x, y)
}

fn main {
  let f0 : F0 = fn () { println("F0") }
  f0()
  f0.apply()
  let f2 : F2[_] = fn (x, y) { println((x, y)) }
  f2(4, "2")
  f2.apply(4, "2")
}
