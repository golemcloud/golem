struct E(Int)

trait T {
  f(Self) -> E
}

impl[X:Show] T for FixedArray[X] with f(self) {
  println((self[0] as &Show).to_string())
  E::E(42)
}

fn use_t(t: &T) -> Int {
  let _ = t.f()
  42
}

fn init {
  let _ = use_t(([1]: FixedArray[_]))
}
