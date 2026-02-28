fn bitand(x : UInt, other : UInt) -> UInt = "%u32.bitand"

fn bitor(x : UInt, other : UInt) -> UInt = "%u32.bitor"

fn bitxor(x : UInt, other : UInt) -> UInt = "%u32.bitxor"

fn bitnot(x : UInt) -> UInt = "%u32.bitnot"

fn shr(x : UInt, other : Int) -> UInt = "%u32.shr"

fn shl(x : UInt, other : Int) -> UInt = "%u32.shl"

fn clz(x : UInt) -> UInt = "%u32.clz"

fn ctz(x : UInt) -> UInt = "%u32.ctz"

fn popcnt(x : UInt) -> UInt = "%u32.popcnt"

fn xtest_32_not(x : Int, y : Int) -> Unit {
  if bitnot(x.reinterpret_as_uint()) == y.reinterpret_as_uint() {
    println(".")
  } else {
    println("x")
  }
}

fn xtest_32_and(x : Int, y : Int, z : Int) -> Unit {
  if bitand(x.reinterpret_as_uint(), y.reinterpret_as_uint()) ==
    z.reinterpret_as_uint() {
    println(".")
  } else {
    println("x")
  }
}

fn xtest_32_or(x : Int, y : Int, z : Int) -> Unit {
  if bitor(x.reinterpret_as_uint(), y.reinterpret_as_uint()) ==
    z.reinterpret_as_uint() {
    println(".")
  } else {
    println("x")
  }
}

fn xtest_32_xor(x : Int, y : Int, z : Int) -> Unit {
  if bitxor(x.reinterpret_as_uint(), y.reinterpret_as_uint()) ==
    z.reinterpret_as_uint() {
    println(".")
  } else {
    println("x")
  }
}

fn xtest_32_shr(x : Int, y : Int, z : Int) -> Unit {
  if shr(x.reinterpret_as_uint(), y) == z.reinterpret_as_uint() {
    println(".")
  } else {
    println("x")
  }
}

fn xtest_32_shl(x : Int, y : Int, z : Int) -> Unit {
  if shl(x.reinterpret_as_uint(), y) == z.reinterpret_as_uint() {
    println(".")
  } else {
    println("x")
  }
}

fn xtest_32_clz(x : UInt, y : UInt) -> Unit {
  if clz(x) == y {
    println(".")
  } else {
    println("x")
  }
}

fn xtest_32_ctz(x : UInt, y : UInt) -> Unit {
  if ctz(x) == y {
    println(".")
  } else {
    println("x")
  }
}

fn xtest_32_popcnt(x : UInt, y : UInt) -> Unit {
  if popcnt(x) == y {
    println(".")
  } else {
    println("x")
  }
}

