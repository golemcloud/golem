enum T {
  A
  B
}

fn init {
  let c : T = B
  let d : T = A
  match d {
    A => println("ok")
    B => println("notok")
  }
  match c {
    A => println("notok")
    B => println("ok")
  }
}

