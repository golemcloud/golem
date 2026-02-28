///|
fn map_literal_with_byte_and_bytes() -> Unit {
  let _ = { b"key1": b"val1", b"key2": b"val2" }
  let _ = { b'1': b'a', b'2': b'b' }

}

///|
fn map_pattern_with_byte_and_bytes() -> Unit {
  match e {
    { b"key1": b"val1", b"key1": b"val2", .. } => ...
    { b'1': b'a', b'2': b'b', .. } => ...
  }
}
