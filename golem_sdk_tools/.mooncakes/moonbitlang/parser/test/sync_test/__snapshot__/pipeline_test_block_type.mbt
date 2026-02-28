struct T {
  mut v: Int
}

fn f(i: Int) -> Unit {
  let x =
  match i {
    1 | 2 => { v: 1 }
    _ => { v: 0 }
  }
  println(x.v)
}

fn init {
 f(1)
}
