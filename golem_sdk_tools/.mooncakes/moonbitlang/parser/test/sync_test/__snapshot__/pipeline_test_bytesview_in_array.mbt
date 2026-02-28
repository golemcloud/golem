///|
fn f(xs : Array[BytesView]) -> BytesView {
  [..xs[0], ..xs[1]]
}

///|
fn g(xs : FixedArray[BytesView]) -> Unit {
  println(xs)
  match xs {
    [a,  ..,  b] => { // unsafe_get and unsafe_get_rev
      println(a)
      println(b)
    }
    _ => ()
  }
}

///|
fn main {
  let bs1 = Bytes::makei(10, i => i.to_byte())
  let bs2 = Bytes::makei(10, i => i.to_byte())
  g(FixedArray::makei(5, i => bs1[i:])) // make_array len and init
  g([bs1[1:9], bs2[2:8]]) // make_array every single elem
  let arr = []
  arr.push(bs1[1:9]) // unsafe set and make_array uninit
  arr.push(bs2[2:8])
  let bv = f(arr)
  println(bv)
}
