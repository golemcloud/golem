fn byte_to_f64(a : Byte) -> Double = "%byte_to_f64"
fn i32_to_byte(a : Int) -> Byte = "%i32_to_byte"
fn f64_to_byte(a : Double) -> Byte = "%f64_to_byte"
fn byte_to_i32(a : Byte) -> Int = "%byte_to_i32"
fn byte_to_u32(a : Byte) -> UInt = "%byte_to_u32"

fn byte_convertion() -> Unit {
  println("test i32 to byte:")
  i32_to_byte(5) |> println()
  i32_to_byte(255) |> println()
  i32_to_byte(257) |> println()

  println("test f64 to byte:")
  f64_to_byte(17.0) |> println()
  f64_to_byte(255.0) |> println()
  f64_to_byte(257.0) |> println()

  println("test byte to other types:")
  byte_to_f64(b'A') |> println()
  byte_to_i32(b'A') |> println()
  byte_to_u32(b'A') |> println()

  println("test negative:")
  i32_to_byte(-1) |> println()
  f64_to_byte(-1.0) |> println()
  i32_to_byte(-255) |> println()
  f64_to_byte(-255.0) |> println()
  i32_to_byte(-257) |> println()
  f64_to_byte(-257.0) |> println()
  i32_to_byte(-0) |> println()
  f64_to_byte(-0.0) |> println()
}

fn init {
  byte_convertion()
}
