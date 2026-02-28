fn use_show(x: &Show) -> String {
  x.to_string()
}

fn init {
  let s: String? = Option::None
  println(use_show(s))
}
