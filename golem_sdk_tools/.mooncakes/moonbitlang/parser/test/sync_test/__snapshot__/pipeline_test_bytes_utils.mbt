
//write/read string
fn init {
  let b = FixedArray::make(60, b'\x00')
  let s1 = @encoding/utf16.encode("hello,")
  let s2 = "world"
  b.blit_from_bytes(0, s1, 0, s1.length())
  b.blit_from_string(s1.length(), s2, 0, s2.length())
  let str_data = FixedArray::make(s1.length() + s2.length() * 2, b'\x00')
  b.blit_to(str_data, src_offset=0, dst_offset=0, len=s1.length() + s2.length() * 2)
  println(Bytes::from_array(str_data).to_unchecked_string())
}
