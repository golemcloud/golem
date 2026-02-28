
trait I {
  f(Self) -> Unit
}

fn[X: I] useI(x: X) -> Unit { x.f() }

impl I for Int with f(x) -> Unit { println(x) }

fn init {
  useI(42)
}
