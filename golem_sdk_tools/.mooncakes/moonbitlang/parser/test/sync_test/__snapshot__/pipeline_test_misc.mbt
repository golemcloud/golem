fn listAdd(fs: @list.List[Int], gs: @list.List[Int]) -> @list.List[Int] {
  match (fs, gs) {
    (@list.List::Empty, fs) => fs
    (fs, @list.List::Empty) => fs
    (@list.List::More(f, tail=fs), @list.List::More(g, tail=gs)) => @list.cons(f + g, listAdd(fs, gs))
  }
}

fn listMul(fs: @list.List[Int], gs: @list.List[Int]) -> @list.List[Int] {
  match (fs, gs) {
    (@list.List::Empty, _) | (_, @list.List::Empty) => @list.empty()
    (@list.List::More(f, tail=fs), @list.List::More(g, tail=gs)) =>
      @list.cons(f * g, listAdd(listMul(@list.cons(f, @list.empty()), gs), listMul(fs, @list.cons(g, gs))))
  }
}

fn listPow(x: @list.List[Int], n: Int) -> @list.List[Int] {
  if n <= 1 { x } else { listMul(listPow(x, n - 1), x) }
}

fn pr(a: @list.List[Int]) -> Unit {
  match a {
    @list.List::Empty => ()
    @list.List::More(x, tail=xs) => { println(x); pr(xs) }
  }
}

fn init {
  let mut i = 1
  while i <= 6 {
    pr(listPow(@list.cons(1, @list.cons(1, @list.empty())), i))
    i = i + 1
  }
}

fn init {
  fn fib0(n: Int) {
    if n < 2 { n } else { fib0(n - 1) + fib0(n - 2) }
  }

  println(fib0(6))
  letrec fib=fn(n: Int) -> Int {
    if n < 2 { n } else { fic(n - 1) + fic(n - 2) }
  }

  and fic=fn(n: Int) -> Int {
    if n < 2 { n } else { fib(n - 1) + fib(n - 2) }
  }

  println(fib(7))
  let mut i = 0
  while i <= 10 {
    println(i)
    i = i + 1
  }
}

fn init {
  let (lo, hi) = (3, 7)
  let mut i = lo
  while i <= hi {
    println(i + lo + hi)
    i = i + 1
  }
}

fn init {
  match (2, 1) {
    (1, 2) => println(1)
    (_, _) => println(2)
  }
}

enum H1 {
  A (Int)
  B (Int)
}

fn init {
  let f = fn(x: (H1, H1)) {
    match x {
      (A(x), A(y)) => println(x + y)
      (A(x), B(y)) => println(y - x)
      (B(x), A(y)) => println(3)
      (B(x), B(y)) => println(4)
    }
  }
  f((A(3), A(4)))
  f((A(3), B(4)))
}

enum H2[X, Y] {
  A1
  D
  E (X)
  F (Y)
}

fn init {
  let h = fn(a) {
    match a {
      (1, H2::D) => { println(1); println('d') }
      (_, E((x:Int))) => { println(x); println('e') }
      (_, F((c:Char))) => { println(c); println('f') }
    }
  }
  h((1, D))
  h((0, E(2)))
  h((3, F('x')))
}

fn init {
  let k1 = fn(a, b) { a }
  let k2 = fn(a, b) { b }
  println(k1(3, 4) + k2(5, 6))
}

fn init {
  println(1)
}

fn init {
  let (a, b) = (3, 4)
  println(a + b)
}

