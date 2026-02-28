
fn[T] null_out(arr: UninitializedArray[T], index: Int) -> Unit = "%fixedarray.set_null"
fn init {
  let xs: UninitializedArray[Option[Double]] = UninitializedArray::make(10)
  xs[1] = Some(3.0)
  println(xs[1])
  let _ = null_out(xs, 1)
}
