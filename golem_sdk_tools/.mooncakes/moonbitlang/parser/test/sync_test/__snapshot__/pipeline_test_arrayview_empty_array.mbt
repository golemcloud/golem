///|
fn main {
  println(f([]))
  println(g([]))
}

///|
fn f(xs : Array[BytesView]) -> Array[BytesView] {
  xs.map(i => i)
}

///|
fn g(xs : Array[ArrayView[Int]]) -> Array[ArrayView[Int]] {
  xs.map(i => i)
}
