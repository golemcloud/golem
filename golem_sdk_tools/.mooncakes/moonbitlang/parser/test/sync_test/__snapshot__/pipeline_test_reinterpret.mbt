fn test1(x: Int64, y: Double) -> Unit {
  if (x.reinterpret_as_double() == y) { println(".") } else { println("x") }
}

fn test2(x: Double, y: Int64) -> Unit {
  if (x.reinterpret_as_int64() == y) { println(".") } else { println("x") }
}

fn init {
  test1 ( 0L, 0.0)
  test1 ( 1L, 0x0.0000000000001p-1022)
  test1 ( 0x8000000000000000L, -0.0)
  test1 ( 1234567890L, 0x0.00000499602d2p-1022)
  test1 ( -9223372036854775807L, -0x0.0000000000001p-1022)

  test2 ( 0.0, 0L)
  test2 ( -0.0, 0x8000000000000000L)
  test2 ( 0x0.0000000000001p-1022, 1L)
  test2 ( -0x0.0000000000001p-1022, 0x8000000000000001L)
  test2 ( 1.0, 4607182418800017408L)
  test2 ( 3.14159265358979, 4614256656552045841L)
  test2 ( 0x1.fffffffffffffp+1023, 9218868437227405311L)
  test2 ( -0x1.fffffffffffffp+1023, -4503599627370497L)

}
