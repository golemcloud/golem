enumview[A: Show] V {
  SNil
  SCons(String, List[A])
} for List[A] with view(x: List[A]) {
  match x {
    Nil => SNil
    Cons(x, xs) => SCons(x.to_string(), xs)
  }
}

enumview V {
  C
  D
} for A with view(x) {
  C
}
