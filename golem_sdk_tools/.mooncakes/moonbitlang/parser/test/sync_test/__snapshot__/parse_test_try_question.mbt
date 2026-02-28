fn init {
  try? f()
  try? 1 + f()
  try? f(g())
  try? {
    f()
    g()
  }
}
