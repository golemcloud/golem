struct E { }

enum T {
  A(E)
  B(Int)
}

fn f1(t: T) -> Unit {
  match t {
    A({}) => println("A")
    B(i) => println(i)
  }
}

impl Show for E with output(_self, logger) {
  logger.write_string("empty_struct")
}

fn use_show(s: &Show) -> Unit {
  println(s.to_string())
}

fn init {
  f1(A(E::{}))
  use_show(E::{})
}
