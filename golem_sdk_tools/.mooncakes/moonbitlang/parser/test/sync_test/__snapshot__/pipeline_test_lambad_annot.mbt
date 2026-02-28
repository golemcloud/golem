enum T {
  A
  B
}

fn init {
  let a = fn()-> T { if true { A } else { B } }
  match a() {
    A => println("A")
    B => println("B")
  }
}

