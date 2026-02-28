let a:FixedArray[Int] = FixedArray::make(3, 2)

fn init {
  println(a.length())
}

let d = 4

fn init {
  a[1] = 33
  println(a[1])
  println((a[2] + d + a[1]))
  println(a.length())
}

