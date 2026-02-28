struct T1(Int)
impl Eq for T1 with equal(l : T1, r : T1) -> Bool { l.0 == r.0 }

struct T2(Int)
fn T2::equal(l : T2, r : T2) -> Bool { l.0 == r.0 }
impl Eq for T2 with equal(_l, _r) { true }

struct T3(Int)
impl Eq for T3 with equal(l : T3, r : T3) -> Bool { l.0 == r.0 }

struct R {
  x1 : T1
  x2 : T2
  x3 : T3
} derive(Eq)

fn init {
  let r = { x1: 1, x2: 2, x3: 3 }
  println(r == { ..r, x1: 42 })
  println(r == { ..r, x2: 42 })
  println(r == { ..r, x3: 42 })
}
