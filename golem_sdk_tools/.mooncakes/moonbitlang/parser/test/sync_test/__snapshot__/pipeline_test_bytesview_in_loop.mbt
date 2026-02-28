///|
fn f1(x : BytesView) -> Int {
  loop (x, 0) {
    ([], acc) => acc
    ([b, .. bs], acc) => continue (bs, acc + b.to_int())
  }
}

///|
fn f2(x : BytesView) -> BytesView {
  loop (x, 0) {
    ([], acc) => [acc.to_byte()]
    ([b, .. bs], acc) =>
      if b > 5 {
        continue (bs, acc + b.to_int())
      } else {
        break bs
      }
  }
}

///|
fn main {
  let x = Bytes::makei(10, i => i.to_byte())
  println(f1(x[1:9]))
  println(f2(x[1:9]))
  println(f2(x[6:]))
}
