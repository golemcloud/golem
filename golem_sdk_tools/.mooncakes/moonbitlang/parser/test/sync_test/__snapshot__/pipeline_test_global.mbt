let a:FixedArray[Int] = FixedArray::make(3, 2)

let b:FixedArray[Int] = [1, 2, 3, 4]

let x:Ref[Double] = { val: 3.14 }

fn init {
  let c = a
  println(b.length())
  println(b[3] + a[1])
  a[0] = 3
  println(c[0])
  let y = x
  println(x.val)
  y.val = 9.29
  println(x.val)
}
