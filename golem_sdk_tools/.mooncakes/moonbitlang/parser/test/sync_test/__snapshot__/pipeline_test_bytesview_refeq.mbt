///|
fn main {
  let bs1 = Bytes::makei(10, i => i.to_byte())
  let bs2 = Bytes::makei(10, i => i.to_byte())
  let bv1 = bs1[1:9]
  let bv2 = bs2[1:9]
  let bv3 = bs1[1:9]
  println(physical_equal(bv1, bv2))
  if physical_equal(bv1, bv3) { // with value type, this should be true
    println("equal")
  } else {
    println("equal")
  }
}
