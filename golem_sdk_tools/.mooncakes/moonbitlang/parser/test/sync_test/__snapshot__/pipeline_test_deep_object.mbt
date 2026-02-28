enum Deep {
  Done
  More(Deep)
}

fn make(depth : Int) -> Deep {
  loop (depth, Done) {
    (0, acc) => acc
    (n, acc) => continue (n - 1, More(acc))
  }
}

fn main {
  ignore(make(1 << 18))
}
