// struct Hello {
//   x : Int
//   y : Int
// }

// struct T {
//   mut f : (T) -> Unit
// }

// // pub fn hello(x : Hello)->Unit {
// // println(x.x)
// // }

// pub fn hello(x : T) -> Unit {
//   let t = { f: fn(t) { println("xxx") } }
//   (t.f)(t)
// }

pub fn outer(x : Double) -> (Double) -> Double {
  let a = x + 1.0
  fn inner(y ) {
    return y + a + x
  }
  return inner
}

pub fn caller() -> Double{
  return outer(1.0)(2.0)
}
