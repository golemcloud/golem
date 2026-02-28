///|
let x : Bytes = Bytes::makei(10, i => i.to_byte())

///|
let v : BytesView = x[1:9]

///|
fn main {
  println(v[0] + v[1])
  let mut v2 = Bytes::makei(10, i => i.to_byte())[3:7]
  println(v2[0] + v2[1])
  v2 = v[1:]
  println(v2[0] + v2[1])
}
