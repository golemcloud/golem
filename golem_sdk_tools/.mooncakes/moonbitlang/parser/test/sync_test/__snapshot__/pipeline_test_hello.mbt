fn iter(l: @list.List[Int], f: (Int) -> Unit) -> Unit {
  match l {
    @list.List::Empty => ()
    @list.List::More(x, tail=xs) => { f(x); iter(xs, f) }
  }
}

fn map(l: @list.List[Int], f: (Int) -> Int) -> @list.List[Int] {
  match l {
    @list.List::Empty => @list.new()
    @list.List::More(x, tail=xs) => @list.cons(f(x), map(xs, f))
  }
}

fn hello(a: Int, b: Int) -> Int {
  a + b
}

fn init {
  println(hello(1, 2))
  let data : @list.List[Int] = @list.cons(1, @list.cons(2, @list.cons(3, @list.cons(4, @list.new()))))
  iter(map(data, fn(x) { x + 1 }), (fn (x) { println(x) }))
}

