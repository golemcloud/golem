trait I {
  f(Self) -> Unit
  g(Self) -> Unit = _
}

impl I with g(self) {
  self.f()
}

struct No_param (Int) derive(Show)

impl I for No_param with f(self) {
  println(self)
}


struct More_param[X, Y] {
  x : X
  y : Y
} derive(Show)

impl[X : Show, Y : Show + Eq + Default] I for More_param[X, Y] with f(self) {
  let is_default = if self.y == Y::default() {
    "is default"
  } else {
    "is not default"
  }
  println("\{self} where y \{is_default}")
}

fn init {
  println("===")
  No_param::g(42)
  More_param::g({ x: "x", y: 42 })
  More_param::g({ x: "x", y: 0 })
}
