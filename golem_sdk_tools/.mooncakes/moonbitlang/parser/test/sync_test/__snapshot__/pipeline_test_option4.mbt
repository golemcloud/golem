enum T {
  Next(T?)
}

pub fn f1(x: T?) -> Int {
  loop (x, 0) {
    (Some(Next(t)), acc) => continue (t, acc + 1)
    (None, acc) => acc
  }
}

fn init {
  println(f1(Some(Next(None))))
}
