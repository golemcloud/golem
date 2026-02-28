fn init {
  let x = 3 // needed based on the current recovery algorithm
  {
    let x = 4
    println(x) // print 4
  }
  let z = 2
  println(x) // print 3
}

fn init {
  let x = 3 // needed based on the current recovery algorithm
  {
    let x = 4
    ()
  }
  let z = 2
  println(x) // print 3
}

