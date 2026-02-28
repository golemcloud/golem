fn init {
  let x: FixedArray[_] = [1.1,2.2,3.3]
  let y = x[0]
  println(y)
  let s: FixedArray[_] = ["abc","def","com"]
  println(s[1])
  println(x.length().to_double())
  println(x[1])
  x[1] = 4.4
  println(x[1])
  let a = FixedArray::make(3, 4.5)
  println(a[0])
  println(a[2])
  a[2] = 5.5
  println(a[2])

}
