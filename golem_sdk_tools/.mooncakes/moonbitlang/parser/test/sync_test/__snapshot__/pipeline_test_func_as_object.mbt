trait T {
  f(Self) -> Int
}

struct F((Int) -> Int)

impl T for F with f(self: F) -> Int {
  (self.0)(42)
}

fn use_t(t: &T) -> Int {
  t.f()
}

fn init {
  let f = F::F(fn (x) { x })
  let a = use_t(f)
  println(a)
}
