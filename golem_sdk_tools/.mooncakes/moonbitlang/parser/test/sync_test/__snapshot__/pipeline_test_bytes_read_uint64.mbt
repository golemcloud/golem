///|
#intrinsic("%bytes.unsafe_read_uint64_le")
fn unsafe_read_uint64_le(bytes : Bytes, index : Int) -> UInt64 {
  let mut result : UInt64 = 0
  for i in 0..=7 {
    result = result | (bytes[i + index].to_uint64() << (8 * i))
  }
  result
}

///|
#intrinsic("%bytes.unsafe_read_uint64_be")
fn unsafe_read_uint64_be(bytes : Bytes, index : Int) -> UInt64 {
  let mut result : UInt64 = 0
  for i in 0..=7 {
    result = result | (bytes[i + index].to_uint64() << (8 * (7 - i)))
  }
  result
}

///|
#intrinsic("%bytes.unsafe_read_uint32_le")
fn unsafe_read_uint32_le(bytes : Bytes, index : Int) -> UInt {
  let mut result : UInt = 0
  for i in 0..=3 {
    result = result | (bytes[i + index].to_uint() << (8 * i))
  }
  result
}

///|
#intrinsic("%bytes.unsafe_read_uint32_be")
fn unsafe_read_uint32_be(bytes : Bytes, index : Int) -> UInt {
  let mut result : UInt = 0
  for i in 0..=3 {
    result = result | (bytes[i + index].to_uint() << (8 * (3 - i)))
  }
  result
}

///|
#intrinsic("%bytes.unsafe_read_uint16_le")
fn unsafe_read_uint16_le(bytes : Bytes, index : Int) -> UInt16 {
  let mut result : UInt16 = 0
  for i in 0..=1 {
    result = result | (bytes[i + index].to_uint16() << (8 * i))
  }
  result
}

///|
#intrinsic("%bytes.unsafe_read_uint16_be")
fn unsafe_read_uint16_be(bytes : Bytes, index : Int) -> UInt16 {
  let mut result : UInt16 = 0
  for i in 0..=1 {
    result = result | (bytes[i + index].to_uint16() << (8 * (1 - i)))
  }
  result
}

///|
fn main {
  let bytes : Bytes = Bytes::makei(50, i => i.to_byte())
  match bytes {
    [u64_le(u1), u64_be(u2), u64_le(u3), u64_be(u4), u32_le(u5), u32_be(u6), u16_le(u7), u16_be(u8), ..] => {
      guard unsafe_read_uint64_le(bytes, 0) == u1
      guard unsafe_read_uint64_be(bytes, 8) == u2
      guard unsafe_read_uint64_le(bytes, 16) == u3
      guard unsafe_read_uint64_be(bytes, 24) == u4
      guard unsafe_read_uint32_le(bytes, 32) == u5
      guard unsafe_read_uint32_be(bytes, 36) == u6
      guard unsafe_read_uint16_le(bytes, 40).to_uint() == u7
      guard unsafe_read_uint16_be(bytes, 42).to_uint() == u8
      println("ok")
    }
    _ => abort("fail")
  }
}

