enum List[T] {
  Cons(Int, List[T])
  Nil
}

fn length_is_even(xs : List[T]) -> Bool {
  match xs {
    Nil => true
    Cons(_, Nil) => false
    Cons(_, Cons(_, rest)) => length_is_even(xs)
  }
}