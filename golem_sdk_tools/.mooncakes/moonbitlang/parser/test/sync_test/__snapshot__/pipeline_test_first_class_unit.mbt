fn use_unit(_x : Unit) -> Unit {
}

struct S {
  u : Unit
} derive(Show)

fn init {
  let r : Ref[_] = { val: 42 }
  use_unit({ r.val += 1 })
  let s : S = { u: { r.val += 1 } }
  println(s)
  use_unit(ignore(println("!")))
}

///|
let counter : Ref[Int] = { val: 0 }

///|
fn mutate() -> Unit? {
  if counter.val < 42 {
    Some({ counter.val += 1 })
  } else {
    None
  }
}

///|
fn init {
  while mutate() is Some(_) {
  }
  println(counter.val)
}
