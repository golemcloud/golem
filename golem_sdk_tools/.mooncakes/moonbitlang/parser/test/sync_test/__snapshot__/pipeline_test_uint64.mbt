fn check(x : UInt64, r : UInt64) -> Unit {
  if (UInt64::equal(x, r)) {
    println("o")
  } else {
    println("x")
  }
}

fn check_true(x : Bool) -> Unit {
  if (x) {
    println("o")
  } else {
    println("x")
  }
}

let max : UInt64 = 18446744073709551615UL
let half : UInt64 = 0x8000000000000000UL

fn test_div() -> Unit {
  println("\nUInt64::op_div: ")
  UInt64::div(1UL, 1UL) |> check(1UL)
  UInt64::div(0UL, 1UL) |> check(0UL)
  UInt64::div(max, max) |> check(1UL)
  UInt64::div(half,max) |> check(1UL)
  UInt64::div(half,2UL) |> check(0x4000000000000000UL)
  UInt64::div(0x8ff00ff00ff00ff0UL, 0x100000001UL) |> check(0x8ff00fefUL)
  UInt64::div(0x8000000000000001UL,1000UL) |> check(0x20c49ba5e353f7UL)
  UInt64::div(5UL,2UL) |> check(2UL)
  UInt64::div(Int64::reinterpret_as_uint64(-5L), 2UL) |> check(0x7ffffffffffffffdUL)
  UInt64::div(5UL, Int64::reinterpret_as_uint64(-2L)) |> check(0UL)
  UInt64::div(Int64::reinterpret_as_uint64(-5L), Int64::reinterpret_as_uint64(-2L)) |> check(0UL)
  UInt64::div(7UL,3UL) |> check(2UL)
  UInt64::div(11UL,5UL) |> check(2UL)
  UInt64::div(17UL,7UL) |> check(2UL)
}

fn test_mod() -> Unit {
  println("\nUInt64::op_mod: ")
  UInt64::mod(1UL,1UL) |> check(0UL)
  UInt64::mod(0UL,1UL) |> check(0UL)
  UInt64::mod(max,max) |> check(0UL)
  UInt64::mod(half, max) |> check(half)
  UInt64::mod(half, 2UL) |> check(0UL)
  UInt64::mod(0x8ff00ff00ff00ff0UL, 0x100000001UL) |> check(0x80000001UL)
  UInt64::mod(0x8000000000000001UL,1000UL) |> check(809UL)
  UInt64::mod(5UL,2UL) |> check(1UL)
  UInt64::mod(Int64::reinterpret_as_uint64(-5L),2UL) |> check(1UL)
  UInt64::mod(5UL, Int64::reinterpret_as_uint64(-2L)) |> check(5UL)
  UInt64::mod(Int64::reinterpret_as_uint64(-5L),Int64::reinterpret_as_uint64(-2L)) |> check(Int64::reinterpret_as_uint64(-5L))
  UInt64::mod(7UL,3UL) |> check(1UL)
  UInt64::mod(11UL,5UL) |> check(1UL)
  UInt64::mod(17UL,7UL) |> check(3UL)
}

fn test_add_sub_mul() -> Unit {
  println("\ntest sub mul: ")
  UInt64::add(9223372036854775807UL, 1UL) |> check(9223372036854775808UL)
  UInt64::sub(0UL, 1UL) |> check(max)
  UInt64::sub(100UL, 10UL) |> check(90UL)
}

fn test_eq_ne() -> Unit {
  println("\ntest eq ne: ")
  check_true(UInt64::equal(0UL, 0UL))
  check_true(UInt64::equal(max, max))
  check_true(UInt64::equal(11UL, 11UL))
  check_true(not(UInt64::equal(0UL, max)))
  check_true(not(UInt64::equal(11UL, 5UL)))
}

fn test_compare() -> Unit {
  println("\ntest compare: ")
  check_true(UInt64::compare(0UL, max) == -1)
  check_true(UInt64::compare(max, 0UL) == 1)
  check_true(UInt64::compare(0UL, Int64::reinterpret_as_uint64(-2L)) == -1)
  check_true(UInt64::compare(1UL, 0UL) == 1)
  check_true(100UL < 101UL)
  check_true(100UL <= 100UL && 90UL <= 100UL)
  check_true(100UL >= 100UL && 100UL >= 90UL)
  check_true(100UL > 90UL)
}

fn init {
  test_eq_ne()
  test_compare()
  test_add_sub_mul()
  test_mod()
  test_div()
}

