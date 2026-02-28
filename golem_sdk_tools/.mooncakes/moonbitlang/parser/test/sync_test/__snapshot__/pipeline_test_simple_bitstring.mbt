//!build: -w -27

fn check_int(x: UInt, expected: UInt) -> Unit {
  if x == expected {
    println(".")
  } else {
    println("expected \{expected} but got \{x}")
  }
}

fn check_int64(x: UInt64, expected: UInt64) -> Unit {
  if x == expected {
    println(".")
  } else {
    println("expected \{expected} but got \{x}")
  }
}

fn f1(x: Bytes) -> Unit {
  match x {
    [u7(a), u15le(b), u23le(c), u40le(d), u3(e), u8(f), ..rest] => {
      check_int(a, 127)
      check_int(b, 129)
      check_int(c, 2097601)
      check_int64(d, 279726645696UL)
      check_int(e, 3)
      check_int(f, 12)
      println(rest)
    }
    _ => ()
  }
}

fn f2(x: Bytes) -> Unit {
  match x {
    [u7(a), u15be(b), u23be(c), u40be(d), u3(e), u8(f), ..rest] => {
      check_int(a, 127)
      check_int(b, 16512)
      check_int(c, 6324384)
      check_int64(d, 828408668481UL)
      check_int(e, 3)
      check_int(f, 12)
      println(rest)
    }
    _ => ()
  }
}

fn g1(x: Bytes) -> Unit {
  match x {
    [bytes(16, bs)] => {
      println(bs)
    }
    _ => ()
  }
}

fn g2(x: Bytes) -> Unit {
  match x {
    [bytes(8, [u8(0xFF), ..bs1]), bytes(8, [u8(0x00), ..bs2])] => {
      println("not ok")
      println(bs1)
      println(bs2)
    }
    [bytes(8, [u8(0xFF), ..bs1]), bytes(8, [u8(0x09), ..bs2])] => {
      println("ok")
      println(bs1)
      println(bs2)
    }
    _ => ()
  }
}

fn g3(x: Bytes) -> Unit {
  match x {
    [bytes(8, [u8(0xFF), ..bs1]), bytes(8, [u8(0x08), ..bs2])] => {
      println("not ok")
      println(bs1)
      println(bs2)
    }
    [bytes(7, [u8(0xFF), ..bs1]), bytes(9, [u8(0x0A), ..bs2])] => {
      println("ok")
      println(bs1)
      println(bs2)
    }
    _ => ()
  }
}

fn h(x: Bytes) -> Unit {
  match x {
    [u3(a), u5(b), ..] => {
      println(a)
      println(b)
    }
    _ => ()
  }
}

fn main {
  let bs = b"\xFF\x02\x03\x04\x05\x06\x07\x08\x09\x0A\x0B\x0C\x0D\x0E\x0F\x10"
  f1(bs)
  f2(bs)
  g1(bs)
  g2(bs)
  g3(bs)
  h(b"\xAA")
}
