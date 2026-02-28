///|
fn[T] f(xs : ArrayView[T]) -> T {
  xs[0]
}

///|
let a : Array[Int] = [1, 2, 3]

///|
fn main {
  println(f(a))
  println(f(a[1:3]))
  println((f(["x", "y", "z"]) : Bytes))
  println(f(["x", "y", "z"]))
}
