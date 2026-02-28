fn bitand(x : UInt64, other : UInt64) -> UInt64 = "%u64.bitand"

fn bitor(x : UInt64, other : UInt64) -> UInt64 = "%u64.bitor"

fn bitxor(x : UInt64, other : UInt64) -> UInt64 = "%u64.bitxor"

fn bitnot(x : UInt64) -> UInt64 = "%u64.bitnot"

fn shr(x : UInt64, other : Int) -> UInt64 = "%u64.shr"

fn shl(x : UInt64, other : Int) -> UInt64 = "%u64.shl"

fn clz(x : UInt64) -> Int = "%u64.clz"

fn ctz(x : UInt64) -> Int = "%u64.ctz"

fn popcnt(x : UInt64) -> Int = "%u64.popcnt"

fn xtest_popcnt(x : UInt64, y : Int) -> Unit {
  if popcnt(x) == y {
    println(".")
  } else {
    println("x")
  }
}

fn xtest_ctz(x : UInt64, y : Int) -> Unit {
  if ctz(x) == y {
    println(".")
  } else {
    println("x")
  }
}

fn xtest_clz(x : UInt64, y : Int) -> Unit {
  if clz(x) == y {
    println(".")
  } else {
    println("x")
  }
}

fn xtest_not(x : Int64, y : Int64) -> Unit {
  if bitnot(x.reinterpret_as_uint64()) == y.reinterpret_as_uint64() {
    println(".")
  } else {
    println("x")
  }
}

fn xtest_and(x : Int64, y : Int64, z : Int64) -> Unit {
  if bitand(x.reinterpret_as_uint64(), y.reinterpret_as_uint64()) == z.reinterpret_as_uint64() {
    println(".")
  } else {
    println("x")
  }
}

fn xtest_or(x : Int64, y : Int64, z : Int64) -> Unit {
  if bitor(x.reinterpret_as_uint64(), y.reinterpret_as_uint64()) == z.reinterpret_as_uint64() {
    println(".")
  } else {
    println("x")
  }
}

fn xtest_xor(x : Int64, y : Int64, z : Int64) -> Unit {
  if bitxor(x.reinterpret_as_uint64(), y.reinterpret_as_uint64()) == z.reinterpret_as_uint64() {
    println(".")
  } else {
    println("x")
  }
}

fn xtest_shl(x : Int64, y : Int, z : Int64) -> Unit {
  if shl(x.reinterpret_as_uint64(), y) == z.reinterpret_as_uint64() {
    println(".")
  } else {
    println("x")
  }
}

fn xtest_shr(x : Int64, y : Int, z : Int64) -> Unit {
  if shr(x.reinterpret_as_uint64(), y) == z.reinterpret_as_uint64() {
    println(".")
  } else {
    println("x")
  }
}

