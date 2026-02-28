
fn print_bool(b: Bool) -> Unit {
  if b { println("O") } else { println("X") }
}

fn xtest_int_relation(f: (Int, Int) -> Bool) -> Unit {
  print_bool(f(1, 2))
  print_bool(f(1, 1))
  print_bool(f(2, 1))

}

fn xtest_int_predicate(f: (Int) -> Bool) -> Unit {
  print_bool(f(-1))
  print_bool(f(0))
  print_bool(f(1))

}


fn init {
  xtest_int_relation(fn (x, y) { x == y })
  xtest_int_relation(fn (x, y) { x < y })
  xtest_int_relation(fn (x, y) { x <= y })
  xtest_int_relation(fn (x, y) { x > y })
  xtest_int_relation(fn (x, y) { x >= y })

  xtest_int_predicate(fn (x) { x.is_pos() })
  xtest_int_predicate(fn (x) { x.is_neg() })
  xtest_int_predicate(fn (x) { x.is_non_pos() })
  xtest_int_predicate(fn (x) { x.is_non_neg() })

  print_bool((1 << 4) - 1 == 15)
  print_bool((3 << 1) == 6)
  print_bool((16U >> 4) == 1)
  print_bool((15U >> 2) == 3)
  print_bool((10000 >> 2) == 2500)
  print_bool((-10000 >> 3) == -1250)

}
