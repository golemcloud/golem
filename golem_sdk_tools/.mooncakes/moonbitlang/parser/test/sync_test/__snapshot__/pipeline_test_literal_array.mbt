fn init {
  // not interned
  let a : FixedArray[Int] = []
  println(a.length())
  let a: FixedArray[_] = [1]
  println(a.length())
  let a: FixedArray[_] = [1, 2]
  println(a.length())
  let a: FixedArray[_] = [1, 2, 3]
  println(a.length())
  let a: FixedArray[_] = [1, 2, 3, 4]
  println(a.length())

  // interned array
  let a: FixedArray[_] = [1, 2, 3, 4, 5]
  println(a.length())
  let a: FixedArray[_] = [1, 2, 3, 4, 97, 98, 99, 100]
  println(a[0])
  let b: FixedArray[_] = [1, 2, 3, 4, 97, 98, 99, 100]
  println(b[0])

  // not literal
  let (x, y, z) = (1, 2, 3)
  let c: FixedArray[_] = [x, y, z]
  println(c[0])
  let c: FixedArray[_] = [a[0], b[1], a[2], b[3], a[4], b[5], a[6], a[7]]
  println(c[0])
}

fn init {
  // not interned
  let a : FixedArray[Int64] = []
  println(a.length())
  let a: FixedArray[_] = [1L]
  println(a.length())
  let a: FixedArray[_] = [1L, 2L]
  println(a.length())
  let a: FixedArray[_] = [1L, 2L, 3L]
  println(a.length())
  let a: FixedArray[_] = [1L, 2L, 3L, 4L]
  println(a.length())

  // interned array
  let a: FixedArray[_] = [1L, 2L, 3L, 4L, 5L]
  println(a.length())
  let a: FixedArray[_] = [1L, 2L, 3L, 4L, 97L, 98L, 99L, 100L]
  println(a[0])
  let b: FixedArray[_] = [1L, 2L, 3L, 4L, 97L, 98L, 99L, 100L]
  println(b[0])

  // not literal
  let (x, y, z) = (1L, 2L, 3L)
  let c: FixedArray[_] = [x, y, z]
  println(c[0])
  let c: FixedArray[_] = [a[0], b[1], a[2], b[3], a[4], b[5], a[6], a[7]]
  println(c[0])
}