fn main {
  println("===== testing and:")
  xtest_and(1L, 0L, 0L)
  xtest_and(0L, 1L, 0L)
  xtest_and(1L, 1L, 1L)
  xtest_and(0L, 0L, 0L)
  xtest_and(0x7fffffffffffffffL, 0x8000000000000000L, 0L)
  xtest_and(0x7fffffffffffffffL, -1L, 0x7fffffffffffffffL)
  xtest_and(0xf0f0ffffL, 0xfffff0f0L, 0xf0f0f0f0L)
  xtest_and(0xffffffffffffffffL, 0xffffffffffffffffL, 0xffffffffffffffffL)
  xtest_and(1L, 0L, 0L)
  xtest_and(0L, 1L, 0L)
  xtest_and(1L, 1L, 1L)
  xtest_and(0L, 0L, 0L)
  xtest_and(0x7fffffffffffffffL, 0x8000000000000000L, 0L)
  xtest_and(0x7fffffffffffffffL, -1L, 0x7fffffffffffffffL)
  xtest_and(0xf0f0ffffL, 0xfffff0f0L, 0xf0f0f0f0L)
  xtest_and(0xffffffffffffffffL, 0xffffffffffffffffL, 0xffffffffffffffffL)
  println("===== testing or:")
  xtest_or(1L, 0L, 1L)
  xtest_or(0L, 1L, 1L)
  xtest_or(1L, 1L, 1L)
  xtest_or(0L, 0L, 0L)
  xtest_or(0x7fffffffffffffffL, 0x8000000000000000L, -1L)
  xtest_or(0x8000000000000000L, 0L, 0x8000000000000000L)
  xtest_or(0xf0f0ffffL, 0xfffff0f0L, 0xffffffffL)
  xtest_or(0xffffffffffffffffL, 0xffffffffffffffffL, 0xffffffffffffffffL)
  xtest_or(1L, 0L, 1L)
  xtest_or(0L, 1L, 1L)
  xtest_or(1L, 1L, 1L)
  xtest_or(0L, 0L, 0L)
  xtest_or(0x7fffffffffffffffL, 0x8000000000000000L, -1L)
  xtest_or(0x8000000000000000L, 0L, 0x8000000000000000L)
  xtest_or(0xf0f0ffffL, 0xfffff0f0L, 0xffffffffL)
  xtest_or(0xffffffffffffffffL, 0xffffffffffffffffL, 0xffffffffffffffffL)
  println("===== testing xor:")
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
  println("===== testing shl, shr:")
  xtest_shl(1L, 1, 2L)
  xtest_shl(1L, 0, 1L)
  xtest_shl(0x7fffffffffffffffL, 1, 0xfffffffffffffffeL)
  xtest_shl(0xffffffffffffffffL, 1, 0xfffffffffffffffeL)
  xtest_shl(0x8000000000000000L, 1, 0L)
  xtest_shl(0x4000000000000000L, 1, 0x8000000000000000L)
  xtest_shl(1L, 63, 0x8000000000000000L)
  xtest_shl(1L, 64, 1L)
  xtest_shl(1L, 65, 2L)
  xtest_shl(1L, -1, 0x8000000000000000L)
  xtest_shl(1L, 0x7fffffff, 0x8000000000000000L)
  xtest_shr(1L, 1, 0L)
  xtest_shr(1L, 0, 1L)
  xtest_shr(-1L, 1, 0x7fffffffffffffffL)
  xtest_shr(0x7fffffffffffffffL, 1, 0x3fffffffffffffffL)
  xtest_shr(0x8000000000000000L, 1, 0x4000000000000000L)
  xtest_shr(0x4000000000000000L, 1, 0x2000000000000000L)
  xtest_shr(1L, 64, 1L)
  xtest_shr(1L, 65, 0L)
  xtest_shr(1L, -1, 0L)
  xtest_shr(1L, 0x7fffffff, 0L)
  xtest_shr(1L, 0x80000000, 1L)
  xtest_shr(0x8000000000000000L, 63, 1L)
  xtest_shr(-1L, 64, -1L)
  xtest_shr(-1L, 65, 0x7fffffffffffffffL)
  xtest_shr(-1L, -1, 1L)
  xtest_shr(-1L, 0x7fffffff, 1L)
  xtest_shr(-1L, 0x80000000, -1L)
  println("===== testing shift right (logical right shift): ")
  xtest_shr(1L, 1, 0L)
  xtest_shr(1L, 0, 1L)
  xtest_shr(-1L, 1, 0x7fffffffffffffffL)
  xtest_shr(0x7fffffffffffffffL, 1, 0x3fffffffffffffffL)
  xtest_shr(0x8000000000000000L, 1, 0x4000000000000000L)
  xtest_shr(0x4000000000000000L, 1, 0x2000000000000000L)
  xtest_shr(1L, 64, 1L)
  xtest_shr(1L, 65, 0L)
  xtest_shr(1L, -1, 0L)
  xtest_shr(1L, 0x7fffffff, 0L)
  xtest_shr(1L, 0x80000000, 1L)
  xtest_shr(0x8000000000000000L, 63, 1L)
  xtest_shr(-1L, 64, -1L)
  xtest_shr(-1L, 65, 0x7fffffffffffffffL)
  xtest_shr(-1L, -1, 1L)
  xtest_shr(-1L, 0x7fffffff, 1L)
  xtest_shr(-1L, 0x80000000, -1L)
  println("===== testing not:")
  xtest_not(0L, -1L)
  xtest_not(-1L, 0L)
  xtest_not(0x7fffffffffffffffL, 0x8000000000000000L)
  xtest_not(0x8000000000000000L, 0x7fffffffffffffffL)
  xtest_not(
    0b11110000111100001111111111111111L, 0b1111111111111111111111111111111100001111000011110000000000000000L,
  )
  xtest_not(0xffffffffffffffffL, 0L)
  println("===== testing clz:")
  xtest_clz(0UL, 64)
  xtest_clz(1UL, 63)
  xtest_clz(0x8000000000000000UL, 0)
  xtest_clz(0x7FFFFFFFFFFFFFFFUL, 1)
  xtest_clz(0x123456789ABCDEF0UL, 3)
  xtest_clz(0xABCDEF0123456789UL, 0)
  xtest_clz(0x00000000FFFFFFFFUL, 32)
  xtest_clz(0x0000000000000001UL, 63)
  xtest_clz(0x000000000000000FUL, 60)
  xtest_clz(0x00000000000000FFUL, 56)
  xtest_clz(0x0000000000000000UL, 64)
  xtest_clz(0xFFFFFFFFFFFFFFFFUL, 0)
  xtest_clz(0x0000000000000001UL, 63)
  xtest_clz(0x8000000000000000UL, 0)
  println("===== testing ctz:")
  xtest_ctz(0UL, 64)
  xtest_ctz(0xFFFFFFFFFFFFFFFFUL, 0)
  xtest_ctz(1UL, 0)
  xtest_ctz(0x8000000000000000UL, 63)
  xtest_ctz(0x0000000000000001UL, 0)
  xtest_ctz(0x0000000000000002UL, 1)
  xtest_ctz(0x0000000000000004UL, 2)
  xtest_ctz(0x0000000000000008UL, 3)
  xtest_ctz(0x0000000000000010UL, 4)
  xtest_ctz(0x0000000000000020UL, 5)
  xtest_ctz(0x0000000000000040UL, 6)
  xtest_ctz(0x0000000000000080UL, 7)
  xtest_ctz(0x0000000000000100UL, 8)
  xtest_ctz(0x0000000000000200UL, 9)
  xtest_ctz(0x0000000000000400UL, 10)
  xtest_ctz(0x0000000000000800UL, 11)
  xtest_ctz(0x0000000000001000UL, 12)
  xtest_ctz(0x0000000000002000UL, 13)
  xtest_ctz(0x0000000000004000UL, 14)
  xtest_ctz(0x0000000000008000UL, 15)
  xtest_ctz(0x0000000000010000UL, 16)
  xtest_ctz(0x0000000000020000UL, 17)
  xtest_ctz(0x0000000000040000UL, 18)
  xtest_ctz(0x0000000000080000UL, 19)
  xtest_ctz(0x0000000000100000UL, 20)
  xtest_ctz(0x0000000000200000UL, 21)
  xtest_ctz(0x0000000000400000UL, 22)
  xtest_ctz(0x0000000000800000UL, 23)
  xtest_ctz(0x0000000001000000UL, 24)
  xtest_ctz(0x0000000002000000UL, 25)
  xtest_ctz(0x0000000004000000UL, 26)
  xtest_ctz(0x0000000008000000UL, 27)
  xtest_ctz(0x0000000010000000UL, 28)
  xtest_ctz(0x0000000020000000UL, 29)
  xtest_ctz(0x0000000040000000UL, 30)
  xtest_ctz(0x0000000080000000UL, 31)
  xtest_ctz(0x0000000100000000UL, 32)
  xtest_ctz(0x0000000200000000UL, 33)
  xtest_ctz(0x0000000400000000UL, 34)
  xtest_ctz(0x0000000800000000UL, 35)
  xtest_ctz(0x0000001000000000UL, 36)
  xtest_ctz(0x0000002000000000UL, 37)
  xtest_ctz(0x0000004000000000UL, 38)
  xtest_ctz(0x0000008000000000UL, 39)
  xtest_ctz(0x0000010000000000UL, 40)
  xtest_ctz(0x0000020000000000UL, 41)
  xtest_ctz(0x0000040000000000UL, 42)
  xtest_ctz(0x0000080000000000UL, 43)
  xtest_ctz(0x0000100000000000UL, 44)
  xtest_ctz(0x0000200000000000UL, 45)
  xtest_ctz(0x0000400000000000UL, 46)
  xtest_ctz(0x0000800000000000UL, 47)
  xtest_ctz(0x0001000000000000UL, 48)
  xtest_ctz(0x0002000000000000UL, 49)
  xtest_ctz(0x0004000000000000UL, 50)
  xtest_ctz(0x0008000000000000UL, 51)
  xtest_ctz(0x0010000000000000UL, 52)
  xtest_ctz(0x0020000000000000UL, 53)
  xtest_ctz(0x0040000000000000UL, 54)
  xtest_ctz(0x0080000000000000UL, 55)
  xtest_ctz(0x0100000000000000UL, 56)
  xtest_ctz(0x0200000000000000UL, 57)
  xtest_ctz(0x0400000000000000UL, 58)
  xtest_ctz(0x0800000000000000UL, 59)
  xtest_ctz(0x1000000000000000UL, 60)
  xtest_ctz(0x2000000000000000UL, 61)
  xtest_ctz(0x4000000000000000UL, 62)
  xtest_ctz(0x8000000000000000UL, 63)
  println("===== testing popcnt:")
  xtest_popcnt(0UL, 0)
  xtest_popcnt(1UL, 1)
  xtest_popcnt(2UL, 1)
  xtest_popcnt(3UL, 2)
  xtest_popcnt(0xFFFFFFFFFFFFFFFFUL, 64)
  xtest_popcnt(0x8000000000000000UL, 1)
  xtest_popcnt(0x7FFFFFFFFFFFFFFFUL, 63)
  xtest_popcnt(0x123456789ABCDEF0UL, 32)
  xtest_popcnt(0xABCDEF0123456789UL, 32)
  xtest_popcnt(0x5555555555555555UL, 32)
  xtest_popcnt(0xAAAAAAAAAAAAAAAAUL, 32)
  xtest_popcnt(0xDEADBEEFDEADBEEFUL, 48)
  xtest_popcnt(0xBEEFDEADBEEFDEADUL, 48)
}
