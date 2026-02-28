enum Lst[X] {
  MNil
  MCons (X, Lst[X])
}

fn[X] Lst::length(self: Lst[X]) -> Int {
  match self {
    MNil => 0
    MCons(_, xs) => 1 + xs.length()
  }
}

fn[X] to_lst (xs: @list.List[X]) -> Lst[X] {
  match xs {
    @list.List::Empty => MNil
    @list.List::More(x, tail=xs) => MCons(x, to_lst(xs))
  }
}

fn[X] Lst::iter(self: Lst[X], f: (X) -> Unit) -> Unit {
  match self {
    MNil => ()
    MCons(x, xs) => { f(x); xs.iter(f) }
  }
}

fn[X, Y] Lst::map(self: Lst[X], f: (X) -> Y) -> Lst[Y] {
  match self {
    MNil => MNil
    MCons(x, xs) => MCons(f(x), xs.map(f))
  }
}

fn init {
  let xs : @list.List[_] = @list.cons(1, @list.cons(2, @list.cons(3, @list.empty())))
  let len = to_lst(xs).length()
  println("length = ")
  println(len)

  to_lst(xs).map(fn(x) { x * 3 }).iter(fn(x) { println(x) })
}

