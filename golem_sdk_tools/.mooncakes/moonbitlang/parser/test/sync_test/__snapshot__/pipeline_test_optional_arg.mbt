
fn f(x~ : Int = tee(1), y~ : String = tee("y")) -> Unit {
  println("f(x=\{x}, y=\{y})")
}

fn init {
  println("===")
  f()
  println("===")
  f(x=2)
  println("===")
  f(y="?")
  println("===")
  f(x=2, y="?")
}


fn[X: Show] tee(x: X) -> X {
  println(x)
  x
}


trait I: Show {
  value() -> Self
}

impl I for Int with value() { 42 }
impl I for String with value() { "str" }

fn[X: I] poly(x~ : X = tee(X::value())) -> X {
  println(x)
  x
}

fn init {
  println("===")
  let _: Int = poly()
  println("===")
  let _: String = poly()
  println("===")
  let _ = poly(x=43)
  println("===")
  let _ = poly(x="?")
}
