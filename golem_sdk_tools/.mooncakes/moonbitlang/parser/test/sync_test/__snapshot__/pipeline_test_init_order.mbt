trait I {
  f(Self) -> Int
}

struct T(Int)

let a: Int = g(T(1))

pub impl I for T with f(self: T) -> Int {
  self.0 + b
}

pub fn[X:I] g(x: X) -> Int {
  x.f()
}

let b: Int = h(42)

pub fn h(x: Int) -> Int {
  x
}

fn main {
  println(a)
}
