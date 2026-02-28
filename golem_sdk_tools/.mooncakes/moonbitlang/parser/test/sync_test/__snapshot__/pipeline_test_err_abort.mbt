
fn f(msg: String) -> Unit {
  if msg.length() < 10 { println("\{msg}\n") } else { abort(msg) }
}

fn init {
  f("12345")
  f("message too long")
}
