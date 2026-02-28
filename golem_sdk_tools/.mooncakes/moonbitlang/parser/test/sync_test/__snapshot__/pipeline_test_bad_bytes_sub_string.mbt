fn init {
  let b = FixedArray::make(10, b'\x00')
  let _ = b.set_utf8_char(0, 'a')
  let _ = Bytes::from_array(b).to_unchecked_string(offset=0, length=1)
}
