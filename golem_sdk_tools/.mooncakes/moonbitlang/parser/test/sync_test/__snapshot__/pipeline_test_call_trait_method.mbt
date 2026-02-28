
trait I {
  f(Self) -> Unit
}

fn[X: I] use_I(x: X) -> Unit {
  x.f()
}


struct T(Int)

// regular method
fn T::f(self: T) -> Unit { println(self.0) }

// extension method
impl I for T with f(x) { println(x.0 + 1) }

fn init {
  let t: T = T(42)
  t.f() // 42
  T::f(t) // 42
  use_I(t) // 43
  I::f(t) // 43
  apply(I::f, t) // 43
}

fn[A, B] apply(f: (A) -> B, x: A) -> B {
  f(x)
}

fn[A, B, C] apply2(f: (A, B) -> C, x: A, y: B) -> C {
  f(x, y)
}

fn init {
  println(Compare::compare(42, 43)) // builtin implementation
  println(apply2(Compare::compare, 42, 43)) // unsaturated
}
