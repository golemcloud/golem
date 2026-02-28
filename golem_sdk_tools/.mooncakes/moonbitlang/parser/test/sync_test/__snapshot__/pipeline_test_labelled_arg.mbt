
fn f(p1 : Int, x~ : Int, p2 : Int, y~ : Int) -> Unit {
  println("f(\{p1}, ~x \{x}, \{p2}, ~y \{y})")
}

fn tee(x: Int) -> Int {
  println(x)
  x
}

fn init {
  println("===")
  f(tee(1), x=tee(2), tee(3), y=tee(4))
  f(tee(1), tee(2), x=tee(3), y=tee(4))
  f(x=tee(1), y=tee(2), tee(3), tee(4))
  f(y=tee(1), tee(2), tee(3), x=tee(4))
}

fn f2(x~ : Int, p1 : Int, y~: Int, p2 : Int) -> Unit {
  println("f2(~x \{x}, \{p1}, ~y \{y}, \{p2})")
}

fn init {
  println("===")
  tee(1) |> f2(tee(2), x=tee(3), y=tee(4))
  tee(1) |> f2(y=tee(2), x=tee(3), tee(4))
}
