///|
fn main {
  let bs = Bytes::makei(10, i => i.to_byte())
  let arr = []
  for i in 0..<10 {
    arr.push(bs[i:])
  }
  println(arr)
  for i in 0..<10 {
    let _ = arr.pop()

  }
  println(arr)
}
