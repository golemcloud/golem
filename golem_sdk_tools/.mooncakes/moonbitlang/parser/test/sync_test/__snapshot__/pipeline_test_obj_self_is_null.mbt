fn use_show(obj : &Show) -> Unit {
  // use twice to trigger [incref]
  println(obj)
  println(obj)
}

fn init {
  use_show((None : String?))
}
