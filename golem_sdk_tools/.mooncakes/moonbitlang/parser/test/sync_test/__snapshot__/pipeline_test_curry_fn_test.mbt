fn main {
  let add = () => x => y => x + y // zero-arity function
  add()(1)(2) |> println
  [[1, 2, 3], [2, 3]].each(x => x.each(y => println(y)))
  let f = _ => println(42) // one-arity function discarded
  f(0)  
  let f = ( _ : Unit) => println(42)
  f(())
}


test {
  let f = _ => println(42)
  f(0)  
}

test {
  let f = ( _ : Unit) => println(42)
  f(())
}
