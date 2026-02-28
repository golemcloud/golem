//!build: -w -27

fn xtest_clz(x: Int64, y: Int, loc~ : ArgsLoc = _ ) -> Unit {
  if (x.clz() == y) { println(".") } else { println("x \{loc}") }
}

fn xtest_ctz(x: Int64, y: Int, loc~ : ArgsLoc = _) -> Unit {
  if (x.ctz() == y) { println(".") } else { println("x \{loc}") }
}

fn xtest_popcnt(x: Int64, y: Int, loc~ : ArgsLoc = _) -> Unit {
  if (x.popcnt() == y) { println(".") } else { println("x \{loc}") }
}

fn init {
  xtest_clz ( 0xffffffffffffffffL, 0)
  xtest_clz ( 0L, 64)
  xtest_clz ( 0x00008000L, 48)
  xtest_clz ( 0xffL, 56)
  xtest_clz ( 0x8000000000000000L, 0)
  xtest_clz ( 1L, 63)
  xtest_clz ( 2L, 62)
  xtest_clz ( 0x7fffffffffffffffL, 1)

  xtest_ctz ( -1L, 0)
  xtest_ctz ( 0L, 64)
  xtest_ctz ( 0x00008000L, 15)
  xtest_ctz ( 0x00010000L, 16)
  xtest_ctz ( 0x8000000000000000L, 63)
  xtest_ctz ( 0x7fffffffffffffffL, 0)

  xtest_popcnt ( -1L, 64)
  xtest_popcnt ( 0L, 0)
  xtest_popcnt ( 0x00008000L, 1)
  xtest_popcnt ( 0x8000800080008000L, 4)
  xtest_popcnt ( 0x7fffffffffffffffL, 63)
  xtest_popcnt ( 0xAAAAAAAA55555555L, 32)
  xtest_popcnt ( 0x99999999AAAAAAAAL, 32)
  xtest_popcnt ( 0xDEADBEEFDEADBEEFL, 48)

}
