fn init{
  //test i32 -> i64 (i64.extend_i32_u)
  println("===== testing i32 -> i64:")
  fn f1(x : Int, e : Int64){
    let r = UInt64::extend_uint(x.reinterpret_as_uint()).reinterpret_as_int64()
    println("to_int64_u(\{x}) = \{r}, expected \{e}")
  }
  f1(0,0L)
  f1(10000,10000L)
  f1(-10000,0x00000000ffffd8f0L)
  f1(-1,0xffffffffL)
  f1(0x7fffffff, 0x000000007fffffffL)
  f1(0x7FFFFFFF+1, 0x0000000080000000L)

  //test i32 -> f64 (f64.convert_i32_u)
  println("===== testing i32 -> f64:")
  fn p2(x : Int, e : Double){
    let r = Double::convert_uint(x.reinterpret_as_uint())
    println("to_double_u(\{x})")
    // println("to_double_u(\{x}) = \{r}, expected \{e}")
  }
  p2(1,1.0)
  p2(0,0.0)
  p2(2147483647,2147483647.0)
  p2(-2147483648,2147483648.0)
  p2(0x0fffffff + 0x50000000 + 0x50000000 + 0x50000000,4294967295.0)

  //test i64 -> f64 (f64.convert_i64_u)
  println("===== testing i64 -> f64:")
  fn p3(x : Int64, e : Double){
    let r = Double::convert_uint64(x.reinterpret_as_uint64())
    println("to_double_u(\{x})")
    // println("to_double_u(\{x}) = \{r}, expected \{e}")
  }
  p3(1L,1.0)
  p3(0L,0.0)
  p3(9223372036854775807L,9223372036854775807.0)
  p3(-9223372036854775808L,9223372036854775808.0)
  p3(0xffffffffffffffffL,18446744073709551616.0)
  p3(0xfffffffffffff401L,0x1.fffffffffffffp+63)
  p3(0xfffffffffffff402L,0x1.fffffffffffffp+63)

  //test f64 -> i32 (i32.trunc_double_u)
  println("===== testing f64 -> i32:")
  fn p4(x : Double, e : Int){
    let r = UInt::trunc_double(x).reinterpret_as_int()
    println("to_int_u result \{r}, expected \{e}")
  }
  p4(4294967295.0, -1)
  p4(1.9,1)

  //test f64 -> i64 (i64.trunc_double_u)
  println("===== testing f64 -> i64:")
  fn p5(x : Double, e : Int64){
    let r = UInt64::trunc_double(x).reinterpret_as_int64()
    println("result \{r}, expected \{e}")
  }
  p5(9223372036854775808.0, -9223372036854775808L)
  p5(1.5,1L)
}
