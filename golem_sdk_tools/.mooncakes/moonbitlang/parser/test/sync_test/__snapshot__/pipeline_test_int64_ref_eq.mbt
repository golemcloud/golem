fn[T] ref_eq(x: T, y: T) -> Bool {
    physical_equal(x,y)
}

fn main{
  println(ref_eq(1, 1))
  println(ref_eq("1", "1"))
  println(ref_eq(64L, 64))
  println(ref_eq(64.0, 64.))
}