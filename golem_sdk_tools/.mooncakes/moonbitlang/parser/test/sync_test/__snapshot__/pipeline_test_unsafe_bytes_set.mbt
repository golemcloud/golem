///|
fn[T] set(xs : FixedArray[T], i : Int, v : T) -> Unit = "%fixedarray.unsafe_set"

///|
fn f(x : FixedArray[Byte]) -> Unit {
  set(x, 0, 127)
}

///|
fn main {
  let x = ([1, 2, 3] : FixedArray[Byte])
  println(x)
  f(x)
  println(x)
}
