fn init {
  let callbacks = loop (10, @list.empty()) {
    (0, acc) => acc
    (i, acc) => continue (i - 1, @list.cons(fn() { i }, acc))
  }
  loop callbacks {
    @list.List::Empty => ()
    @list.List::More(x, tail=rest) => {
      println(x())
      continue rest
    }
  }
}

fn init {
  loop 10 {
    0 => ()
    i => {
      let z = if i % 2 == 0 || i > 7 { continue i - 1 } else { i }
      println(i)
      continue z - 1
    }
  }
}
