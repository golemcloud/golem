// construct @immut/list.T from Array.
fn[T] stream(xs: FixedArray[T]) -> @list.List[T] {
  fn go(idx: Int) -> @list.List[T] {
    if idx == xs.length() { @list.empty() } else { @list.cons(xs[idx], go(idx + 1)) }
  }

  go(0)
}

// length of @immut/list.T
fn[T] length(xs: @list.List[T]) -> Int {
  match xs {
    @list.List::Empty => 0
    @list.List::More(_, tail=xs) => 1 + length(xs)
  }
}

fn init {
  let xs: FixedArray[_] = [1,2,3]
  let ys: FixedArray[_] = ["xx","yy"]
  println(length(stream(xs)))
  println(length(stream(ys)))
}
