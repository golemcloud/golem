fn compare_int_u(a : Int, b : Int) -> Unit {
  println(a.reinterpret_as_uint().compare(b.reinterpret_as_uint()))
}

fn compare_int64_u(a : Int64, b : Int64) -> Unit{
  println(a.reinterpret_as_uint64().compare(b.reinterpret_as_uint64()))
}

fn xtest_int() -> Unit {
  println("test unsigned operations for int")
  compare_int_u(0,0) //0
  compare_int_u(1,1) //0
  compare_int_u(-1,1) //1
  compare_int_u(0x7FFFFFFF+1,0x7FFFFFFF+1) //0
  compare_int_u(0x7fffffff,0x7fffffff) //0
  compare_int_u(-1,-1)//0
  compare_int_u(1,0)//1
  compare_int_u(0,1)//-1
  compare_int_u(0x7FFFFFFF+1, 0) //1
  compare_int_u(0, 0x7FFFFFFF+1) //-1
  compare_int_u(0x7FFFFFFF+1, -1) //-1
  compare_int_u(-1, 0x7FFFFFFF+1) //1
  compare_int_u(0x7FFFFFFF+1,0x7fffffff) //1
  compare_int_u(0x7fffffff,0x7FFFFFFF+1) //-1
}

fn xtest_int64() -> Unit {
  println("test unsigned compare for int64")
  compare_int64_u(0L,0L) //0
  compare_int64_u(1L,1L) //0
  compare_int64_u(-1L,1L) //1
  compare_int64_u(0x8000000000000000L,0x8000000000000000L) //0
  compare_int64_u(0x7fffffffffffffffL,0x7fffffffffffffffL) //0
  compare_int64_u(-1L,-1L)//0
  compare_int64_u(1L,0L)//1
  compare_int64_u(0L,1L)//-1
  compare_int64_u(0x8000000000000000L, 0L) //1
  compare_int64_u(0L, 0x8000000000000000L) //-1
  compare_int64_u(0x8000000000000000L, -1L) //-1
  compare_int64_u(-1L, 0x8000000000000000L) //1
  compare_int64_u(0x8000000000000000L,0x7fffffffffffffffL) //1
  compare_int64_u(0x7fffffffffffffffL,0x8000000000000000L) //-1
}

fn init{
  xtest_int()
  xtest_int64()
}

