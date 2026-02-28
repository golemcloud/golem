//!build: -w -27

///|
pub fn decode_uint64(bs : Bytes) -> UInt64 {
  let shift = 0
  let result = 0UL
  loop (bs[:], shift, result) {
    ([u1(0), u7(value)], shift, result) => {
      let value = value.to_uint64()
      result | (value << shift)
    }
    ([u1(1), u7(value), .. rest], shift, result) => {
      let value = value.to_uint64()
      continue (rest, shift + 7, result | (value << shift))
    }
    _ => abort("Invalid LEB128 encoding")
  }
}

///|
pub fn decode_int64(bs : Bytes) -> Int64 {
  let result = 0L
  let shift = 0
  loop (bs[:], shift, result) {
    ([u1(b), u7(value), .. rest], shift, result) => {
      let value = value.to_int64()
      let next_result = result | (value << shift)
      let next_shift = shift + 7
      if b == 1 {
        continue (rest, next_shift, next_result)
      } else if (value & 0x40) != 0 {
        // sign extend
        let sign_extend_mask = -1L << next_shift
        next_result | sign_extend_mask
      } else {
        next_result
      }
    }
    _ => abort("Invalid SLEB128 encoding")
  }
}

///|
pub fn encode_uint64(i : UInt64) -> Bytes {
  let bytes = Array::new()
  let mut remaining = i
  while true {
    let mut byte = (remaining & 0x7FUL).to_int()
    remaining = remaining >> 7
    if remaining != 0UL {
      byte = byte | 0x80
    }
    bytes.push(byte.to_byte())
    if remaining == 0UL {
      break
    }
  }
  Bytes::from_array(bytes)
}

///|
pub fn encode_int64(i : Int64) -> Bytes {
  let bytes = Array::new()
  let mut remaining = i
  let mut more = true
  while more {
    let mut byte = (remaining & 0x7FL).to_int()
    remaining = remaining >> 7

    // Check if we need more bytes
    // For positive numbers: if remaining is 0 and sign bit is 0
    // For negative numbers: if remaining is -1 and sign bit is 1
    if (remaining == 0L && (byte & 0x40) == 0) ||
      (remaining == -1L && (byte & 0x40) != 0) {
      more = false
    } else {
      byte = byte | 0x80
    }
    bytes.push(byte.to_byte())
  }
  Bytes::from_array(bytes)
}

///|
fn main {
  let test_values : Array[UInt64] = [
    0UL, 1UL, 127UL, 128UL, 255UL, 256UL, 16383UL, 16384UL, 65535UL, 65536UL, 1048575UL,
    1048576UL, 16777215UL, 16777216UL, 4294967295UL, 4294967296UL, 1099511627775UL,
    1099511627776UL, 18446744073709551615UL,
  ]
  for i in test_values {
    let b = encode_uint64(i)
    println(b)
    let j = decode_uint64(b)
    if i != j {
      abort("failed")
    }
  }
  let test_values = [
    0L, 1L, -1L, 63L, -64L, 64L, -65L, 127L, -128L, 8191L, -8192L, 16383L, -16384L,
    1048575L, -1048576L, 134217727L, -134217728L, 2147483647L, -2147483648L,
  ]
  for i in test_values {
    let b = encode_int64(i)
    println(b)
    let j = decode_int64(b)
    if i != j {
      abort("failed")
    }
  }
}
