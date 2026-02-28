let test1:(Bool) -> Unit = fn(b) {
  if b { println("ok\n") } else { println("not ok\n") }
}

struct R[A] {
  id:(A) -> A
}

fn init {
  let p1 : R[_] = { id:fn(x) { x }, }
  let p2 : R[_] = { id:fn(x) { x }, }
  test1((p1.id)(1) == 1)
  test1((p2.id)('c') == 'c')
}

