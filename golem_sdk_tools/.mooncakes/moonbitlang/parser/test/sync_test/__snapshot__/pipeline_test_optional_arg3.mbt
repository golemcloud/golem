fn opt(
  x~ : Int = 42,
  y~ : Int = x + 1
) -> Unit {
  println("x=\{x}, y=\{y}")
}

fn main {
  let some = x => Some(x)
  let none = None

  println("===")
  opt() // this currently cannot be inlined, due to default value of `y`
  opt(x?=none, y?=none)

  println("===")
  opt(x=41) // same here, cannot inline as long as `y` is not provided
  opt(x?=some(41))
  opt(x?=some(41), y?=none)

  println("===")
  opt(y=1) // this one can be inlined, as default value of `x` is constant
  opt(x?=none, y=1) // currently this one will be inlined too
  opt(y?=some(1))

  println("===")
  opt(x=1, y=2) // all arguments supplied, should be inlined
  opt(x?=some(1), y?=some(2))
}
