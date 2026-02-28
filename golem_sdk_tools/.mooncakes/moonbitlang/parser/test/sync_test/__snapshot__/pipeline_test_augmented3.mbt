struct U {
  mut arr : FixedArray[U]
  get : FixedArray[Int]
  set : FixedArray[Int]
}

fn U::op_get(self : U, id : Int) -> U {
  self.get[id] += 1
  self.arr[id]
}

fn U::op_set(self : U, id : Int, val : U) -> Unit {
  self.set[id] += 1
  self.arr[id] = val
}

fn init {
  let mut a = { arr: [], get: FixedArray::make(6, 0), set: FixedArray::make(6, 0) }
  a.arr = FixedArray::make(6, a)
  a[1][1][4][5][1][4] = a
  println(a.get)
  println(a.set)
  a = a[1][1][4][5][1][4]
  println(a.get)
  println(a.set)
  println(a[0][1][2][3][4][5].arr[5][4][3][2][1][0].get)
  println(a[5][4][3][2][1][0].arr[0][1][2][3][4][5].set)
}

