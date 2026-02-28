//!build: -w -17-18

fn train() -> Double {
  let (correct, wrong) = loop 0 {
    i => { (45, 56) }
  }
  correct.to_double()
}

fn init {
  println(train())
}

fn f(x: Int) -> String {
  let (a, b) = loop x {
    n => if n > 0 { break("sprint", "summer") } else { continue n + 1 }
  }
  a + b
}

fn g(x:Int?) -> String {
  let (a, _, b) = loop x {
    None => break("Jan", "Feb", "Mar")
    Some(_) => continue None
  }
  a + b
}

fn h(x: String) -> String {
  match (loop x { _ => break (x, x) }) {
    (s1, s2) => s1 + s2
  }
}

fn init {
  println(f(-5))
  println(g(Some(10)))
  println(h("loopy"))
}
