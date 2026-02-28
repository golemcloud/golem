///|
struct A {
  a : ReadOnlyArray[A]
}

///|
fn main {
  let a : ReadOnlyArray[A] = []
  print_view(a)
}

///|
fn print_view(x : ArrayView[A]) -> Unit {
  println(x.length())
}
