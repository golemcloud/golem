enum T1[X, Y] {
  A (X)
  B (Y)
}

enum T2 {
  A (String)
  B (String)
}

fn[X] f1 (x: T1[Char, X]) -> Unit {
  match x {
    A(c) => println(c)
    B(_) => println(0)
  }
}

fn[Y] f2 (x: T1[Y, Char]) -> Unit {
  match x {
    A(_) => println(0)
    B(c) => println(c)
  }
}

fn init {
  let x1 = (A('o') : T1[_, _])
  let x2 = (B('k') : T1[_, _])
  f1(x1)
  f2(x2)
}

struct R1[X] {
  a:X
  b:Int
}

struct R2[X] {
  a:String
  b:X
}

fn init {
  let f3 = fn(x: R1[_]) { println((x.a: Char)) } // unify _ to Char
  let f4 = fn(x: R2[_]) { println((x.b: Char)) }
  let x3 = ({ a:'o', b:1 } : R1[_])
  let x4 = ({ b:'k', a:"x" } : R2[_])
  f3(x3)
  f4(x4)
}

fn[A] get_r1_a (x: R1[A]) -> A {
  x.a
}

// generics
fn init {
  let a = ({ a:"ok", b:0 } : R1[_])
  let b = ({ a:'!', b:1 } : R1[_])
  println(get_r1_a(a))
  println(get_r1_a(b))
}

// p<_> gets expanded to p<_,_>
struct P[X, Y] {
  a:X
  b:Y
}

fn init {
  let output_int_char = fn(x: P[_]) { println((x.a: Int)); println((x.b: Char)) }
  output_int_char({ a:0, b:'k' })
}

