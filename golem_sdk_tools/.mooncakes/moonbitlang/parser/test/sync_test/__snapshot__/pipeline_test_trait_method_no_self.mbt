trait Named {
  name() -> String
}

fn[X : Named] name_of(_x : X) -> String {
  X::name()
}

impl Named for Int with name() { "Int" }
impl[X : Named] Named for Array[X] with name() {
  "Array[\{X::name()}]"
}

fn init {
  println(name_of([ 1, 2, 3 ]))
  println(name_of([ [ 1 ] ]))
}
