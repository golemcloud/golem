let b = true

let f:(Bool) -> Unit = fn(x) {
  match x {
    true => println(1)
    false => println(2)
  }
}

let g:(Bool, Bool) -> Unit = fn(x, y) {
  match (x, y) {
    (true, false) => println(1)
    (false, true) => println(2)
    _ => println(3)
  }
}

fn init {
  f(b)
  g(b, b)

  xtest_match(true,false)
  xtest_match(false,true)
  xtest_match(true,true)
  xtest_match(false,false)
}

fn xtest_match(x : Bool, y : Bool) -> Unit {
  match (x, y) {
    (true, false) => println(1)
    (false, true) => println(2)
    (true, true) => println(3)
    (false, false) => println(4)
  }
}

