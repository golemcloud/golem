
fn[T] List::iter(self: List[T], f: (T) -> Unit) -> Unit {
  fn go(l: List[T]) {
    match l {
      Nil => ()
      Cons(x, xs) => { f(x); go(xs) }
    }
  }
  go(self)
}

fn[T] to_list(xs: FixedArray[T]) -> List[T] {
  fn go(i: Int) -> List[T] {
    if i == xs.length() { Nil } else { Cons(xs[i], go(i+1)) }
  }
  go(0)
}

fn init {
  to_list([1, 2, 3, 4, 5, 6]).iter(println)
}

enum List[T] {
  Nil
  Cons(T, List[T])
}
