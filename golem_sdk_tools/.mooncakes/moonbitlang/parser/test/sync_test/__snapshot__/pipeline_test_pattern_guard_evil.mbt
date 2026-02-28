fn f2(xs : Array[Int]) -> Unit {
  fn evil() {
    xs[0] = 42
    println("evil")
    false
  }
  match xs {
    [] => println("empty")
    [42, .._] => println("hd is 42")
    [_, .._] if evil() => abort("")
    [42, .._] => println("hd is changed to 42")
    _ => println("hd is never 42")
  }

}

///|
struct U {
  mut b : Int?
  a : Bool
}

///|
struct V {
  a : Bool
  mut b : Int?
}

fn fu (x: U) -> Int {
    match x {
     {a : false, b : Some(_)} => 0
     {a : _, b : _} if { x.b = None; false } => 2
     {a : _, b : None} => 1
     {a : true, b : Some(y)} => y
    }
}

fn fv (x: V) -> Int {
    match x {
     {a : false, b : Some(_)} => 0
     {a : _, b : _} if { x.b = None; false } => 2
     {a : _, b : None} => 1
     {a : true, b : Some(y)} => y
    }
}

///|
fn main {
  f2([0, 1])
  f2([42, 1])
  let a = fu({ a: true, b: Some(5) })
  let b = fv({ a: true, b: Some(5) })
  println(a)
  println(b)
}
