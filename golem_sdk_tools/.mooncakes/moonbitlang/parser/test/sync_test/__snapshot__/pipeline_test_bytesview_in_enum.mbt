///|
enum W {
  A(BytesView, Int64)
  B(String, BytesView, Int64)
}

///|
fn mk_a(bv : BytesView, d : Int64) -> W {
  W::A(bv, d)
}

///|
fn mk_b(s : String, bv : BytesView, d : Int64) -> W {
  W::B(s, bv, d)
}

///|
fn f(v : W) -> BytesView {
  match v {
    W::A(bv, d) => {
      println(bv[d.to_int()])
      bv[1:]
    }
    W::B(s, bv, d) => {
      println(bv[s.length() - d.to_int()])
      bv[2:]
    }
  }
}

///|
fn main {
  let bs1 = Bytes::makei(10, i => i.to_byte())
  let bs2 = Bytes::makei(10, i => i.to_byte())
  let v = mk_a(bs1[1:9], 5L)
  let bv = f(v)
  let v2 = mk_b("hello", bs2[2:8], 5L)
  let bv2 = f(v2)
  println(bv)
  println(bv2)
}
