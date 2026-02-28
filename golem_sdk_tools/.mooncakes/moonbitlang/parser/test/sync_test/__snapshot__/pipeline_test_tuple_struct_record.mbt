struct R {
  mut x : Int
  y : Int
}

struct NTR(R)

fn init {
  let ntr : NTR = { x: 1, y: 2 }
  ntr.x += 1
  println(ntr.x * ntr.y)
}
