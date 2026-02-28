fn init {
    let m1 = { "x": {"x" : 2 }, "y": { "z" : 3 } }
    match m1 {
      { "x": { "x"? : v, .. }, .. } => println(v)
      _ => panic()
    }
}
