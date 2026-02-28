struct T {
  v: Int
}

pub fn f(t: T?) -> Int {
  match t {
    Some({ v: 1 } as s) | Some({ v: 2 } as s) => s.v
    _ => 3
  }
}

struct S {
  ss: String
}

pub fn g(s: S) -> String {
  match s {
    ({ ss: "xxx" } as s) | ({ ss: "yyy" } as s) => s.ss
    _ => "zzz"
  }
}

fn init {
  println(f(None))
  println(f(Some({v: 2})))
  println(f(Some({v: 1})))
  println(g({ ss: "xxx" }))
  println(g({ ss: "yyy" }))
  println(g({ ss: "zzz" }))
}
