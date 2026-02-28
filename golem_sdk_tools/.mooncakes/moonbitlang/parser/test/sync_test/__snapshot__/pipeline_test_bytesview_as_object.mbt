///|
fn use_show(x : &Show) -> String {
  x.to_string()
}

///|
fn main {
  let x = Bytes::makei(10, i => i.to_byte())
  let y = use_show(x[1:9])
  println(y)
}
