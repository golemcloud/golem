fn fib2(n: Int) -> Int64 {
  fn aux(n: Int, m: Int, list: @list.List[Int64]) -> Int64 {
    match list {
      @list.List::More(hd, tail=@list.List::More(tl, ..)) => if n == m { hd + tl } else { aux(n, m + 1, @list.cons(hd + tl, list)) }
    }
  }
  if n == 1 || n == 2 { 1L } else { aux(n, 3, @list.cons(1L, @list.cons(1L, @list.empty()))) }

}

fn init {
  println(fib2(2))
}
