trait I {
  f(Self, x~ : Int, Int, y~ : Int) -> Unit
}

fn[X : Show] tee(x : X) -> X {
  println(x)
  x
}

struct T(Int) derive(Show)
impl I for T with f(self : T, x~ : Int, w : Int, y~ : Int) -> Unit {
  println("f(\{self}, x=\{x}, \{w}, y=\{y})")
}

fn init {
  let t = T(42)
  println("===")
  I::f(tee(t), x=tee(1), tee(2), y=tee(3))
  println("===")
  I::f(tee(t), tee(2), y=tee(3), x=tee(1))
}
