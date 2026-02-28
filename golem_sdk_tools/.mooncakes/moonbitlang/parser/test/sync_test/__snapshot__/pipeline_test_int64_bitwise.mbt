fn xtest(x: Int64, y: Int64) -> Unit {
  if (x == y) { println(".") } else { println("x") }
}

fn xtest_not(x: Int64, y: Int64) -> Unit {
  if (x.lnot() == y) { println(".") } else { println("x") }
}

fn xtest_and(x: Int64, y: Int64, z: Int64) -> Unit {
  if (x.land(y) == z) { println(".") } else { println("x") }
}

fn xtest_or(x: Int64, y: Int64, z: Int64) -> Unit {
  if (x.lor(y) == z) { println(".") } else { println("x") }
}

fn xtest_xor(x: Int64, y: Int64, z: Int64) -> Unit {
  if (x.lxor(y) == z) { println(".") } else { println("x") }
}

fn xtest_lsl(x: Int64, y: Int, z: Int64) -> Unit {
  if (x << y) == z { println(".") } else { println("x") }
}

fn xtest_lsr(x: Int64, y: Int, z: Int64) -> Unit {
  if (x.reinterpret_as_uint64() >> y).reinterpret_as_int64() == z { println(".") } else { println("x") }
}

fn xtest_asr(x: Int64, y: Int, z: Int64) -> Unit {
  if (x >> y == z) { println(".") } else { println("x") }
}

