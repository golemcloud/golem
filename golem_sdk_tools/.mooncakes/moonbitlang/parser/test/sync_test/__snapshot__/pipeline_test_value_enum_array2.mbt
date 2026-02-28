///|
#valtype
enum E {
  A
  B(Int)
  C(Double, Double)
} derive(Show)

///|
fn main {
  let e3 = E::C(1.0, 2.0)
  let arr3 = FixedArray::make(5, e3)
  arr3[1] = E::B(1)
  println(arr3)
}
