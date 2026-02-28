fn f1(x : Int64) -> String {
  match x {
    0 => "zero"
    42 => "the ultimate answer"
    0x8000_0000_0000_0000 => "minimum"
    0xffff_ffff_ffff_ffff => "minus one"
    _ => "other"
  }
}

fn init {
  println(f1(0))
  println(f1(40L + 2))
  println(f1(0x8000_0000_0000_0000))
  println(f1(-1))
  println(f1(24))
}

fn f2(x : UInt64) -> String {
  match x {
    0 => "zero"
    42 => "the ultimate answer"
    0x8000_0000_0000_0000 => "minimum"
    0xffff_ffff_ffff_ffff => "minus one"
    _ => "other"
  }
}

fn init {
  println(f2(0))
  println(f2(40UL + 2))
  println(f2(0x8000_0000_0000_0000))
  println(f2(0xffff_ffff_ffff_ffff))
  println(f2(24))
}
