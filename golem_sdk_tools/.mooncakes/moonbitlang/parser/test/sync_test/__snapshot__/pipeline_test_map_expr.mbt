fn init {
  let m1 = { "x": 1, "y": 2 }
  m1.each((k, v) => println("\{k}: \{v}"))

  let m2 = { 1: "x", 2: "y" }
  m2.each((k, v) => println("\{k}: \{v}"))
}
