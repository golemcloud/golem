fn nested(n: Int, s: String) -> Unit {
  loop 0 {
    i => if i < n {
      loop i.to_string() {
        a => if a.length() <= i {
          println(a)
          continue a + s
        }
      }
      continue i + s.length()
    }
  }
}

fn init {
  nested(5, "!")
}
