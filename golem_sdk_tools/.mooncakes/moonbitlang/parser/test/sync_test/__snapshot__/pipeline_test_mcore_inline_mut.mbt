///|
#locals(f)
fn p(arr : Array[Int], f : (Int) -> Int) -> Unit {
  let mut i = 0
  for a in arr {
    println(f(a + i))
    i = i + 1
  }
}

///|
fn main {
  p([1, 2, 3], x => x)
}
