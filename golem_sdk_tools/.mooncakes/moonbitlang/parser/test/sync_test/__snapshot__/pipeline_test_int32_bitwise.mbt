//!build: -w -27

fn xtest_32_and(x : Int, y : Int, z : Int , loc~ : ArgsLoc = _ ) -> Unit {
  if x.land(y) == z {
    println(".")
  } else {
    println("x\{loc}")
  }
}

fn xtest_32_or(x : Int, y : Int, z : Int,  loc~ : ArgsLoc = _ ) -> Unit {
  if x.lor(y) == z {
    println(".")
  } else {
    println("x\{loc}")
  }
}

fn xtest_32_xor(x : Int, y : Int, z : Int,  loc~ : ArgsLoc = _ ) -> Unit {
  if x.lxor(y) == z {
    println(".")
  } else {
    println("x\{loc}")
  }
}

fn xtest_32_lsl(x : Int, y : Int, z : Int, loc~ : ArgsLoc = _ ) -> Unit {
  if (x << y) == z {
    println(".")
  } else {
    println("x\{loc}")
  }
}

fn xtest_32_lsr(x : Int, y : Int, z : Int, loc~ : ArgsLoc = _ ) -> Unit {
  if (x.reinterpret_as_uint() >> y).reinterpret_as_int() == z {
    println(".")
  } else {
    println("x\{loc}")
  }
}

fn xtest_32_asr(x : Int, y : Int, z : Int, loc~ : ArgsLoc = _ ) -> Unit {
  if (x >> y) == z {
    println(".")
  } else {
    println("x\{loc}")
  }
}

fn xtest_i32_not(x : Int, y : Int,  loc~ : ArgsLoc = _ ) -> Unit {
  if x.lnot() == y {
    println(".")
  } else {
    println("x\{loc}")
  }
}

fn xtest_i32_clz(x : Int, y : Int,  loc~ : ArgsLoc = _ ) -> Unit {
  if x.clz() == y {
    println(".")
  } else {
    println("x\{loc}")
  }
}

fn xtest_i32_ctz(x : Int, y : Int, loc~ : ArgsLoc = _ ) -> Unit {
  if x.ctz() == y {
    println(".")
  } else {
    println("x\{loc}")
  }
}

fn xtest_i32_popcnt(x : Int, y : Int, loc~ : ArgsLoc = _ ) -> Unit {
  if x.popcnt() == y {
    println(".")
  } else {
    println("x\{loc}")
  }
}

fn init {
  xtest_i32_not(0, -1)
  xtest_i32_not(-1, 0)
  xtest_i32_not(0x7fffffff, 0x80000000)
  xtest_i32_not(0x80000000, 0x7fffffff)
  xtest_i32_not(
    0b11110000111100001111111111111111, 0b1111000011110000000000000000,
  )
  xtest_i32_not(0xffffffff, 0)

  xtest_32_and(1, 0, 0)
  xtest_32_and(0, 1, 0)
  xtest_32_and(1, 1, 1)
  xtest_32_and(0, 0, 0)
  xtest_32_and(0x7fffffff, 0x80000000, 0)
  xtest_32_and(0x7fffffff, -1, 0x7fffffff)
  xtest_32_and(0xf0f0ffff, 0xfffff0f0, 0xf0f0f0f0)
  xtest_32_and(0xffffffff, 0xffffffff, 0xffffffff)

  xtest_32_or(1, 0, 1)
  xtest_32_or(0, 1, 1)
  xtest_32_or(1, 1, 1)
  xtest_32_or(0, 0, 0)
  xtest_32_or(0x7fffffff, 0x80000000, -1)
  xtest_32_or(0x80000000, 0, 0x80000000)
  xtest_32_or(0xf0f0ffff, 0xfffff0f0, 0xffffffff)
  xtest_32_or(0xffffffff, 0xffffffff, 0xffffffff)

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

  xtest_32_lsl(1, 1, 2)
  xtest_32_lsl(1, 0, 1)
  xtest_32_lsl(0x7fffffff, 1, 0xfffffffe)
  xtest_32_lsl(0xffffffff, 1, 0xfffffffe)
  xtest_32_lsl(0x80000000, 1, 0)
  xtest_32_lsl(0x40000000, 1, 0x80000000)
  xtest_32_lsl(1, 31, 0x80000000)
  xtest_32_lsl(1, 32, 1)
  xtest_32_lsl(1, 33, 2)
  xtest_32_lsl(1, -1, 0x80000000)
  xtest_32_lsl(1, 0x7fffffff, 0x80000000)

  xtest_32_asr(1, 1, 0)
  xtest_32_asr(1, 0, 1)
  xtest_32_asr(-1, 1, -1)
  xtest_32_asr(0x7fffffff, 1, 0x3fffffff)
  xtest_32_asr(0x80000000, 1, 0xc0000000)
  xtest_32_asr(0x40000000, 1, 0x20000000)
  xtest_32_asr(1, 32, 1)
  xtest_32_asr(1, 33, 0)
  xtest_32_asr(1, -1, 0)
  xtest_32_asr(1, 0x7fffffff, 0)
  xtest_32_asr(1, 0x80000000, 1)
  xtest_32_asr(0x80000000, 31, -1)
  xtest_32_asr(-1, 32, -1)
  xtest_32_asr(-1, 33, -1)
  xtest_32_asr(-1, -1, -1)
  xtest_32_asr(-1, 0x7fffffff, -1)
  xtest_32_asr(-1, 0x80000000, -1)

  xtest_i32_clz(0x0fffffff + 0x50000000 + 0x50000000 + 0x50000000, 0)
  xtest_i32_clz(0, 32)
  xtest_i32_clz(0x00008000, 16)
  xtest_i32_clz(0xff, 24)
  xtest_i32_clz(0x7FFFFFFF + 1, 0)
  xtest_i32_clz(1, 31)
  xtest_i32_clz(2, 30)
  xtest_i32_clz(0x7fffffff, 1)
  xtest_i32_ctz(-1, 0)
  xtest_i32_ctz(0, 32)
  xtest_i32_ctz(0x00008000, 15)
  xtest_i32_ctz(0x00010000, 16)
  xtest_i32_ctz(0x7FFFFFFF + 1, 31)
  xtest_i32_ctz(0x7fffffff, 0)

  xtest_i32_popcnt(-1, 32)
  xtest_i32_popcnt(0, 0)
  xtest_i32_popcnt(0x00008000, 1)
  xtest_i32_popcnt(0x8000 + 0x7FFFFFFF + 1, 2)
  xtest_i32_popcnt(0x7fffffff, 31)
  xtest_i32_popcnt(0x0AAAAAAA + 0x70000000 + 0x30000000, 16)
  xtest_i32_popcnt(0x55555555, 16)
  xtest_i32_popcnt(0x0EADBEEF + 0x70000000 + 0x60000000, 24)
}
