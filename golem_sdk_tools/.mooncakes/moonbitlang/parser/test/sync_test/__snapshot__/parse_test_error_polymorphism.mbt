fn[X] each(self : Array[X], f : (X) -> Unit raise?) -> Unit raise? {
  for x in self {
    f(x)
  }
}
