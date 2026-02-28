fn[T] has (xs: FixedArray[T], f: (T) -> Bool) -> Bool {
  let mut i = 0
  while i < xs.length() {
    if f(xs[i]) { return true }
    i = i + 1
  }
  return false
}

fn init {
  if has([1, 2, 3], fn(x) { x > 2 }) { println("ok") }
  if has([1, 2, 3], fn(x) { x > 4 }) { println("not ok") } else { println("ok") }
}

