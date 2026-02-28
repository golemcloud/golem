fn init {
  let m1 = { }
  m1.set("x", 1)
  m1.set("y", 2)
  m1.each((k, v) => println("\{k}: \{v}"))

  let m2 = {  }
  m2.set(1, "x")
  m2.set(2, "y")
  m2.each((k, v) => println("\{k}: \{v}"))
}
