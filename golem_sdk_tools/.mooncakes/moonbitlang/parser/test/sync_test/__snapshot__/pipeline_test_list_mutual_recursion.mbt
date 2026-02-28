type List[T] = @list.List[T]

fn[T] keep_alternates(lst : List[T]) -> List[T] {
  match lst {
    @list.List::Empty => @list.empty()
    @list.List::More(x, tail=xs) => @list.cons(x, drop_alternates(xs))
  }
}

fn[T] drop_alternates(lst : List[T]) -> List[T] {
  match lst {
    @list.List::Empty => @list.empty()
    @list.List::More(_, tail=xs) => keep_alternates(xs)
  }
}

fn main {
  // @list.of([0, 2, 4, 6])
  println(keep_alternates(@list.from_array([0, 1, 2, 3, 4, 5, 6])))
  // @list.of(['a', 'c'])
  println(keep_alternates(@list.from_array(['a', 'b', 'c', 'd'])))
}
