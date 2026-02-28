
fn u32_add(x : UInt, other : UInt) -> UInt = "%u32.add"
fn u32_sub(x : UInt, other : UInt) -> UInt = "%u32.sub"
fn u32_mul(x : UInt, other : UInt) -> UInt = "%u32.mul"
fn u32_div(x : UInt, other : UInt) -> UInt = "%u32.div"
fn u32_mod(x : UInt, other : UInt) -> UInt = "%u32.mod"
fn u32_eq(x : UInt, other : UInt) -> Bool = "%u32.eq"
fn u32_ne(x : UInt, other : UInt) -> Bool = "%u32.ne"
fn u32_compare(x : UInt, other : UInt) -> Int = "%u32.compare"

fn uint_to_int(x : UInt) -> Int = "%u32.to_i32_reinterpret"
fn int_to_uint(x : Int) -> UInt = "%i32.to_u32_reinterpret"

fn check(x : UInt, r : UInt) -> Unit {
  if (u32_eq(x, r)) {
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

let max : UInt = 4294967295U

fn test_div() -> Unit {
  println("\nu32_div: ")
  u32_div(1U,1U) |> check(1U)
  u32_div(0U,1U) |> check(0U)
  u32_div(max,max) |> check(1U)
  u32_div(0x80000000U,max) |> check(0U)
  u32_div(0x80000000U,2U) |> check(0x40000000U)
  u32_div(0x8ff00ff0U,0x10001U) |> check(0x8fefU)
  u32_div(0x80000001U,1000U) |> check(0x20c49bU)
  u32_div(5U,2U) |> check(2U)
  u32_div(int_to_uint(-5),2U) |> check(0x7ffffffdU)
  u32_div(5U,int_to_uint(-2)) |> check(0U)
  u32_div(int_to_uint(-5),int_to_uint(-2)) |> check(0U)
  u32_div(7U,3U) |> check(2U)
  u32_div(11U,5U) |> check(2U)
  u32_div(17U,7U) |> check(2U)
}

fn test_mod() -> Unit {
  println("\nu32_mod: ")
  u32_mod(1U,1U) |> check(0U)
  u32_mod(0U,1U) |> check(0U)
  u32_mod(max,max) |> check(0U)
  u32_mod(0x80000000U,max) |> check(0x80000000U)
  u32_mod(0x80000000U,2U) |> check(0U)
  u32_mod(0x8ff00ff0U,0x10001U) |> check(0x8001U)
  u32_mod(0x80000001U,1000U) |> check(649U)
  u32_mod(5U,2U) |> check(1U)
  u32_mod(int_to_uint(-5),2U) |> check(1U)
  u32_mod(5U,int_to_uint(-2)) |> check(5U)
  u32_mod(int_to_uint(-5),int_to_uint(-2)) |> check(int_to_uint(-5))
  u32_mod(7U,3U) |> check(1U)
  u32_mod(11U,5U) |> check(1U)
  u32_mod(17U,7U) |> check(3U)
}

fn test_add_sub_mul() -> Unit {
  println("\ntest sub mul: ")
  u32_add(5U, 10U) |> check(15U)
  u32_sub(10U, 5U) |> check(5U)
  u32_mul(10U, 5U) |> check(50U)
  u32_mul(1000000000U, 4U) |> check(4000000000U)
}

fn test_eq_ne() -> Unit {
  println("\ntest eq ne: ")
  check_true(u32_eq(0U,0U))
  check_true(u32_eq(max,max))
  check_true(u32_eq(11U,11U))
  check_true(u32_ne(0U,max))
  check_true(u32_ne(11U,5U))
}

fn test_compare() -> Unit {
  println("\ntest compare: ")
  check_true(u32_compare(0U,max) == -1)
  check_true(u32_compare(max,0U) == 1)
  check_true(u32_compare(0U,int_to_uint(-2)) == -1)
  check_true(u32_compare(1U,0U) == 1)
  check_true(100U < 101U)
  check_true(100U <= 100U && 90U <= 100U)
  check_true(100U >= 100U && 100U >= 90U)
  check_true(100U > 90U)
  check_true(0xDABA0600U > 0)
}

fn init {
  test_eq_ne()
  test_compare()
  test_add_sub_mul()
  test_mod()
  test_div()
}
