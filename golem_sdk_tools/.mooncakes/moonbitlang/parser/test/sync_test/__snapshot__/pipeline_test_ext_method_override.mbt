
trait I {
  f(Self) -> Unit
}

fn[X: I] use_I(x: X) -> Unit {
  x.f()
}

struct T(Int)

fn T::f(self: T) -> Unit { println(self.0) }
impl I for T with f(x) { println(x.0 + 1) }

fn init {
  let t: T = T(42)
  t.f() // 42
  T::f(t) // 42
  use_I(t) // 43
}
