///|
fn i16_to_int(x : Int16) -> Int = "%i16_to_i32"

///|
fn i16_to_int64(x : Int16) -> Int64 = "%i16_to_i64"

///|
fn int_to_i16(x : Int) -> Int16 = "%i32_to_i16"

///|
fn int64_to_i16(x : Int64) -> Int16 = "%i64_to_i16"

///|
fn u16_to_int(x : UInt16) -> Int = "%u16_to_i32"

///|
fn u16_to_byte(x : UInt16) -> Byte = "%u16_to_byte"

///|
fn u16_to_int64(x : UInt16) -> Int64 = "%u16_to_i64"

///|
fn int_to_u16(x : Int) -> UInt16 = "%i32_to_u16"

///|
fn int64_to_u16(x : Int64) -> UInt16 = "%i64_to_u16"

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
  print_i16("int64_to_i16(32767)", int64_to_i16(32767))
  println("i16_to_int64(100): \{i16_to_int64(100)}")
  println("i16_to_int64(int_to_i16(-100)): \{i16_to_int64(int_to_i16(-100))}")
  print_u16("int64_to_u16(32767)", int64_to_u16(32767))
  println("u16_to_int64(100): \{u16_to_int64(100)}")
}
