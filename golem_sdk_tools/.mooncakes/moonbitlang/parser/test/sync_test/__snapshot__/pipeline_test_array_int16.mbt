///|
fn make_i16_arr(a : Int16, b : Int16, c : Int16) -> FixedArray[Int16] {
  [a, b, c, -1]
}

///|
fn make_i16_arr2(a : Int16, b : Int16, c : Int16) -> FixedArray[Int16] {
  [a, b, c, a, b, c, -1]
}

///|
fn make_i16_arr3(a : Int16, b : Int16, c : Int16) -> Array[Int16] {
  [a, b, c, -1]
}

///|
fn main {
  let arr1 = make_i16_arr(0, -32768, 32767)
  let arr2 = make_i16_arr2(0, -32768, 32767)
  let arr3 = make_i16_arr3(0, -32768, 32767)
  println(arr1)
  println(arr2)
  println(arr3)
  let x = arr1[1]
  println(x)
  arr1[2] = -32768
  println(arr1)
  let arr4 : FixedArray[Int16] = FixedArray::make(5, -42)
  println(arr4)
  arr3.push(0)
  arr3.push(1)
  arr3.push(2)
  arr3.push(3)
  arr3.push(4)
  println(arr3)
  let arr5 : FixedArray[Int16] = [
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, -1, -2, -3, -4, -5,
  ]
  println(arr5)
}
