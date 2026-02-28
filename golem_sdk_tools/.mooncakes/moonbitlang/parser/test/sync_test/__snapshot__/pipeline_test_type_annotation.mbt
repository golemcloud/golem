fn[A] id (x: A) -> A {
  (x : A)
}

fn[A, B] map (xs: @list.List[A], f: (A) -> B) -> @list.List[B] {
  fn go(xs: @list.List[A]) -> @list.List[B] {
    match xs {
      @list.List::Empty => @list.new ()
      @list.List::More(x, tail=rest) => @list.cons(f(x), go(rest))
    }
  }

  go(xs)
}

fn[A] iter (xs: @list.List[A], f: (A) -> Unit) -> Unit {
  fn go(xs: @list.List[A]) -> Unit {
    match xs {
      @list.List::Empty => ()
      @list.List::More(x, tail=rest) => { f(x); go(rest) }
    }
  }

  go(xs)
}

fn init {
  iter(map(@list.cons(1, @list.cons(2, @list.cons(3, @list.empty()))), fn(x) { x + 1 }), fn (x) { println(x) })
}

