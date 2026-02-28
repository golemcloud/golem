fn uint_to_int(x : UInt) -> Int = "%u32.to_i32_reinterpret"
fn int_to_uint(x : Int) -> UInt = "%i32.to_u32_reinterpret"
fn uint_eq(x : UInt, y : UInt) -> Bool = "%u32.eq"

fn check(x : Bool) -> Unit {
  println(if x {"o"} else {"x"})
}

fn init {
  (uint_to_int(100U) == 100) |> check()
  (uint_to_int(2147483647U) == 2147483647) |> check()
  (uint_to_int(4294967295U) == -1) |> check()
  uint_eq(int_to_uint(100), 100U) |> check()
  uint_eq(int_to_uint(2147483647), 2147483647U) |> check()
  uint_eq(int_to_uint(-1), 4294967295U) |> check()

  let mut a = 0U
  a = 4294967295U
  (uint_to_int(a) == -1) |> check()
}




