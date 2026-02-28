suberror FooA {
  BarA(label1~ : Int, label2~ : Bool) 
}

test "error enum" {
  let label1 = 5
  let fooa = BarA(label1~, label2=true)
  match fooa {
    BarA(label1~, label2=a) => ...
  }
}

enum FooB {
  BarB(label3~ : Int, label4~ : Bool)
}

test "enum" {
  let label3 = 6
  let foob = BarB(label3~, label4=false)
  match foob {
    BarB(label3~, label4=a) => println(label3)
  }
}

struct Rcd {
  label5 : Int
  label6 : Int
}
test "record" {
  let label5 = 8
  let rcd = { label5, label6: 20 }
  match rcd {
    { label5, label6: a } => ...
  }
}

fn f(label7~ : Int, label8~ : Int) -> Unit {}

test "fn label~" {
  let label7 = 9
  f(label7~, label8=11)
}

fn g(label9? : Int, label10? : Int, label11? : Int) -> Unit {}

test "fn label?" {
  let label9 = None
  g(label9?, label10=5, label11?=Some(5))
}

trait Trait {
  h(Self, label12~ : Int, label13~ : Int) -> Unit
}

type MyType Int

impl Trait for MyType with h(self, label12~, label13~) {}

test "impl trait" {
  let label12 = 12
  let label13 = 13
  let mytype = MyType(0)
  Trait::h(mytype, label12~, label13~)
  Trait::h(mytype, label12=5, label13=6)
}
