///|
fn i16_to_int(x : Int16) -> Int = "%i16_to_i32"

///|
fn i16_to_byte(x : Int16) -> Byte = "%i16_to_byte"

///|
fn int_to_i16(x : Int) -> Int16 = "%i32_to_i16"

///|
fn byte_to_i16(x : Byte) -> Int16 = "%byte_to_i16"

///|
fn u16_to_int(x : UInt16) -> Int = "%u16_to_i32"

///|
fn u16_to_byte(x : UInt16) -> Byte = "%u16_to_byte"

///|
fn int_to_u16(x : Int) -> UInt16 = "%i32_to_u16"

///|
fn byte_to_u16(x : Byte) -> UInt16 = "%byte_to_u16"

///|
fn print_u16(s : String, x : UInt16) -> Unit {
  println("\{s}: \{u16_to_int(x)}")
}

///|
fn print_i16(s : String, x : Int16) -> Unit {
  println("\{s}: \{i16_to_int(x)}")
}

///|
fn main {
  print_i16("int_to_i16(100)", int_to_i16(100))
  print_i16("int_to_i16(65536)", int_to_i16(65536))
  print_i16("int_to_i16(-1)", int_to_i16(-1))
  print_i16("int_to_i16(-65536)", int_to_i16(-65536))
  print_i16("byte_to_i16(255)", byte_to_i16(255))
  println("i16_to_int(100): \{i16_to_int(100)}")
  println("i16_to_byte(32766): \{i16_to_byte(32766)}")
  println("===test array===")
  let xs : Array[Int16] = [0, 32767, -32768]
  xs.push(100)
  for i in xs {
    print_i16("xs[i]", i)
  }
  print_u16("int_to_u16(100)", int_to_u16(100))
  print_u16("int_to_u16(65536)", int_to_u16(65536))
  print_u16("int_to_u16(-1)", int_to_u16(-1))
  print_u16("byte_to_u16(255)", byte_to_u16(255))
  println("u16_to_int(100): \{u16_to_int(100)}")
  println("u16_to_byte(32766): \{u16_to_byte(32766)}")
  println("===test array===")
  let xs : Array[UInt16] = [0, 32767, 65535]
  xs.push(100)
  for i in xs {
    print_u16("xs[i]", i)
  }
}
