///|
fn get_bytes(b : BytesView) -> Bytes = "%bytesview.bytes"

///|
fn get_start(b : BytesView) -> Int = "%bytesview.start"

///|
fn get_len(b : BytesView) -> Int = "%bytesview.len"

///|
fn make_bytesview(b : Bytes, start : Int, len : Int) -> BytesView = "%bytesview.make"

///|
fn main {
  let b : Bytes = "hello"
  let bv = b[1:4]
  println(get_bytes(bv))
  println(get_start(bv))
  println(get_len(bv))
  let bvv = make_bytesview(b, 1, 4)
  println(bvv)
}
