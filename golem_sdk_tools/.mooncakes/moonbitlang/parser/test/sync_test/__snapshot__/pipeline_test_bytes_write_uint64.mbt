///|
#intrinsic("%bytes.unsafe_write_uint64_le")
fn unsafe_write_uint64_le(
  bytes : FixedArray[Byte],
  index : Int,
  value : UInt64,
) -> Unit {
  for i in 0..=7 {
    bytes[i + index] = (value >> (8 * i)).to_byte()
  }
}

///|
#intrinsic("%bytes.unsafe_write_uint64_be")
fn unsafe_write_uint64_be(
  bytes : FixedArray[Byte],
  index : Int,
  value : UInt64,
) -> Unit {
  for i in 0..=7 {
    bytes[i + index] = ((value >> (8 * (7 - i))) & 0xFF).to_byte()
  }
}

///|
#intrinsic("%bytes.unsafe_write_uint32_le")
fn unsafe_write_uint32_le(
  bytes : FixedArray[Byte],
  index : Int,
  value : UInt,
) -> Unit {
  for i in 0..=3 {
    bytes[i + index] = ((value >> (8 * i)) & 0xFF).to_byte()
  }
}

///|
#intrinsic("%bytes.unsafe_write_uint32_be")
fn unsafe_write_uint32_be(
  bytes : FixedArray[Byte],
  index : Int,
  value : UInt,
) -> Unit {
  for i in 0..=3 {
    bytes[i + index] = ((value >> (8 * (3 - i))) & 0xFF).to_byte()
  }
}

///|
#intrinsic("%bytes.unsafe_write_uint16_le")
fn unsafe_write_uint16_le(
  bytes : FixedArray[Byte],
  index : Int,
  value : UInt16,
) -> Unit {
  for i in 0..=1 {
    bytes[i + index] = ((value >> (8 * i)) & 0xFF).to_byte()
  }
}

///|
#intrinsic("%bytes.unsafe_write_uint16_be")
fn unsafe_write_uint16_be(
  bytes : FixedArray[Byte],
  index : Int,
  value : UInt16,
) -> Unit {
  for i in 0..=1 {
    bytes[i + index] = ((value >> (8 * (1 - i))) & 0xFF).to_byte()
  }
}

///|
fn main {
  let bytes : FixedArray[Byte] = FixedArray::make(30, 0)
  unsafe_write_uint64_le(bytes, 0, 0x1234567890ABCDEFUL)
  unsafe_write_uint64_be(bytes, 8, 0x1234567890ABCDEFUL)
  unsafe_write_uint32_le(bytes, 16, 0x12345678)
  unsafe_write_uint32_be(bytes, 20, 0x12345678)
  unsafe_write_uint16_le(bytes, 24, 0x1234)
  unsafe_write_uint16_be(bytes, 26, 0x1234)
  println(bytes)
}
