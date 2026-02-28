let dead_global = 0

pub fn pub_dead_func() -> Unit {
  println("hello")
}

fn dead_func() -> Unit {
  println("hello")
}

fn init {
  let unused_let = dead_global
  let mut unused_var = dead_global
  fn dead_fn() { dead_func() }
  println("see you")
}
