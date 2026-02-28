fn u64_to_int(x : UInt64) -> Int = "%u64.to_i32"
fn u64_to_uint(x : UInt64) -> UInt = "%u64.to_u32"

fn check(x : Bool) -> Unit {
  println(if x {"o"} else {"x"})
}

fn init {
  (UInt64::reinterpret_as_int64(100UL) == 100L) |> check()
  (UInt64::reinterpret_as_int64(9223372036854775807UL) == 9223372036854775807L) |> check()
  (UInt64::reinterpret_as_int64(18446744073709551615UL) == -1L) |> check()
  UInt64::equal(Int64::reinterpret_as_uint64(100L), 100UL) |> check()
  UInt64::equal(Int64::reinterpret_as_uint64(9223372036854775807L), 9223372036854775807UL) |> check()
  UInt64::equal(Int64::reinterpret_as_uint64(-1L), 18446744073709551615UL) |> check()
  
  (u64_to_uint(100UL) == 100U) |> check()
  (u64_to_uint(9223372036854775807UL) == 4294967295U) |> check()
  (u64_to_uint(18446744073709551615UL) == 4294967295U) |> check()
  (u64_to_int(100UL) == 100) |> check()
  (u64_to_int(9223372036854775807UL) == -1) |> check()
  (u64_to_int(18446744073709551615UL) == -1) |> check()

  let mut a = 0UL
  a = 18446744073709551615UL
  (UInt64::reinterpret_as_int64(a) == -1L) |> check()
}


