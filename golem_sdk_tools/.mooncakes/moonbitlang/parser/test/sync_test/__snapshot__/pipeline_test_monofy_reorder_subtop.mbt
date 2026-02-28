///|
let b : ((Int, Int), Int) = (a, 1)

///|
fn g() -> Unit {
  let c = (a, b)
  println(c)
}

///|
let a : (Int, Int) = (3, 4)

///|
fn main {
  g()
}