fn init {
  xtest_and(1L, 0L, 0L)
  xtest_and(0L, 1L, 0L)
  xtest_and(1L, 1L, 1L)
  xtest_and(0L, 0L, 0L)
  xtest_and(0x7fffffffffffffffL, 0x8000000000000000L, 0L)
  xtest_and(0x7fffffffffffffffL, -1L, 0x7fffffffffffffffL)
  xtest_and(0xf0f0ffffL, 0xfffff0f0L, 0xf0f0f0f0L)
  xtest_and(0xffffffffffffffffL, 0xffffffffffffffffL, 0xffffffffffffffffL)

  xtest(1L.land( 0L), 0L)
  xtest(0L.land( 1L), 0L)
  xtest(1L.land( 1L), 1L)
  xtest(0L.land( 0L), 0L)
  xtest(0x7fffffffffffffffL.land( 0x8000000000000000L), 0L)
  xtest(0x7fffffffffffffffL.land( -1L), 0x7fffffffffffffffL)
  xtest(0xf0f0ffffL.land( 0xfffff0f0L), 0xf0f0f0f0L)
  xtest(0xffffffffffffffffL.land( 0xffffffffffffffffL), 0xffffffffffffffffL)

  xtest_or(1L, 0L, 1L)
  xtest_or(0L, 1L, 1L)
  xtest_or(1L, 1L, 1L)
  xtest_or(0L, 0L, 0L)
  xtest_or(0x7fffffffffffffffL, 0x8000000000000000L, -1L)
  xtest_or(0x8000000000000000L, 0L, 0x8000000000000000L)
  xtest_or(0xf0f0ffffL, 0xfffff0f0L, 0xffffffffL)
  xtest_or(0xffffffffffffffffL, 0xffffffffffffffffL, 0xffffffffffffffffL)

  xtest(1L.lor( 0L), 1L)
  xtest(0L.lor( 1L), 1L)
  xtest(1L.lor( 1L), 1L)
  xtest(0L.lor( 0L), 0L)
  xtest(0x7fffffffffffffffL.lor( 0x8000000000000000L), -1L)
  xtest(0x8000000000000000L.lor( 0L), 0x8000000000000000L)
  xtest(0xf0f0ffffL.lor( 0xfffff0f0L), 0xffffffffL)
  xtest(0xffffffffffffffffL.lor( 0xffffffffffffffffL), 0xffffffffffffffffL)

  xtest_xor(1L, 0L, 1L)
  xtest_xor(0L, 1L, 1L)
  xtest_xor(1L, 1L, 0L)
  xtest_xor(0L, 0L, 0L)
  xtest_xor(0x7fffffffffffffffL, 0x8000000000000000L, -1L)
  xtest_xor(0x8000000000000000L, 0L, 0x8000000000000000L)
  xtest_xor(-1L, 0x8000000000000000L, 0x7fffffffffffffffL)
  xtest_xor(-1L, 0x7fffffffffffffffL, 0x8000000000000000L)
  xtest_xor(0xf0f0ffffL, 0xfffff0f0L, 0x0f0f0f0fL)
  xtest_xor(0xffffffffffffffffL, 0xffffffffffffffffL, 0L)

  xtest(1L.lxor( 0L), 1L)
  xtest(0L.lxor( 1L), 1L)
  xtest(1L.lxor( 1L), 0L)
  xtest(0L.lxor( 0L), 0L)
  xtest(0x7fffffffffffffffL.lxor( 0x8000000000000000L), -1L)
  xtest(0x8000000000000000L.lxor( 0L), 0x8000000000000000L)
  xtest((-1L).lxor( 0x8000000000000000L), 0x7fffffffffffffffL)
  xtest((-1L).lxor( 0x7fffffffffffffffL), 0x8000000000000000L)
  xtest(0xf0f0ffffL.lxor( 0xfffff0f0L), 0x0f0f0f0fL)
  xtest(0xffffffffffffffffL.lxor( 0xffffffffffffffffL), 0L)

  xtest_lsl( 1L, 1, 2L)
  xtest_lsl( 1L, 0, 1L)
  xtest_lsl( 0x7fffffffffffffffL, 1, 0xfffffffffffffffeL)
  xtest_lsl( 0xffffffffffffffffL, 1, 0xfffffffffffffffeL)
  xtest_lsl( 0x8000000000000000L, 1, 0L)
  xtest_lsl( 0x4000000000000000L, 1, 0x8000000000000000L)
  xtest_lsl( 1L, 63, 0x8000000000000000L)
  xtest_lsl( 1L, 64, 1L)
  xtest_lsl( 1L, 65, 2L)
  xtest_lsl( 1L, -1, 0x8000000000000000L)
  xtest_lsl( 1L, 0x7fffffff, 0x8000000000000000L)

  xtest(1L << ( 1), 2L)
  xtest(1L << ( 0), 1L)
  xtest(0x7fffffffffffffffL << ( 1), 0xfffffffffffffffeL)
  xtest(0xffffffffffffffffL << ( 1), 0xfffffffffffffffeL)
  xtest(0x8000000000000000L << ( 1), 0L)
  xtest(0x4000000000000000L << ( 1), 0x8000000000000000L)
  xtest(1L << ( 63), 0x8000000000000000L)
  xtest(1L << ( 64), 1L)
  xtest(1L << ( 65), 2L)
  xtest(1L << ( -1), 0x8000000000000000L)
  xtest(1L << ( 0x7fffffff), 0x8000000000000000L)

  xtest_asr(1L, 1, 0L)
  xtest_asr(1L, 0, 1L)
  xtest_asr(-1L, 1, -1L)
  xtest_asr(0x7fffffffffffffffL, 1, 0x3fffffffffffffffL)
  xtest_asr(0x8000000000000000L, 1, 0xc000000000000000L)
  xtest_asr(0x4000000000000000L, 1, 0x2000000000000000L)
  xtest_asr(1L, 64, 1L)
  xtest_asr(1L, 65, 0L)
  xtest_asr(1L, -1, 0L)
  xtest_asr(1L, 0x7fffffff, 0L)
  xtest_asr(1L, 0x80000000, 1L)
  xtest_asr(0x8000000000000000L, 63, -1L)
  xtest_asr(-1L, 64, -1L)
  xtest_asr(-1L, 65, -1L)
  xtest_asr(-1L, -1, -1L)
  xtest_asr(-1L, 0x7fffffff, -1L)
  xtest_asr(-1L, 0x80000000, -1L)

  xtest(1L >> ( 1), 0L)
  xtest(1L >> ( 0), 1L)
  xtest((-1L) >> ( 1), -1L)
  xtest(0x7fffffffffffffffL >> ( 1), 0x3fffffffffffffffL)
  xtest(0x8000000000000000L >> ( 1), 0xc000000000000000L)
  xtest(0x4000000000000000L >> ( 1), 0x2000000000000000L)
  xtest(1L >> ( 64), 1L)
  xtest(1L >> ( 65), 0L)
  xtest(1L >>  (-1), 0L)
  xtest(1L >> ( 0x7fffffff), 0L)
  xtest(1L >> ( 0x80000000), 1L)
  xtest(0x8000000000000000L >> ( 63), -1L)
  xtest((-1L) >> ( 64), -1L)
  xtest((-1L) >> ( 65), -1L)
  xtest((-1L) >> ( -1), -1L)
  xtest((-1L) >> ( 0x7fffffff), -1L)
  xtest((-1L) >> ( 0x80000000), -1L)

  xtest_lsr( 1L, 1, 0L)
  xtest_lsr( 1L, 0, 1L)
  xtest_lsr( -1L, 1, 0x7fffffffffffffffL)
  xtest_lsr( 0x7fffffffffffffffL, 1, 0x3fffffffffffffffL)
  xtest_lsr( 0x8000000000000000L, 1, 0x4000000000000000L)
  xtest_lsr( 0x4000000000000000L, 1, 0x2000000000000000L)
  xtest_lsr( 1L, 64, 1L)
  xtest_lsr( 1L, 65, 0L)
  xtest_lsr( 1L, -1, 0L)
  xtest_lsr( 1L, 0x7fffffff, 0L)
  xtest_lsr( 1L, 0x80000000, 1L)
  xtest_lsr( 0x8000000000000000L, 63, 1L)
  xtest_lsr( -1L, 64, -1L)
  xtest_lsr( -1L, 65, 0x7fffffffffffffffL)
  xtest_lsr( -1L, -1, 1L)
  xtest_lsr( -1L, 0x7fffffff, 1L)
  xtest_lsr( -1L, 0x80000000, -1L)

  xtest((1L.reinterpret_as_uint64() >> 1).reinterpret_as_int64(), 0L)
  xtest((1L.reinterpret_as_uint64() >> 0).reinterpret_as_int64(), 1L)
  xtest(((-1L).reinterpret_as_uint64() >> 1).reinterpret_as_int64(), 0x7fffffffffffffffL)
  xtest((0x7fffffffffffffffL.reinterpret_as_uint64() >> 1).reinterpret_as_int64(), 0x3fffffffffffffffL)
  xtest((0x8000000000000000L.reinterpret_as_uint64() >> 1).reinterpret_as_int64(), 0x4000000000000000L)
  xtest((0x4000000000000000L.reinterpret_as_uint64() >> 1).reinterpret_as_int64(), 0x2000000000000000L)
  xtest((1L.reinterpret_as_uint64() >> 64).reinterpret_as_int64(), 1L)
  xtest((1L.reinterpret_as_uint64() >> 65).reinterpret_as_int64(), 0L)
  xtest((1L.reinterpret_as_uint64() >> -1).reinterpret_as_int64(), 0L)
  xtest((1L.reinterpret_as_uint64() >> 0x7fffffff).reinterpret_as_int64(), 0L)
  xtest((1L.reinterpret_as_uint64() >> 0x80000000).reinterpret_as_int64(), 1L)
  xtest((0x8000000000000000L.reinterpret_as_uint64() >> 63).reinterpret_as_int64(), 1L)
  xtest(((-1L).reinterpret_as_uint64() >> 64).reinterpret_as_int64(), -1L)
  xtest(((-1L).reinterpret_as_uint64() >> 65).reinterpret_as_int64(), 0x7fffffffffffffffL)
  xtest(((-1L).reinterpret_as_uint64() >> -1).reinterpret_as_int64(), 1L)
  xtest(((-1L).reinterpret_as_uint64() >> 0x7fffffff).reinterpret_as_int64(), 1L)
  xtest(((-1L).reinterpret_as_uint64() >> 0x80000000).reinterpret_as_int64(), -1L)

  xtest_not(0L, -1L)
  xtest_not(-1L, 0L)
  xtest_not(0x7fffffffffffffffL, 0x8000000000000000L)
  xtest_not(0x8000000000000000L, 0x7fffffffffffffffL)
  xtest_not(0b11110000111100001111111111111111L, 0b1111111111111111111111111111111100001111000011110000000000000000L)
  xtest_not(0xffffffffffffffffL, 0L)
}
