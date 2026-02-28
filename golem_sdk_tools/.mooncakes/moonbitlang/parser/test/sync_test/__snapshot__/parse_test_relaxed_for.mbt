fn init {
  for i = 0, j = 0 {
    if i == j { break 3 }
  }
  for {
    break 3
  }
  for i = 0, j = 0 // auto-inserted semi, so this is not treated as relaxed-for
  {
    break
  }
  let a = 0

}
