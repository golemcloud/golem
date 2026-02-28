
///|
fn f(xs : ArrayView[Int]) -> Unit {
  println(xs[1:])
}

///|
fn main {
  let arr : ReadOnlyArray[Int] = [1, 2, 3]
  println(arr[0])
  f(arr)
  f(arr[:2])
}
