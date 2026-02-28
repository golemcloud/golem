
trait T {
  f(Self) -> Int
}

struct F((F) -> Int)

impl T for F with f(self: F) -> Int {
  (self.0)(self)
}

fn use_t(t: &T) -> Int {
  t.f()
}

fn init {
  let f = F::F(fn (_x) { 42 })
  let a = use_t(f)
  println(a)
}
