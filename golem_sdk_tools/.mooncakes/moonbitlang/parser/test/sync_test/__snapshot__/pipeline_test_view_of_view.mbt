///|

fn[T : Hash] stable_hash(value : T) -> Int {
  let hasher = Hasher::new(seed = 42)
  hasher.combine(value)
  hasher.finalize()
}

fn f1(x : ArrayView[ArrayView[Int]]) -> Int {
  stable_hash(x)
}

///|
fn f2(x : ArrayView[ArrayView[Int]]) -> Unit {
  for i in x {
    for j in i {
      println(j)
    }
  }
}

///|
fn main {
  let arr : ArrayView[ArrayView[Int]] = [[1, 2, 3], [4, 5, 6]]
  println(f1(arr))
  f2(arr)
}
