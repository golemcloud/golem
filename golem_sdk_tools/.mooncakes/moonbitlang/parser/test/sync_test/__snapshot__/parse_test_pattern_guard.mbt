fn f(x: Int) -> Unit {
  match x {
    1 if x is 0 => ()
    2 if { match x { 1 => true; _ => false } } => ()
    3 if x >= 0 && x <= 100 => ()
    c if c < 0 => ()
  }
}

///|
fn g(f : (Int, Int) -> Int) -> Int {
  f(1, 2)
}

///|
suberror E Int

///|
fn f1(x : Int) -> Int raise E {
  raise E(x)
}

fn f2() -> Unit {
  try f1(42) catch {
    E(x) if x >= 0 => println(x)
    E(x) as e if e is E(y) => println(y)
    E(x) => println(x)
  } else {
    c if c < 0 => ()
  }
}

fn f3(x: Int, y: Int) -> Int {
  loop (x, y) {
    (0, 0) if x > 0 => x
    (0, 0) if y > 0 => y
    (x, y) => x + y
  }
}