fn main {
  println("===== testing not:")
  xtest_32_not(0, -1)
  xtest_32_not(-1, 0)
  xtest_32_not(0x7fffffff, 0x80000000)
  xtest_32_not(0x80000000, 0x7fffffff)
  xtest_32_not(
    0b11110000111100001111111111111111, 0b1111000011110000000000000000,
  )
  xtest_32_not(0xffffffff, 0)
  println("===== testing and:")
  xtest_32_and(1, 0, 0)
  xtest_32_and(0, 1, 0)
  xtest_32_and(1, 1, 1)
  xtest_32_and(0, 0, 0)
  xtest_32_and(0x7fffffff, 0x80000000, 0)
  xtest_32_and(0x7fffffff, -1, 0x7fffffff)
  xtest_32_and(0xf0f0ffff, 0xfffff0f0, 0xf0f0f0f0)
  xtest_32_and(0xffffffff, 0xffffffff, 0xffffffff)
  println("===== testing or:")
  xtest_32_or(1, 0, 1)
  xtest_32_or(0, 1, 1)
  xtest_32_or(1, 1, 1)
  xtest_32_or(0, 0, 0)
  xtest_32_or(0x7fffffff, 0x80000000, -1)
  xtest_32_or(0x80000000, 0, 0x80000000)
  xtest_32_or(0xf0f0ffff, 0xfffff0f0, 0xffffffff)
  xtest_32_or(0xffffffff, 0xffffffff, 0xffffffff)
  println("===== testing xor:")
  xtest_32_xor(1, 0, 1)
  xtest_32_xor(0, 1, 1)
  xtest_32_xor(1, 1, 0)
  xtest_32_xor(0, 0, 0)
  xtest_32_xor(0x7fffffff, 0x80000000, -1)
  xtest_32_xor(0x80000000, 0, 0x80000000)
  xtest_32_xor(-1, 0x80000000, 0x7fffffff)
  xtest_32_xor(-1, 0x7fffffff, 0x80000000)
  xtest_32_xor(0xf0f0ffff, 0xfffff0f0, 0x0f0f0f0f)
  xtest_32_xor(0xffffffff, 0xffffffff, 0)
  println("===== testing shift right (logical right shift): ")
  xtest_32_shr(1, 1, 0)
  xtest_32_shr(1, 0, 1)
  xtest_32_shr(-1, 1, 0x7fffffff)
  xtest_32_shr(0x7fffffff, 1, 0x3fffffff)
  xtest_32_shr(0x80000000, 1, 0x40000000)
  xtest_32_shr(0x40000000, 1, 0x20000000)
  xtest_32_shr(1, 32, 1)
  xtest_32_shr(1, 33, 0)
  xtest_32_shr(1, -1, 0)
  xtest_32_shr(1, 0x7fffffff, 0)
  xtest_32_shr(1, 0x80000000, 1)
  xtest_32_shr(0x80000000, 31, 1)
  xtest_32_shr(-1, 32, -1)
  xtest_32_shr(-1, 33, 0x7fffffff)
  xtest_32_shr(-1, -1, 1)
  xtest_32_shr(-1, 0x80000000, -1)
  println("===== testing shift left:")
  xtest_32_shl(1, 1, 2)
  xtest_32_shl(1, 0, 1)
  xtest_32_shl(-1, 1, -2)
  xtest_32_shl(0x7fffffff, 1, -2)
  xtest_32_shl(0x80000000, 1, 0)
  xtest_32_shl(0x40000000, 1, -2147483648)
  xtest_32_shl(1, 32, 1)
  xtest_32_shl(1, 33, 2)
  xtest_32_shl(1, -1, -2147483648)
  xtest_32_shl(1, 0x7fffffff, -2147483648)
  xtest_32_shl(1, 0x80000000, 1)
  xtest_32_shl(0x80000000, 31, 0)
  xtest_32_shl(-1, 32, -1)
  xtest_32_shl(-1, 33, -2)
  xtest_32_shl(-1, -1, -2147483648)
  xtest_32_shl(-1, 0x80000000, -1)
  println("===== testing clz:")
  xtest_32_clz(0x00000000U, 32)
  xtest_32_clz(0x80000000U, 0)
  xtest_32_clz(0x40000000U, 1)
  xtest_32_clz(0x20000000U, 2)
  xtest_32_clz(0x10000000U, 3)
  xtest_32_clz(0x12345678U, 3)
  xtest_32_clz(0x87654321U, 0)
  xtest_32_clz(0xABCDEF01U, 0)
  xtest_32_clz(0x0FEDCBA9U, 4)
  xtest_32_clz(0x0000FFFFU, 16)
  xtest_32_clz(0xFFFF0000U, 0)
  xtest_32_clz(0x00000001U, 31)
  xtest_32_clz(0x00000002U, 30)
  xtest_32_clz(0x00000004U, 29)
  xtest_32_clz(0x00000008U, 28)
  xtest_32_clz(0x00000010U, 27)
  xtest_32_clz(0x00000020U, 26)
  xtest_32_clz(0x00000040U, 25)
  xtest_32_clz(0x00000080U, 24)
  xtest_32_clz(0x00000100U, 23)
  xtest_32_clz(0x00000200U, 22)
  xtest_32_clz(0x00000400U, 21)
  xtest_32_clz(0x00000800U, 20)
  xtest_32_clz(0x00001000U, 19)
  xtest_32_clz(0x00002000U, 18)
  xtest_32_clz(0x00004000U, 17)
  xtest_32_clz(0x00008000U, 16)
  xtest_32_clz(0x00010000U, 15)
  xtest_32_clz(0x00020000U, 14)
  xtest_32_clz(0x00040000U, 13)
  xtest_32_clz(0x00080000U, 12)
  xtest_32_clz(0x00100000U, 11)
  xtest_32_clz(0x00200000U, 10)
  xtest_32_clz(0x00400000U, 9)
  xtest_32_clz(0x00800000U, 8)
  xtest_32_clz(0x01000000U, 7)
  xtest_32_clz(0x02000000U, 6)
  xtest_32_clz(0x04000000U, 5)
  xtest_32_clz(0x08000000U, 4)
  xtest_32_clz(0x10000000U, 3)
  xtest_32_clz(0x20000000U, 2)
  xtest_32_clz(0x40000000U, 1)
  xtest_32_clz(0x80000000U, 0)
}
