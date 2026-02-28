
fn int64_div_u(a : Int64, b : Int64, expected_result : Int64) -> Unit {
  let r = a.reinterpret_as_uint64().div(b.reinterpret_as_uint64()).reinterpret_as_int64()
  println("\{a} div_u \{b} = \{r}, expected \{expected_result}")
}

fn int64_mod_u(a : Int64, b : Int64, expected_result : Int64) -> Unit {
  let r = a.reinterpret_as_uint64().mod(b.reinterpret_as_uint64()).reinterpret_as_int64()
  println("\{a} mod_u \{b} = \{r}, expected \{expected_result}")
}

fn xtest_div_u() -> Unit {
  println("tesing div_u:")
  // int64_div_u64(1,0) //divide by zero
  // int64_div_u64(0,0) //divide by zero
  int64_div_u(1L,1L,1L)
  int64_div_u(0L,1L,0L)
  int64_div_u(-1L,-1L,1L)
  int64_div_u(0x8000000000000000L, -1L, 0L)
  int64_div_u(0x8000000000000000L, 2L, 0x4000000000000000L)
  int64_div_u(0x8ff00ff00ff00ff0L, 0x100000001L, 0x8ff00fefL)
  int64_div_u(0x8000000000000001L, 1000L, 0x20c49ba5e353f7L)
  int64_div_u(5L, 2L, 2L)
  int64_div_u(-5L, 2L, 0x7ffffffffffffffdL)
  int64_div_u(5L, -2L, 0L)
  int64_div_u(-5L, -2L, 0L)
  int64_div_u(7L, 3L, 2L)
  int64_div_u(11L, 5L, 2L)
  int64_div_u(17L, 7L, 2L)
}

fn xtest_mod_u() -> Unit {
  int64_mod_u(1L,1L,0L)
  int64_mod_u(0L,1L,0L)
  int64_mod_u(-1L,-1L,0L)
  int64_mod_u(0x8000000000000000L,-1L,0x8000000000000000L)
  int64_mod_u(0x8000000000000000L,2L,0L)
  int64_mod_u(0x8ff00ff00ff00ff0L,0x100000001L,0x80000001L)
  int64_mod_u(0x8000000000000001L,1000L,809L)
  int64_mod_u(5L,2L,1L)
  int64_mod_u(-5L,2L,1L)
  int64_mod_u(5L,-2L,5L)
  int64_mod_u(-5L,-2L,-5L)
  int64_mod_u(7L,3L,1L)
  int64_mod_u(11L,5L,1L)
  int64_mod_u(17L,7L,3L)
}

fn init{
  xtest_div_u()

  xtest_mod_u()
}


fn init {
  println(Int64::default())
}
