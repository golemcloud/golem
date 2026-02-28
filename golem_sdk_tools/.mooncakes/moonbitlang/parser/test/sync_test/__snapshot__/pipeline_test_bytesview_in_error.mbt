///|
suberror Err  derive(Show)

///|
fn f(a : BytesView, b : Bool) -> BytesView raise Err {
  if b {
    return a[1:]
  } else {
    raise Err
  }
}

///|
fn g1() -> Unit {
  let v = Bytes::makei(10, i => i.to_byte())
  try {
    let v2 = f(v[1:9], true)
    println(v2)
  } catch {
    _ => println("error")
  }
}

///|
fn g2() -> Unit {
  let v = Bytes::makei(10, i => i.to_byte())
  try {
    let v2 = f(v[1:9], false)
    println(v2)
  } catch {
    _ => println("error")
  }
}

///|
fn main {
  g1()
  g2()
  let bs = Bytes::makei(10, i => i.to_byte())
  let v = bs[1:9]
  (try? f(v[1:7], true)) |> println
  (try? f(v[3:5], false)) |> println
}
