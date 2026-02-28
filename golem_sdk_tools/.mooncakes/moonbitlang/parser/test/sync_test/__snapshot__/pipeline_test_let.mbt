
let output_string:(String) -> Unit = fn(n) {
  for c in n {
    println(c)
  }
}

fn even(n: Int) -> Unit {
  if n == 0 { println("yes\n") } else { odd(n - 1) }
}

fn odd(n: Int) -> Unit {
  if n == 0 { println("no\n") } else { even(n - 1) }
}

fn init {
  odd(7)
  odd(6)
  even(6)
  even(7)
}

enum T {
  A
  B (Int)
  C (Char)
}

fn init {
  let B(x) = T::B(3)
  println(x)
}

// let () =
// let B(3) = C('c') ;
fn init {
  let a : T = B(7)
  let b : T = C('e')
  let B(x) = a
  let C(y) = b
  println(x)
  println(y)
}

// fn f<X, Y> (a: X, b: Y): Unit { println(3) }
fn init {
  // value restrict
  let f = (fn(a) { Option::None }) (3)
  // value restrict
  let f = (fn(x) { x }) (fn(x) { x })
  let f = fn(x) { (fn(x) { x }) (fn(x) { x })(x) }
  fn f(x) {
    (fn(x) { x }) (fn(x) { x })(x)
  }

  let f = fn() { Option::None }
  let x = f()
  match x {
    Some(a) => println("some!\n")
    None => println("none!\n")
  }
}

// constructors
let f1:(Int) -> T = fn(x) { B(x) }

let f2:(Char) -> T = fn(x) { C(x) }

// let f = fn (x, xs) -> (::) (x, xs)
// let f = (::)

let s1 = "yes\n"

let s2 = "no\n"

fn even1(n: Int) -> Unit {
  if n == 0 { println(s1) } else { odd(n - 1) }
}

fn odd1(n: Int) -> Unit {
  if n == 0 { println(s2) } else { even(n - 1) }
}

fn init {
  odd1(7)
  even1(7)
}

