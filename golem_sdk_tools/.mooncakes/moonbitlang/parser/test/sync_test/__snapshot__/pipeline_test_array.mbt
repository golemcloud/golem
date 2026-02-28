let a:FixedArray[Int] = FixedArray::make(3, 2)

let b:FixedArray[Int] = [1, 2, 3, 4]

fn init {
  println(b.length())
  println(b[3] + a[1])
  println(FixedArray::default())
}

