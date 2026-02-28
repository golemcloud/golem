// constructor
enum T1 {
  A (Char)
  B (Int)
}

enum T2 {
  A (Int)
  B (Char)
}

fn f1(x: T1) -> Unit {
  match x {
    A(c) => println(c)
    B(i) => println(i)
  }
}

fn f2(x: T2) -> Unit {
  match x {
    A(i) => println(i)
    B(c) => println(c)
  }
}

fn init {
  f1(A('1'))
  f1(B(2))
  f2(A(3))
  f2(B('4'))
}

// record field
struct R1 {
  a:T1
  b:T2
}

struct R2 {
  a:T2
  b:T1
}

fn f3(x: R1) -> Unit {
  match x {
    {a:A(c), b:A(i)} => { println(c); println(i) }
    {a:A(c), b:B(d)} => { println(c); println(d) }
    {a:B(i), b:A(j)} => { println(i); println(j) }
    {a:B(i), b:B(c)} => { println(i); println(c) }
  }
}

fn f4(x: R2) -> Unit {
  match x {
    {a:A(i), b:A(c)} => { println(i); println(c) }
    {a:A(i), b:B(j)} => { println(i); println(j) }
    {a:B(c), b:A(d)} => { println(c); println(d) }
    {a:B(c), b:B(i)} => { println(c); println(i) }
  }
}

fn init {
  f3({ a:B(5), b:B('6') })
  f4({ a:B('7'), b:B(8) })
}

// let
fn init {
  let x1 = (A('9') : T1)
  let x2 : T2 = A(1)
  let x3 = ((B(0) : T1) : T1)
  f1(x1)
  f2(x2)
  f1(x3)
  let x4 : R1 = { a:A('J'), b:B('Q') }
  let x5 = ({ a:B('K'), b:A('A') } : R2)
  f3(x4)
  f4(x5)
}

