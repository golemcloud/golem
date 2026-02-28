let test1:(Bool) -> Unit = fn(b) {
  if b { println("ok\n") } else { println("not ok\n") }
}

struct R1 {
  x:Int
  y:Bool
}

struct R2[A, B] {
  a:A
  b:B
}

let p1:R1 = { x:3, y:true }

let p2:R2[Char, Char] = { a:'a', b:'b' }

let p3:R2[R1, R2[Char, Char]] = { a:p1, b:p2 }

let p4:R2[Int, Int] = { b:2, a:1 }

fn init {
  test1(p1.x == 3)
  test1(p2.a == 'a')
  test1(p2.b == 'b')
  test1(p3.a.x == 3)
  test1(p3.b.a == 'a')
  test1(p4.b == 2)
  test1(p4.a == 1)
}

// mutation
struct R3 {
  mut x1:Int
  x2:Bool
}

let p5:R3 = { x1:3, x2:true }

fn init {
  test1(p5.x1 == 3)
  p5.x1 = 4
  test1(p5.x1 == 4)
}

// pattern matching
fn init {
  match p1 {
    {x:v1, y:v2} => { test1(v1 == 3); test1(v2 == true) }
    _ => println("not ok\n")
  }
}

fn init {
  match p2 {
    {b:c1, a:c2} => { test1(c1 == 'b'); test1(c2 == 'a') }
    _ => println("not ok\n")
  }
}

// generalization
struct R4[A] {
  y1:A
  y2:Int
}

fn[A] get_y1 (p: R4[A]) -> A {
  p.y1
}

fn init {
  let p7 = { y1:Option::None, y2:1 }
  let p8 = { y1:p7, y2:2 }
  match get_y1(get_y1(p8)) {
    Some(_) => println("not ok\n")
    None => println("ok\n")
  }
}

// value restriction
struct R5[A] {
  mut z1:A
  z2:Int
}

fn init {
  let p9 = { z1:Option::None, z2:3 }
  ()
}

// more about generalization
struct R6[A, B, C] {
  w1:A
  w2:B?
  w3:R2[C, C]
}
// more pattern matching

fn init {
  let p11 = { w1: @list.empty(), w2:Option::Some(1), w3:{ a:"xxx", b:"yyy" } }
  match p11 {
    {w1:v1, w2:None, w3:_} => println("not ok\n")
    {w1:v1, w2:Some(0), w3:{a:_, b:_}} => println("not ok\n")
    {w1:_, w2:Some(1), w3:{a:v1, b:v2}} => {
      println(v1)
      println(v2)
      println("...ok\n")
    }
    _ => println("not ok\n")
  }
}

// more about polymorphism
struct R7[A, B] {
  ff:(A, B) -> A
  gg:Int
}

fn init {
  let p14 = { ff:fn(x, y) { x }, gg:1 }
  test1((p14.ff)(1, 'c') == 1)
}

// record disambiguate
struct R8 {
  i:Int
  j:Bool
}

struct R9 {
  i:Bool
  j:Int
}

fn init {
  let x1 = ({ i:1, j:true } : R8)
  let x2 = ({ i:true, j:2 } : R9)
  let f1 = fn(x: R8) { if x.j && x.i == 1 { println("ok\n") } }
  let f2 = fn(x: R9) { if x.i && x.j == 2 { println("ok\n") } }
  f1(x1)
  f2(x2)
}

// record disambiguate on mutation
struct R10 {
  mut i:Int
  j:Bool
}

struct R11 {
  mut i:Bool
  j:Int
}

fn init {
  let x1 = ({ i:1, j:true } : R10)
  let x2 = ({ i:false, j:2 } : R11)
  let f1 = fn(x: R10, y: R11) { x.i = x.i + y.j; y.i = x.j || y.i }
  f1(x1, x2)
  if x1.j && x1.i == 3 { println("ok\n") }
  if x2.i && x2.j == 2 { println("ok\n") }
  ()
}

