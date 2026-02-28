fn int_div_u(a : Int, b : Int, expected_result : Int) -> Unit {
  let r = a.reinterpret_as_uint().div(b.reinterpret_as_uint()).reinterpret_as_int()
  println("\{a} div_u \{b} = \{r}, expected \{expected_result}")
}

fn int_mod_u(a : Int, b : Int, expected_result : Int) -> Unit {
  let r = a.reinterpret_as_uint().mod(b.reinterpret_as_uint()).reinterpret_as_int()
  println("\{a} mod_u \{b} = \{r}, expected \{expected_result}")
}

fn xtest_div_u() -> Unit {
  println("tesing div_u:")
  // int64_div_u64(1,0) //divide by zero
  // int64_div_u64(0,0) //divide by zero
  int_div_u(1,1,1)
  int_div_u(0,1,0)
  int_div_u(-1,-1,1)
  int_div_u(0x7FFFFFFF+1, -1, 0)
  int_div_u(0x7FFFFFFF+1, 2, 0x40000000)
  int_div_u(0x7FFFFFFF+0x0ff00ff1, 0x10001, 0x8fef)
  int_div_u(0x7FFFFFFF+2, 1000, 0x20c49b)
  int_div_u(5, 2, 2)
  int_div_u(-5, 2, 0x7ffffffd)
  int_div_u(5, -2, 0)
  int_div_u(-5, -2, 0)
  int_div_u(7, 3, 2)
  int_div_u(11, 5, 2)
  int_div_u(17, 7, 2)
}

fn xtest_mod_u() -> Unit {
  println("tesing mod_u:")
  int_mod_u(1,1,0)
  int_mod_u(0,1,0)
  int_mod_u(-1,-1,0)
  int_mod_u(0x7FFFFFFF+1, -1, 0x7FFFFFFF+1)
  int_mod_u(0x7FFFFFFF+1, 2, 0)
  int_mod_u(0x7FFFFFFF+0x0ff00ff1, 0x10001, 0x8001)
  int_mod_u(0x7FFFFFFF+2, 1000, 649)
  int_mod_u(5, 2, 1)
  int_mod_u(-5, 2, 1)
  int_mod_u(5, -2, 5)
  int_mod_u(-5, -2, -5)
  int_mod_u(7, 3, 1)
  int_mod_u(11, 5, 1)
  int_mod_u(17, 7, 3)
}

fn init{
  xtest_div_u()

  xtest_mod_u()
}

