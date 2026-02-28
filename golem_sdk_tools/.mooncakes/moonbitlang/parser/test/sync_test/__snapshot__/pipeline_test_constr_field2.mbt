enum E1 {
  C(Bool, Bool, x~ : Int)
}

// case 1: the join point of match arm is not single-use and not inlined
fn f1(e : E1) -> Unit {
  match e {
    C(true, true, ..) => ()
    C(_) as c => println(c.x)
  }
}

fn init {
  f1(C(true, true, x=0))
  f1(C(true, false, x=0))
}

// case 2: captured by closure
enum E2 {
  C(mut x~ : Int)
} derive(Show)

fn f2(e : E2) -> Unit {
  let f =
    match e {
      C(_) as c => fn () { c.x += 1 }
    }
  f()
  f()
}

fn init {
  let e : E2 = C(x=0)
  f2(e)
  println(e)
}

// case 3: different value for same binding
fn f3(e1 : E2, e2: E2) -> Unit {
  match (e1, e2) {
    (C(x=42), C(_) as c) | (C(_) as c, C(_)) => c.x += 1
  }
}

fn init {
  let e1 : E2 = C(x=42)
  let e2 : E2 = C(x=0)
  f3(e1, e2)
  println((e1, e2))
  f3(e2, e1)
  println((e1, e2))
}
