fn init {
  println(fib(10))
}

fn fib(n: Int) -> Int {
  let acc1 = 0
  let acc2 = 1
  loop (n, acc1, acc2) {
    (0, res, _) => res
    (n, acc1, acc2) => continue (n-1, acc1+acc2, acc1)
  }
}


fn[T] iter(xs: @list.List[T], f: (T) -> Unit) -> Unit {
  loop xs {
    @list.List::More(x, tail=rest) => {
      f(x)
      continue rest
    }
    @list.List::Empty => break
  }
}

fn[T, S] reverse_map(xs: @list.List[T], f: (T) -> S) -> @list.List[S] {
  let acc = @list.empty()
  loop (xs, acc) {
    (@list.List::Empty, acc) => acc
    (@list.List::More(x, tail=rest), acc) => { continue (rest, @list.cons(f(x), acc)) }
  }
}

fn init {
  let xs = @list.cons(1, @list.cons(2, @list.cons(3, @list.empty())))
  let ys = reverse_map(xs, x => x + 1 )
  iter(ys, println)
}

fn test_break() -> Double {
  let i = 3.14
  let n = 10
  loop n {
    n => {
      if (n == 0) { break i }
      else { continue n - 2 }
    }
  }
}

fn test_tail(x: Int?) -> String {
  fn f() -> String { "Nothing" }
  let n = 0 // to break the letrec
  fn g(i: Int) -> String { i.to_string() }
  loop (x, n) {
    (None, _) => break f()
    (Some(i), acc) => if i == 0 { g(acc) } else { continue((Some(i-1)), acc+i) }
  }
}

fn test_not_tail(x: Int?) -> String {
  fn f() -> String { "Nothing" }
  let n = 0 // to break the letrec
  fn g(i: Int) -> String { i.to_string() }
  (loop (x, n) {
    (None, _) => break f()
    (Some(i), acc) => if i == 0 { g(acc) } else { continue((Some(i-1)), acc+i) }
  }) + "!"
}
fn test_return(x: Int?) -> String {
  let n = 0
  (loop (x, n) {
    (None, acc) => if acc == 0 { return "0" } else { break acc.to_string() }
    (Some(i), acc) => if i == 0 { break acc.to_string() } else { continue (Some(i-1), acc+i) }
  }) + "!"
}

fn test_capture(x: @list.List[Int]) -> @list.List[() -> String] {
  loop (x, @list.empty()) {
    (@list.List::Empty, acc) => acc
    (@list.List::More(x, tail=rest), acc) => continue (rest, @list.cons(fn () { x.to_string() } , acc))
  }
}

fn init {
  println(test_break())
  println(test_tail(None))
  println(test_tail(Some(10)))
  println(test_not_tail(None))
  println(test_not_tail(Some(10)))
  println(test_return(None))
  println(test_return(Some(10)))
  let fs = test_capture(@list.cons(1, @list.cons(2, @list.cons(3, @list.empty()))))
  loop fs {
    @list.List::Empty => ()
    @list.List::More(f, tail=rest) => { println(f()); continue rest }
  }
}
