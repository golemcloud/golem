let a0 : FixedArray[Int] = []
let a1 : FixedArray[Byte] = []

let a2 : FixedArray[String] = []
fn len() -> Int {
  println("not inlined")
  0
}
fn init {

  let v0 :FixedArray[Int] = FixedArray::make(len(),0)
  let v1 : FixedArray[Byte] = FixedArray::make(len(),0)
  let v2 : FixedArray[String] = FixedArray::make(len(),"")

  if v0 == a0 {
    println("ok")
  } else {
    println("not ok")
  }
  if v1 == a1 {
    println("ok")
  } else {
    println("not ok")
  }
  if v2 == a2 {
    println("ok")
  } else {
    println("not ok")
  }
}