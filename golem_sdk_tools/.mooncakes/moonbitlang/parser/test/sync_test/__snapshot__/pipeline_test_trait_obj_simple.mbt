trait H {
  name(Self)->String
}

pub fn hey(x : &H) -> Unit {
  println(x.name())
}

struct UU {
  name : String
}

pub impl H for UU with name(self : UU) -> String {
  self.name
}

fn init {
  println(hey(UU::{name : "xx"} as &H))
}


fn init {
  let a = UU::{name : "yy"} as &H
  let _ = a
}
