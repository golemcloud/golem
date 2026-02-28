fn init {
  let xs = FixedArray::make(10, 100L)
  xs[0] = 123L
  println(xs[0])
  println(xs[1])
}

fn init {
  let xs: FixedArray[_] = [123L, 456L, 789L]
  xs[2] = 921L
  println(xs[1])
  println(xs[2])
}
