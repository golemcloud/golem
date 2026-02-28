///|
fn[T] MutArrayView::buf(self : MutArrayView[T]) -> UninitializedArray[T] = "%arrayview.buf"

///|
fn[T] MutArrayView::start(self : MutArrayView[T]) -> Int = "%arrayview.start"

///|
fn[T] MutArrayView::make(
  buf : UninitializedArray[T],
  start : Int,
  len : Int,
) -> MutArrayView[T] = "%arrayview.make"

///|
fn[T] MutArrayView::my_mut_view(
  self : MutArrayView[T],
  start? : Int = 0,
  end? : Int,
) -> MutArrayView[T] {
  MutArrayView::make(
    {
      let a = self
      println("xxx")
      a
    }.buf(),
    {
      let a = self
      println("yyy")
      a
    }.start() +
    start,
    0,
  )
}

///|
fn main {
  let _ = [1, 2, 3].mut_view().my_mut_view()

}
