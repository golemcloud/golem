fn[T] panic() -> T = "%panic"

fn[A] array_get_exn(xs: FixedArray[A], i: Int) -> A {
  if i >= xs.length() {
    panic()
  }
  xs[i]
}

fn init {
  let _ = array_get_exn([1, 2, 3], 4)
}
