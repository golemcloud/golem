///|
struct V {
  b1 : BytesView
  i : Int64
  b2 : BytesView
  s : String
}

///|
fn f(v : V) -> BytesView {
  println(v.b1[v.i.to_int()])
  println(v.b1.length())
  println(v.b2[v.s.length()])
  println(v.b2.length())
  [..v.b1, ..v.b2]
}

///|
fn main {
  let bs1 = Bytes::makei(10, i => i.to_byte())
  let bs2 = Bytes::makei(10, i => i.to_byte())
  let v = V::{ b1: bs1[1:9], i: 5, b2: bs2[2:8], s: "hello" }
  let bv = f(v)
  println(bv)
}
