#intrinsic("%fixedarray.fill")
fn[T] array_fill(arr : FixedArray[T], d : Int, val : T, n : Int) -> Unit {
  for i = 0; i < n; i = i + 1 {
    arr[i + d] = val
  }
}

fn init {
  let a : FixedArray[Int] = [1, 2, 3]
  array_fill(a, 1, 0, 2)
  println(a)
}

fn init {
  let a : FixedArray[Ref[Int]] = [{ val: 1 }, { val: 2 }, { val: 3 }]
  array_fill(a, 1, { val: 0 }, 2)
  println(a)
}
