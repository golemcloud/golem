fn sum(x: FixedArray[Int]) -> Int {
  let mut u = 0
  let mut i = 0
  while i < x.length() {
    u = u + x[i]
    i = i + 1
  }
  u
}

fn init {
  println(sum([1, 2, 3, 4, 5]))
}

fn[X] iter (xs: @list.List[X], f: (X) -> Unit) -> Unit {
  match xs {
    @list.List::Empty => ()
    @list.List::More(x, tail=rest) => { f(x); iter(rest, f) }
  }
}

fn sum_list(xs: @list.List[Int]) -> Int {
  let mut total = 0
  iter(xs, fn(x) { total = total + x })
  total
}

fn init {
  println(sum_list(@list.cons(1, @list.cons(2, @list.cons(3, @list.cons(4, @list.cons(5, @list.empty())))))))
}

fn[X] rev (xs: @list.List[X]) -> @list.List[X] {
  let mut acc : @list.List[_] = @list.empty()
  iter(xs, fn(x) { acc = @list.cons(x, acc) })
  acc
}

fn init {
  iter(rev(@list.cons(1, @list.cons(2, @list.cons(3, @list.empty())))), fn (x) { println(x) })
}

fn init {
  let mut id = fn(x) { x }
  id = fn(x: Int) { x + 1 }
  if id(0) == 1 { println("\nok\n") } else { println("\nnot ok\n") }
  id = fn(x) { x + 2 }
  if id(0) == 2 { println("ok\n") } else { println("not ok\n") }
}

fn init {
  let mut x = 3
  match x {
    3 => println("ok")
    _ => println("not ok")
  }
}

// toplevel mutable
let str_buf : Ref[() -> Unit] = { val: fn() { println("") } }
fn replace_str(s: String) -> Unit {
  (str_buf.val)()
  str_buf.val = fn() { println(s) }
}

fn flush_str() -> Unit {
  (str_buf.val)()
}

fn init {
  replace_str("1.ok!\n")
  replace_str("2.ok!\n")
  flush_str()
}
