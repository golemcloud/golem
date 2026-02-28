fn f(x : Array[UInt?]) -> Unit {
  println(x[0])
}

fn main {
  f([Some(1), None, Some(3)])
}
